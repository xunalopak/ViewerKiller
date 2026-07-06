//! Côté contrôleur : connexion directe à l'hôte, handshake, puis pont entre la
//! session chiffrée et l'interface via des canaux.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use vk_core::crypto::derive_psk;
use vk_core::protocol::{
    ControllerMessage, DiscoveryMessage, FrameUpdate, HostMessage, InputEvent, MonitorInfo,
    KEEPALIVE_INTERVAL, PROTO_VERSION, SESSION_TIMEOUT,
};
use vk_net::frame::{read_framed, write_framed};
use vk_net::transport::EncryptedStream;

use crate::clipboard::ClipboardSync;

/// Période de sondage du presse-papiers local (synchronisation façon RDP).
const CLIPBOARD_POLL: Duration = Duration::from_millis(500);

/// Délai d'attente **avant** l'établissement de la session (réception du premier
/// `ScreenInfo`). Doit dépasser le temps de consentement côté hôte (~30 s) : tant
/// que l'utilisateur distant n'a pas accepté, aucun message n'arrive.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(45);

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
    ScreenInfo {
        width: u32,
        height: u32,
    },
    Frame(FrameUpdate),
    /// Liste des moniteurs de l'hôte (choix multi-écrans, J12).
    Monitors(Vec<MonitorInfo>),
    /// La connexion a été perdue ; une reconnexion automatique est en cours.
    Reconnecting,
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

/// Issue d'une session (une connexion). Détermine si une reconnexion doit être
/// tentée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEnd {
    /// L'UI locale a fermé la session (`input_rx` clos) : ne pas reconnecter.
    UserQuit,
    /// L'hôte a mis fin proprement (`Bye`) : ne pas reconnecter.
    HostClosed,
    /// Connexion perdue (erreur réseau ou délai dépassé) : reconnexion possible.
    Dropped,
}

/// Traduit une perte de connexion en [`SessionEnd`] : on ne reconnecte **que** si
/// la session avait réellement été établie (au moins un `ScreenInfo` reçu). Une
/// coupure avant établissement = refus de consentement, version incompatible ou
/// hôte injoignable → terminal, pas de reconnexion (sinon la demande de connexion
/// « rebondit » en boucle côté hôte).
fn drop_end(established: bool) -> SessionEnd {
    if established {
        SessionEnd::Dropped
    } else {
        SessionEnd::HostClosed
    }
}

/// Politique de reconnexion automatique côté contrôleur (backoff exponentiel).
#[derive(Debug, Clone)]
pub struct ReconnectPolicy {
    /// Si faux, une coupure termine immédiatement la session.
    pub enabled: bool,
    /// Nombre maximal de tentatives avant abandon.
    pub max_attempts: u32,
    /// Délai avant la première tentative.
    pub initial_backoff: Duration,
    /// Plafond du délai (le backoff double à chaque échec jusqu'à ce plafond).
    pub max_backoff: Duration,
}

impl ReconnectPolicy {
    /// Reconnexion désactivée (utile pour les tests et la CLI ponctuelle).
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            max_attempts: 10,
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(10),
        }
    }
}

