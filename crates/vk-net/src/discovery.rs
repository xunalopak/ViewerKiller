//! Découverte de l'hôte sur le VPN.
//!
//! mDNS/broadcast ne traversant généralement pas un VPN niveau 3, on procède par
//! **balayage** du sous-réseau : pour chaque adresse, on tente une connexion TCP
//! courte et on envoie une sonde `Probe { code }`. L'hôte qui correspond répond
//! `ProbeResult { matches: true }`.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use vk_core::protocol::{DiscoveryMessage, PROTO_VERSION};

use crate::frame::{read_framed, write_framed};

/// Nombre maximal d'adresses balayées (garde-fou pour les sous-réseaux larges).
pub const MAX_SCAN_HOSTS: usize = 4096;

/// Une interface IPv4 locale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceV4 {
    pub name: String,
    pub ip: Ipv4Addr,
    /// Longueur du préfixe réseau (ex. 24 pour un /24).
    pub prefix: u8,
}

/// Liste les interfaces IPv4 non-loopback de la machine.
pub fn list_ipv4_interfaces() -> Vec<InterfaceV4> {
    let mut out = Vec::new();
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for ifa in ifaces {
            if let if_addrs::IfAddr::V4(v4) = ifa.addr {
                if v4.ip.is_loopback() {
                    continue;
                }
                out.push(InterfaceV4 {
                    name: ifa.name,
                    ip: v4.ip,
                    prefix: netmask_to_prefix(v4.netmask),
                });
            }
        }
    }
    out
}

/// Tente de deviner l'interface du VPN WireGuard.
///
/// Heuristique : d'abord par nom (`wg*`, `wireguard`, `wintun` côté Windows),
/// puis repli sur la première interface en adresse privée qui n'est pas un pont
/// Docker/conteneur.
pub fn guess_wireguard_interface() -> Option<InterfaceV4> {
    let ifaces = list_ipv4_interfaces();

    if let Some(found) = ifaces.iter().find(|i| {
        let n = i.name.to_lowercase();
        n.starts_with("wg") || n.contains("wireguard") || n.contains("wintun")
    }) {
        return Some(found.clone());
    }

    ifaces.into_iter().find(|i| {
        i.ip.is_private()
            && !i.name.starts_with("docker")
            && !i.name.starts_with("br-")
            && !i.name.starts_with("veth")
    })
}

/// Énumère les adresses hôtes utilisables d'un sous-réseau IPv4.
///
/// Pour un préfixe < 31, l'adresse réseau et l'adresse de diffusion sont
/// exclues. Le résultat est plafonné à [`MAX_SCAN_HOSTS`].
pub fn hosts_in_subnet(ip: Ipv4Addr, prefix: u8) -> Vec<Ipv4Addr> {
    if prefix == 0 || prefix > 32 {
        return Vec::new();
    }
    let bits = 32 - prefix as u32; // 0..=31
    let mask: u32 = if bits == 0 { u32::MAX } else { u32::MAX << bits };
    let total: u32 = 1u32 << bits;
    let network = u32::from(ip) & mask;

    let mut hosts = Vec::new();
    if prefix >= 31 {
        for off in 0..total {
            hosts.push(Ipv4Addr::from(network + off));
        }
    } else {
        for off in 1..(total - 1) {
            hosts.push(Ipv4Addr::from(network + off));
            if hosts.len() >= MAX_SCAN_HOSTS {
                break;
            }
        }
    }
    hosts
}

/// Balaye `hosts` en parallèle et renvoie l'adresse de l'hôte qui correspond au
/// `code`, ou `None` si aucun ne correspond dans les délais.
pub async fn find_host_by_code(
    hosts: Vec<Ipv4Addr>,
    port: u16,
    code: String,
    per_host_timeout: Duration,
    concurrency: usize,
) -> Option<SocketAddr> {
    let sem = Arc::new(Semaphore::new(concurrency.max(1)));
    let mut set = JoinSet::new();

    for ip in hosts {
        let sem = sem.clone();
        let code = code.clone();
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.ok()?;
            let addr = SocketAddr::new(IpAddr::V4(ip), port);
            if probe_one(addr, &code, per_host_timeout).await {
                Some(addr)
            } else {
                None
            }
        });
    }

    while let Some(res) = set.join_next().await {
        if let Ok(Some(addr)) = res {
            set.abort_all();
            return Some(addr);
        }
    }
    None
}

/// Envoie une sonde unique à `addr` et indique si l'hôte correspond au code.
async fn probe_one(addr: SocketAddr, code: &str, timeout: Duration) -> bool {
    let Ok(Ok(mut stream)) = tokio::time::timeout(timeout, TcpStream::connect(addr)).await else {
        return false;
    };

    let probe = DiscoveryMessage::Probe {
        proto_version: PROTO_VERSION,
        code: code.to_string(),
    };
    if tokio::time::timeout(timeout, write_framed(&mut stream, &probe))
        .await
        .map(|r| r.is_ok())
        != Ok(true)
    {
        return false;
    }

    matches!(
        tokio::time::timeout(timeout, read_framed::<_, DiscoveryMessage>(&mut stream)).await,
        Ok(Ok(DiscoveryMessage::ProbeResult { matches: true, .. }))
    )
}

fn netmask_to_prefix(mask: Ipv4Addr) -> u8 {
    u32::from(mask).count_ones() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_24_yields_254_hosts() {
        let hosts = hosts_in_subnet(Ipv4Addr::new(10, 0, 0, 5), 24);
        assert_eq!(hosts.len(), 254);
        assert_eq!(hosts[0], Ipv4Addr::new(10, 0, 0, 1));
        assert_eq!(hosts[253], Ipv4Addr::new(10, 0, 0, 254));
    }

    #[test]
    fn prefix_32_yields_single_host() {
        let hosts = hosts_in_subnet(Ipv4Addr::new(10, 0, 0, 5), 32);
        assert_eq!(hosts, vec![Ipv4Addr::new(10, 0, 0, 5)]);
    }

    #[test]
    fn large_subnet_is_capped() {
        let hosts = hosts_in_subnet(Ipv4Addr::new(10, 0, 0, 1), 8);
        assert_eq!(hosts.len(), MAX_SCAN_HOSTS);
    }

    #[test]
    fn netmask_conversion() {
        assert_eq!(netmask_to_prefix(Ipv4Addr::new(255, 255, 255, 0)), 24);
        assert_eq!(netmask_to_prefix(Ipv4Addr::new(255, 255, 0, 0)), 16);
    }
}
