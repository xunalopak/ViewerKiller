//! Côté hôte : écoute sur l'adresse configurée, répond aux sondes de
//! vérification de code, établit la session chiffrée, puis diffuse l'écran et
//! injecte les entrées.
//!
//! Durcissement intégré : limiteur anti-bruteforce par IP, demande de
//! consentement après authentification, et journal d'audit (cible `audit`).

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::net::{TcpListener, TcpStream};

use vk_core::crypto::derive_psk;
use vk_core::protocol::{
    ControllerMessage, DiscoveryMessage, HostMessage, InputEvent, KEEPALIVE_INTERVAL,
    PROTO_VERSION, SESSION_TIMEOUT,
};
use vk_media::{QualityController, TileEncoder};
use vk_net::frame::{read_framed, write_framed};
use vk_net::transport::EncryptedStream;
use vk_net::NetError;
use vk_platform::{InputInjector, ScreenCapturer};

use crate::clipboard::ClipboardSync;
use crate::security::{BruteForceGuard, Consent};

/// Période de sondage du presse-papiers local (synchronisation façon RDP).
const CLIPBOARD_POLL: Duration = Duration::from_millis(500);

/// Configuration d'un hôte.
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// Adresse d'écoute.
    pub bind_addr: SocketAddr,
    pub code: String,
    pub password: String,
    pub host_name: String,
    pub tile_size: u32,
    pub quality: u8,
    pub fps: u32,
    /// Si vrai, demande un consentement explicite avant chaque session.
    pub require_consent: bool,
    /// Si vrai, synchronise le presse-papiers texte avec le contrôleur.
    pub share_clipboard: bool,
}

/// Issue du traitement d'une connexion entrante.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionOutcome {
    /// IP verrouillée par l'anti-bruteforce ; connexion refusée d'emblée.
    Locked,
    /// Le code ne correspond pas (souvent une simple sonde de balayage).
    CodeMismatch,
    /// Le pair a fermé avant le handshake (cas normal du balayage).
    Benign,
    /// Handshake échoué : mauvais mot de passe.
    AuthFailed,
    /// Connexion refusée par l'utilisateur (consentement).
    Refused,
    /// Session menée à son terme.
    Completed,
}

/// Construit un capteur/injecteur à la demande (une instance par session).
pub type CapturerFactory = dyn FnMut() -> Result<Box<dyn ScreenCapturer>> + Send;
pub type InjectorFactory = dyn FnMut() -> Result<Box<dyn InputInjector>> + Send;

/// Boucle d'écoute : sert les contrôleurs un à un (MVP mono-session).
pub async fn serve(
    config: &HostConfig,
    make_capturer: &mut CapturerFactory,
    make_injector: &mut InjectorFactory,
    guard: &mut BruteForceGuard,
    consent: &mut dyn Consent,
) -> Result<()> {
    let listener = TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("liaison impossible sur {}", config.bind_addr))?;
    tracing::info!(target: "audit", addr = %config.bind_addr, code = %config.code, "hôte en écoute");

    loop {
        let (stream, peer) = listener.accept().await?;
        let capturer = make_capturer()?;
        let injector = make_injector()?;
        match handle_connection(stream, config, guard, consent, capturer, injector).await {
            Ok(outcome) => tracing::debug!(?outcome, %peer, "connexion traitée"),
            Err(e) => tracing::debug!(%peer, "erreur de connexion : {e:#}"),
        }
    }
}

