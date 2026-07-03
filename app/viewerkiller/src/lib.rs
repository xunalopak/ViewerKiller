//! Orchestration de ViewerKiller : boucles de session côté hôte et contrôleur.
//!
//! La logique réseau est découplée de l'interface : le contrôleur communique
//! avec l'UI via des canaux ([`controller::SessionEvent`] en sortie,
//! [`vk_core::protocol::InputEvent`] en entrée), ce qui rend l'ensemble
//! testable sans affichage ni matériel Windows (voir `tests/e2e.rs`).

pub mod controller;
pub mod host;
pub mod security;

pub use controller::{controller_session, ControllerConfig, SessionEvent};
pub use host::{handle_connection, serve, ConnectionOutcome, HostConfig};
pub use security::{AutoAccept, BruteForceGuard, Consent, RejectAll};

/// Génère un code de connexion à 6 chiffres et un mot de passe aléatoire fort.
pub fn generate_credentials() -> (String, String) {
    use rand::{distributions::Alphanumeric, Rng};
    let mut rng = rand::thread_rng();
    let code = format!("{:06}", rng.gen_range(0..1_000_000u32));
    let password: String = (0..12).map(|_| rng.sample(Alphanumeric) as char).collect();
    (code, password)
}

/// Adresses IPv4 locales (nom d'interface, adresse) à communiquer au
/// contrôleur : Wi-Fi, Ethernet, VPN… Exclut le loopback et
/// l'auto-configuration (169.254/16). Affichage uniquement — aucun balayage.
pub fn local_ipv4_addresses() -> Vec<(String, std::net::Ipv4Addr)> {
    let mut out = Vec::new();
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for ifa in ifaces {
            if let if_addrs::IfAddr::V4(v4) = ifa.addr {
                if v4.ip.is_loopback() || v4.ip.is_link_local() {
                    continue;
                }
                out.push((ifa.name, v4.ip));
            }
        }
    }
    out
}
