//! Côté hôte : écoute sur l'adresse configurée, répond aux sondes de
//! vérification de code, établit la session chiffrée, puis diffuse l'écran et
//! injecte les entrées.
//!
//! Durcissement intégré : limiteur anti-bruteforce par IP, demande de
//! consentement après authentification, et journal d'audit (cible `audit`).

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::net::{TcpListener, TcpStream};

use vk_core::crypto::derive_psk;
use vk_core::protocol::{ControllerMessage, DiscoveryMessage, HostMessage, InputEvent};
use vk_media::TileEncoder;
use vk_net::frame::{read_framed, write_framed};
use vk_net::transport::EncryptedStream;
use vk_net::NetError;
use vk_platform::{InputInjector, ScreenCapturer};

use crate::security::{BruteForceGuard, Consent};

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

    // 1. Découverte.
    let probe: DiscoveryMessage = read_framed(&mut stream).await?;
    let matches = matches!(&probe, DiscoveryMessage::Probe { code, .. } if *code == config.code);
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
    host_session(enc, capturer, injector, config).await?;
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

    let mut encoder = TileEncoder::new(config.tile_size, config.quality);
    let period = Duration::from_secs_f64(1.0 / config.fps.max(1) as f64);
    let mut ticker = tokio::time::interval(period);

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if let Some(frame) = capturer.capture()? {
                    let update = encoder.encode(&frame)?;
                    if !update.tiles.is_empty() {
                        enc.send(&HostMessage::Frame(update)).await?;
                    }
                }
            }
            msg = enc.recv::<ControllerMessage>() => {
                match msg? {
                    ControllerMessage::Input(ev) => apply_input(injector.as_mut(), ev)?,
                    ControllerMessage::RequestFullFrame => encoder.force_full_frame(),
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
