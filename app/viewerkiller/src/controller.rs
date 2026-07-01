//! Côté contrôleur : connexion directe à l'hôte, handshake, puis pont entre la
//! session chiffrée et l'interface via des canaux.

use std::net::SocketAddr;

use anyhow::{Context, Result};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use vk_core::crypto::derive_psk;
use vk_core::protocol::{
    ControllerMessage, DiscoveryMessage, FrameUpdate, HostMessage, InputEvent, PROTO_VERSION,
};
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
    Disconnected,
}

/// Se connecte directement à une adresse connue (l'hôte doit déjà écouter et
/// attendre la connexion).
pub async fn connect_to(
    addr: SocketAddr,
    config: &ControllerConfig,
) -> Result<EncryptedStream<TcpStream>> {
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
}

/// Boucle de session : reçoit les trames (vers `events_tx`) et envoie les
/// entrées (depuis `input_rx`).
pub async fn controller_session(
    mut enc: EncryptedStream<TcpStream>,
    events_tx: UnboundedSender<SessionEvent>,
    mut input_rx: UnboundedReceiver<InputEvent>,
) -> Result<()> {
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
                        tracing::warn!("réception interrompue : {e:#}");
                        break;
                    }
                }
            }
            ev = input_rx.recv() => {
                match ev {
                    Some(ev) => enc.send(&ControllerMessage::Input(ev)).await?,
                    None => {
                        let _ = enc.send(&ControllerMessage::Bye).await;
                        break;
                    }
                }
            }
        }
    }
    let _ = events_tx.send(SessionEvent::Disconnected);
    Ok(())
}
