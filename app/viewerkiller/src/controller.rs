//! Côté contrôleur : découverte de l'hôte sur le réseau local, handshake, puis
//! pont entre la session chiffrée et l'interface via des canaux.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use vk_core::crypto::derive_psk;
use vk_core::protocol::{
    ControllerMessage, DiscoveryMessage, FrameUpdate, HostMessage, InputEvent, PROTO_VERSION,
};
use vk_net::discovery::{find_host_by_code, guess_lan_interface, hosts_in_subnet};
use vk_net::frame::{read_framed, write_framed};
use vk_net::transport::EncryptedStream;

/// Configuration d'un contrôleur.
#[derive(Debug, Clone)]
pub struct ControllerConfig {
    pub code: String,
    pub password: String,
    pub port: u16,
}

/// Événement remonté vers l'interface.
#[derive(Debug)]
pub enum SessionEvent {
    ScreenInfo { width: u32, height: u32 },
    Frame(FrameUpdate),
    /// Fin de session. `Some(raison)` en cas de coupure anormale (réseau,
    /// déchiffrement…) ; `None` pour une fin propre (Bye ou UI fermée).
    Disconnected(Option<String>),
}

/// Délai maximal pour établir une session (connexion TCP + sonde + handshake).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Découvre l'hôte affichant le code sur le réseau local, puis établit la session.
///
/// `subnet` permet de forcer un sous-réseau ; sinon il est déduit de
/// l'interface réseau locale détectée.
pub async fn discover_and_connect(
    subnet: Option<(Ipv4Addr, u8)>,
    config: &ControllerConfig,
    per_host_timeout: Duration,
) -> Result<EncryptedStream<TcpStream>> {
    let (ip, prefix) = match subnet {
        Some(s) => s,
        None => {
            let iface = guess_lan_interface()
                .context("aucune interface réseau locale détectée ; précisez le sous-réseau")?;
            tracing::info!(name = %iface.name, ip = %iface.ip, prefix = iface.prefix, "interface réseau locale");
            (iface.ip, iface.prefix)
        }
    };

    let hosts = hosts_in_subnet(ip, prefix);
    tracing::info!(count = hosts.len(), "balayage du sous-réseau");
    let addr = find_host_by_code(
        hosts,
        config.port,
        config.code.clone(),
        per_host_timeout,
        128,
    )
    .await
    .context("aucun hôte ne correspond à ce code sur le réseau local")?;
    tracing::info!(%addr, "hôte trouvé");
    connect_to(addr, config).await
}

/// Se connecte directement à une adresse connue (utilisé après découverte, ou
/// en tests).
pub async fn connect_to(
    addr: SocketAddr,
    config: &ControllerConfig,
) -> Result<EncryptedStream<TcpStream>> {
    // Garde-fou : si l'hôte accepte la connexion TCP mais ne répond plus ensuite
    // (trou noir MTU du tunnel qui bloque les gros paquets, pare-feu, hôte figé…),
    // on échoue proprement au lieu de bloquer l'interface indéfiniment.
    tokio::time::timeout(CONNECT_TIMEOUT, async {
        let mut stream = TcpStream::connect(addr).await?;

        // Confirme le code auprès de l'hôte avant le handshake.
        write_framed(
            &mut stream,
            &DiscoveryMessage::Probe {
                proto_version: PROTO_VERSION,
                code: config.code.clone(),
            },
        )
        .await?;
        match read_framed::<_, DiscoveryMessage>(&mut stream).await? {
            DiscoveryMessage::ProbeResult { matches: true, .. } => {}
            _ => anyhow::bail!("l'hôte ne reconnaît pas ce code"),
        }

        let psk = derive_psk(&config.password);
        EncryptedStream::connect(stream, &psk)
            .await
            .context("handshake Noise (mot de passe incorrect ?)")
    })
    .await
    .context("délai de connexion dépassé (hôte injoignable ou réseau saturé ?)")?
}

/// Boucle de session : reçoit les trames (vers `events_tx`) et envoie les
/// entrées (depuis `input_rx`).
pub async fn controller_session(
    mut enc: EncryptedStream<TcpStream>,
    events_tx: UnboundedSender<SessionEvent>,
    mut input_rx: UnboundedReceiver<InputEvent>,
) -> Result<()> {
    // Raison de fin : renseignée uniquement en cas de coupure anormale, pour que
    // l'UI distingue un vrai problème réseau d'une fin propre (Bye / fermeture).
    let mut reason: Option<String> = None;
    loop {
        tokio::select! {
            msg = enc.recv::<HostMessage>() => {
                match msg {
                    Ok(HostMessage::ScreenInfo { width, height }) => {
                        let _ = events_tx.send(SessionEvent::ScreenInfo { width, height });
                    }
                    Ok(HostMessage::Frame(update)) => {
                        if events_tx.send(SessionEvent::Frame(update)).is_err() {
                            break; // l'UI a fermé le récepteur
                        }
                    }
                    Ok(HostMessage::Bye) => break,
                    Err(e) => {
                        let m = format!("réception interrompue : {e:#}");
                        tracing::warn!("{m}");
                        reason = Some(m);
                        break;
                    }
                }
            }
            ev = input_rx.recv() => {
                match ev {
                    // Un échec d'envoi = coupure réseau : on le signale au lieu de
                    // propager l'erreur (ce qui laissait l'UI sans « Disconnected »).
                    Some(ev) => {
                        if let Err(e) = enc.send(&ControllerMessage::Input(ev)).await {
                            let m = format!("envoi interrompu : {e:#}");
                            tracing::warn!("{m}");
                            reason = Some(m);
                            break;
                        }
                    }
                    None => {
                        let _ = enc.send(&ControllerMessage::Bye).await;
                        break;
                    }
                }
            }
        }
    }
    let _ = events_tx.send(SessionEvent::Disconnected(reason));
    Ok(())
}