/// Traite une connexion entrante : anti-bruteforce → découverte → handshake →
/// consentement → session.
pub async fn handle_connection(
    mut stream: TcpStream,
    config: &HostConfig,
    guard: &mut BruteForceGuard,
    consent: &mut dyn Consent,
    capturer: Box<dyn ScreenCapturer>,
    injector: Box<dyn InputInjector>,
) -> Result<ConnectionOutcome> {
    let peer = stream.peer_addr()?;
    let ip = peer.ip();

    if !guard.check(ip) {
        tracing::warn!(target: "audit", %peer, "connexion refusée (verrouillage anti-bruteforce)");
        return Ok(ConnectionOutcome::Locked);
    }

    // 1. Découverte : le code doit correspondre ET la version de protocole être
    //    identique (sinon la session échouerait plus loin par un message
    //    cryptique ; on refuse tôt en le journalisant).
    let probe: DiscoveryMessage = read_framed(&mut stream).await?;
    let matches = match &probe {
        DiscoveryMessage::Probe {
            code,
            proto_version,
        } => {
            if *proto_version != PROTO_VERSION {
                tracing::warn!(
                    target: "audit", %peer, theirs = proto_version, ours = PROTO_VERSION,
                    "version de protocole incompatible"
                );
            }
            *code == config.code && *proto_version == PROTO_VERSION
        }
        _ => false,
    };
    write_framed(
        &mut stream,
        &DiscoveryMessage::ProbeResult {
            matches,
            host_name: config.host_name.clone(),
        },
    )
    .await?;
    if !matches {
        return Ok(ConnectionOutcome::CodeMismatch);
    }

    // 2. Handshake Noise authentifié par le mot de passe.
    let psk = derive_psk(&config.password);
    let mut enc = match EncryptedStream::accept(stream, &psk).await {
        Ok(enc) => enc,
        // Bytes reçus mais tag invalide → mauvais mot de passe.
        Err(NetError::Crypto(_)) => {
            guard.record_failure(ip);
            tracing::warn!(target: "audit", %peer, "échec d'authentification (mot de passe)");
            return Ok(ConnectionOutcome::AuthFailed);
        }
        // Pair fermé avant le handshake (balayage) → bénin, non comptabilisé.
        Err(NetError::Io(_)) => return Ok(ConnectionOutcome::Benign),
        Err(e) => return Err(e.into()),
    };
    guard.record_success(ip);

    // 3. Consentement.
    if config.require_consent && !consent.request(peer).await {
        tracing::info!(target: "audit", %peer, "connexion refusée par l'utilisateur");
        let _ = enc.send(&HostMessage::Bye).await;
        return Ok(ConnectionOutcome::Refused);
    }

    tracing::info!(target: "audit", %peer, host = %config.host_name, "session établie");
    let result = host_session(enc, capturer, injector, config).await;
    // Toujours signaler la fin (même sur erreur) pour retirer l'indicateur UI.
    consent.session_ended(peer);
    result?;
    tracing::info!(target: "audit", %peer, "session terminée");
    Ok(ConnectionOutcome::Completed)
}

