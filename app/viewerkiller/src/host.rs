//! Côté hôte : écoute sur l'interface VPN, répond aux sondes de découverte,
//! établit la session chiffrée, puis diffuse l'écran et injecte les entrées.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::net::{TcpListener, TcpStream};

use vk_core::crypto::derive_psk;
use vk_core::protocol::{ControllerMessage, DiscoveryMessage, HostMessage, InputEvent};
use vk_media::TileEncoder;
use vk_net::frame::{read_framed, write_framed};
use vk_net::transport::EncryptedStream;
use vk_platform::{InputInjector, ScreenCapturer};

/// Configuration d'un hôte.
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// Adresse d'écoute — **doit** être l'IP de l'interface VPN (jamais 0.0.0.0).
    pub bind_addr: SocketAddr,
    pub code: String,
    pub password: String,
    pub host_name: String,
    pub tile_size: u32,
    pub quality: u8,
    pub fps: u32,
}

/// Construit un capteur/injecteur à la demande (une instance par session).
pub type CapturerFactory = dyn FnMut() -> Result<Box<dyn ScreenCapturer>> + Send;
pub type InjectorFactory = dyn FnMut() -> Result<Box<dyn InputInjector>> + Send;

/// Boucle d'écoute : sert les contrôleurs un à un (MVP mono-session).
pub async fn serve(
    config: &HostConfig,
    make_capturer: &mut CapturerFactory,
    make_injector: &mut InjectorFactory,
) -> Result<()> {
    let listener = TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("liaison impossible sur {}", config.bind_addr))?;
    tracing::info!(addr = %config.bind_addr, code = %config.code, "hôte en écoute");

    loop {
        let (stream, peer) = listener.accept().await?;
        tracing::debug!(%peer, "connexion entrante");
        let capturer = make_capturer()?;
        let injector = make_injector()?;
        if let Err(e) = handle_connection(stream, config, capturer, injector).await {
            tracing::debug!("session/sonde terminée : {e:#}");
        }
    }
}

/// Traite une connexion entrante : découverte → handshake → session.
pub async fn handle_connection(
    mut stream: TcpStream,
    config: &HostConfig,
    capturer: Box<dyn ScreenCapturer>,
    injector: Box<dyn InputInjector>,
) -> Result<()> {
    // 1. Découverte : on lit la sonde et on répond.
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
        return Ok(());
    }

    // 2. Handshake Noise authentifié par le mot de passe.
    let psk = derive_psk(&config.password);
    let enc = EncryptedStream::accept(stream, &psk)
        .await
        .context("handshake Noise (mot de passe incorrect ?)")?;
    tracing::info!("session chiffrée établie");

    // 3. Session.
    host_session(enc, capturer, injector, config).await
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
    }
}