/// Boucle d'une session (une connexion) : reçoit les trames (vers `events_tx`),
/// envoie les entrées (depuis `input_rx`) et, si `share_clipboard`, synchronise
/// le presse-papiers. Émet un keepalive périodique et abandonne la connexion si
/// l'hôte devient muet au-delà de [`SESSION_TIMEOUT`].
///
/// Ne signale pas [`SessionEvent::Disconnected`] : c'est [`run_controller`] qui
/// décide, selon le [`SessionEnd`] renvoyé, de reconnecter ou de terminer.
pub async fn controller_session(
    mut enc: EncryptedStream<TcpStream>,
    events_tx: &UnboundedSender<SessionEvent>,
    input_rx: &mut UnboundedReceiver<InputEvent>,
    monitor_rx: &mut UnboundedReceiver<u32>,
    share_clipboard: bool,
) -> SessionEnd {
    let mut clipboard =
        share_clipboard.then(|| ClipboardSync::new(vk_platform::default_clipboard()));
    let mut clip_ticker = tokio::time::interval(CLIPBOARD_POLL);

    let mut keepalive = tokio::time::interval(KEEPALIVE_INTERVAL);
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut watchdog = tokio::time::interval(KEEPALIVE_INTERVAL);
    watchdog.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_rx = Instant::now();
    let mut monitor_open = true;
    // Vrai dès le premier `ScreenInfo` reçu : conditionne la reconnexion et le
    // délai d'attente (long avant établissement, pour couvrir le consentement).
    let mut established = false;

    loop {
        tokio::select! {
            msg = enc.recv::<HostMessage>() => {
                last_rx = Instant::now();
                match msg {
                    Ok(HostMessage::ScreenInfo { width, height }) => {
                        established = true;
                        let _ = events_tx.send(SessionEvent::ScreenInfo { width, height });
                    }
                    Ok(HostMessage::Frame(update)) => {
                        if events_tx.send(SessionEvent::Frame(update)).is_err() {
                            return SessionEnd::UserQuit; // l'UI a fermé le récepteur
                        }
                    }
                    Ok(HostMessage::Clipboard(text)) => {
                        if let Some(c) = clipboard.as_mut() {
                            c.apply_remote(text);
                        }
                    }
                    Ok(HostMessage::Ping) => {}
                    Ok(HostMessage::Monitors(list)) => {
                        let _ = events_tx.send(SessionEvent::Monitors(list));
                    }
                    Ok(HostMessage::Bye) => return SessionEnd::HostClosed,
                    Err(e) => {
                        tracing::warn!("réception interrompue : {e:#}");
                        return drop_end(established);
                    }
                }
            }
            _ = keepalive.tick() => {
                if enc.send(&ControllerMessage::Ping).await.is_err() {
                    return drop_end(established);
                }
            }
            _ = watchdog.tick() => {
                let timeout = if established { SESSION_TIMEOUT } else { CONNECT_TIMEOUT };
                if last_rx.elapsed() > timeout {
                    tracing::warn!("session : hôte silencieux (délai dépassé)");
                    return drop_end(established);
                }
            }
            _ = clip_ticker.tick(), if clipboard.is_some() => {
                if let Some(text) = clipboard.as_mut().and_then(ClipboardSync::poll_local) {
                    if enc.send(&ControllerMessage::Clipboard(text)).await.is_err() {
                        return drop_end(established);
                    }
                }
            }
            idx = monitor_rx.recv(), if monitor_open => {
                match idx {
                    // Demande de bascule de moniteur émise par l'UI (J12).
                    Some(index) => {
                        if enc
                            .send(&ControllerMessage::SelectMonitor { index })
                            .await
                            .is_err()
                        {
                            return drop_end(established);
                        }
                    }
                    // Canal fermé : on n'écoute plus cette branche (pas de boucle folle).
                    None => monitor_open = false,
                }
            }
            ev = input_rx.recv() => {
                match ev {
                    Some(ev) => {
                        if enc.send(&ControllerMessage::Input(ev)).await.is_err() {
                            return drop_end(established);
                        }
                    }
                    None => {
                        let _ = enc.send(&ControllerMessage::Bye).await;
                        return SessionEnd::UserQuit;
                    }
                }
            }
        }
    }
}

/// Pilote une session avec **reconnexion automatique**. Exécute la session
/// courante ; en cas de coupure ([`SessionEnd::Dropped`]) tente de se reconnecter
/// à la même adresse avec les mêmes identifiants (backoff exponentiel) en
/// réutilisant les canaux d'UI. Émet [`SessionEvent::Reconnecting`] pendant les
/// tentatives et [`SessionEvent::Disconnected`] une fois terminé ou abandonné.
#[allow(clippy::too_many_arguments)]
pub async fn run_controller(
    first: EncryptedStream<TcpStream>,
    addr: SocketAddr,
    config: ControllerConfig,
    events_tx: UnboundedSender<SessionEvent>,
    mut input_rx: UnboundedReceiver<InputEvent>,
    mut monitor_rx: UnboundedReceiver<u32>,
    share_clipboard: bool,
    policy: ReconnectPolicy,
) -> Result<()> {
    let mut enc = first;
    loop {
        match controller_session(
            enc,
            &events_tx,
            &mut input_rx,
            &mut monitor_rx,
            share_clipboard,
        )
        .await
        {
            SessionEnd::UserQuit | SessionEnd::HostClosed => break,
            SessionEnd::Dropped => {
                if !policy.enabled {
                    break;
                }
                match reconnect(addr, &config, &events_tx, &mut input_rx, &policy).await {
                    Some(new_enc) => enc = new_enc,
                    None => break,
                }
            }
        }
    }
    let _ = events_tx.send(SessionEvent::Disconnected);
    Ok(())
}

/// Tente de rétablir la connexion avec backoff exponentiel. Renvoie le nouveau
/// flux chiffré, ou `None` si toutes les tentatives échouent ou si l'UI ferme la
/// session entre-temps (`input_rx` clos).
async fn reconnect(
    addr: SocketAddr,
    config: &ControllerConfig,
    events_tx: &UnboundedSender<SessionEvent>,
    input_rx: &mut UnboundedReceiver<InputEvent>,
    policy: &ReconnectPolicy,
) -> Option<EncryptedStream<TcpStream>> {
    let _ = events_tx.send(SessionEvent::Reconnecting);
    let mut backoff = policy.initial_backoff;
    for attempt in 1..=policy.max_attempts {
        // Attente du backoff, interruptible si l'UI ferme la session.
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            ev = input_rx.recv() => {
                // `None` = l'UI a fermé la session pendant la coupure → abandon.
                // Une entrée reçue pendant la coupure est ignorée (pas de session).
                ev?;
            }
        }
        match connect_to(addr, config).await {
            Ok(enc) => {
                tracing::info!(attempt, "reconnexion réussie");
                return Some(enc);
            }
            Err(e) => {
                tracing::warn!(
                    attempt,
                    max = policy.max_attempts,
                    "échec de reconnexion : {e:#}"
                );
                backoff = (backoff * 2).min(policy.max_backoff);
            }
        }
    }
    tracing::warn!(attempts = policy.max_attempts, "abandon de la reconnexion");
    None
}