async fn host_session(
    mut enc: EncryptedStream<TcpStream>,
    mut capturer: Box<dyn ScreenCapturer>,
    mut injector: Box<dyn InputInjector>,
    config: &HostConfig,
) -> Result<()> {
    let (width, height) = capturer.dimensions();
    enc.send(&HostMessage::ScreenInfo { width, height }).await?;
    // Liste des moniteurs disponibles pour le choix côté contrôleur (J12).
    enc.send(&HostMessage::Monitors(capturer.monitors()))
        .await?;

    let mut encoder = TileEncoder::new(config.tile_size, config.quality);
    // Qualité adaptative (J10b) : baisse la qualité JPEG si une trame déborde la
    // période (réseau lent), remonte quand la marge revient.
    let mut quality_ctl = QualityController::new(config.quality);
    let period = Duration::from_secs_f64(1.0 / config.fps.max(1) as f64);
    let mut ticker = tokio::time::interval(period);
    // Cadence adaptative : si une trame (capture + encode + envoi) déborde la
    // période, on saute les ticks manqués au lieu de les rattraper en rafale —
    // le débit baisse tout seul sous charge, sans accumuler de retard.
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Presse-papiers partagé (façon RDP), seulement si activé.
    let mut clipboard = config
        .share_clipboard
        .then(|| ClipboardSync::new(vk_platform::default_clipboard()));
    let mut clip_ticker = tokio::time::interval(CLIPBOARD_POLL);
    clip_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Maintien de connexion : on émet un Ping périodique, et un chien de garde
    // ferme la session si plus rien n'arrive du contrôleur — détecte une coupure
    // (VPN tombé, contrôleur éteint) que TCP mettrait très longtemps à remonter.
    let mut keepalive = tokio::time::interval(KEEPALIVE_INTERVAL);
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut watchdog = tokio::time::interval(KEEPALIVE_INTERVAL);
    watchdog.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_rx = Instant::now();
    // Curseur distant (J12) : dernier type de curseur transmis (envoi au changement).
    let mut last_cursor: Option<vk_platform::CursorState> = None;

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let cycle_start = Instant::now();
                if let Some(frame) = capturer.capture()? {
                    // L'encodage JPEG est synchrone et coûteux : on le sort du
                    // runtime async (spawn_blocking) pour ne pas monopoliser un
                    // thread ouvrier. L'encodeur (état inter-trames) est déplacé
                    // dans la tâche puis récupéré. L'erreur est convertie en
                    // texte pour rester `Send` à travers la frontière de tâche.
                    let (returned, update) = tokio::task::spawn_blocking(move || {
                        let update = encoder.encode(&frame).map_err(|e| e.to_string());
                        (encoder, update)
                    })
                    .await?;
                    encoder = returned;
                    let update = update.map_err(anyhow::Error::msg)?;
                    if !update.tiles.is_empty()
                        && enc.send(&HostMessage::Frame(update)).await.is_err()
                    {
                        break; // contrôleur parti
                    }
                    // Qualité adaptative : ajuste selon le temps réel du cycle.
                    quality_ctl.observe(cycle_start.elapsed(), period);
                    encoder.set_quality(quality_ctl.quality());
                }
                // Curseur distant : signale au contrôleur tout changement de type.
                if let Some(cur) = vk_platform::probe_cursor() {
                    if last_cursor != Some(cur) {
                        last_cursor = Some(cur);
                        if enc
                            .send(&HostMessage::Cursor {
                                kind: cur.kind,
                                visible: cur.visible,
                            })
                            .await
                            .is_err()
                        {
                            break; // contrôleur parti
                        }
                    }
                }
            }
            _ = clip_ticker.tick(), if clipboard.is_some() => {
                if let Some(text) = clipboard.as_mut().and_then(ClipboardSync::poll_local) {
                    if enc.send(&HostMessage::Clipboard(text)).await.is_err() {
                        break; // contrôleur parti
                    }
                }
            }
            _ = keepalive.tick() => {
                if enc.send(&HostMessage::Ping).await.is_err() {
                    break; // contrôleur parti
                }
            }
            _ = watchdog.tick() => {
                if last_rx.elapsed() > SESSION_TIMEOUT {
                    tracing::warn!(target: "audit", "session fermée : contrôleur silencieux (délai dépassé)");
                    break;
                }
            }
            msg = enc.recv::<ControllerMessage>() => {
                last_rx = Instant::now();
                // Contrôleur parti (fermeture, reset réseau — sous Windows un RST
                // à la fermeture peut effacer le `Bye` en vol) : fin de session
                // normale, pas une erreur fatale ; on repart en écoute.
                let msg = match msg {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::debug!("réception interrompue (contrôleur parti) : {e:#}");
                        break;
                    }
                };
                match msg {
                    ControllerMessage::Input(ev) => apply_input(injector.as_mut(), ev)?,
                    ControllerMessage::RequestFullFrame => encoder.force_full_frame(),
                    ControllerMessage::Clipboard(text) => {
                        if let Some(c) = clipboard.as_mut() {
                            c.apply_remote(text);
                        }
                    }
                    ControllerMessage::Ping => {}
                    ControllerMessage::SelectMonitor { index } => {
                        match capturer.select_monitor(index) {
                            Ok(()) => {
                                // Nouvelle géométrie → le contrôleur redimensionne ;
                                // TileEncoder repart en trame pleine (détection du
                                // changement de dimensions).
                                let (width, height) = capturer.dimensions();
                                if enc
                                    .send(&HostMessage::ScreenInfo { width, height })
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                                tracing::info!(target: "audit", index, "moniteur sélectionné");
                            }
                            Err(e) => {
                                tracing::warn!(target: "audit", "sélection moniteur refusée : {e:#}")
                            }
                        }
                    }
                    ControllerMessage::Bye => {
                        tracing::info!("contrôleur déconnecté");
                        break;
                    }
                }
            }
        }
    }
    Ok(())
}

fn apply_input(injector: &mut dyn InputInjector, ev: InputEvent) -> Result<()> {
    match ev {
        InputEvent::MouseMove { x, y } => injector.mouse_move(x, y),
        InputEvent::MouseButton { button, pressed } => injector.mouse_button(button, pressed),
        InputEvent::MouseScroll { dx, dy } => injector.mouse_scroll(dx, dy),
        InputEvent::Key { key, pressed } => injector.key(key, pressed),
        InputEvent::Char { c } => injector.char_input(c),
    }
}
