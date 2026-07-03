//! Binaire ViewerKiller (CLI).
//!
//! - `viewerkiller host [ip[:port]]` : démarre l'hôte ; affiche un code + un
//!   mot de passe à transmettre au contrôleur, puis attend une connexion
//!   entrante.
//! - `viewerkiller connect <code> <mot_de_passe> <ip[:port]>` : se connecte
//!   directement à l'hôte qui attend à cette adresse et établit la session.
//!
//! L'interface graphique (rendu de l'écran distant, capture clavier/souris) est
//! ajoutée au jalon 6 ; cette CLI sert à valider la chaîne réseau de bout en
//! bout.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::mpsc;

use viewerkiller::{
    controller::connect_to, controller_session, generate_credentials, serve, AutoAccept,
    BruteForceGuard, ControllerConfig, HostConfig, SessionEvent,
};
use vk_core::protocol::DEFAULT_PORT;
use vk_media::FrameBuffer;

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(String::as_str) {
        Some("host") => {
            let bind_addr = args
                .get(2)
                .map(|s| parse_addr(s, DEFAULT_PORT))
                .transpose()?
                .unwrap_or_else(|| {
                    SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), DEFAULT_PORT)
                });
            run_host(bind_addr).await
        }
        Some("connect") => {
            let code = args
                .get(2)
                .context("usage : viewerkiller connect <code> <mot_de_passe> <ip[:port]>")?;
            let password = args.get(3).context("mot de passe manquant")?;
            let addr = args
                .get(4)
                .context("adresse manquante (ip ou ip:port de l'hôte)")
                .and_then(|s| parse_addr(s, DEFAULT_PORT))?;
            run_connect(code.clone(), password.clone(), addr).await
        }
        _ => {
            eprintln!("Usage :");
            eprintln!("  viewerkiller host [ip[:port]]");
            eprintln!("  viewerkiller connect <code> <mot_de_passe> <ip[:port]>");
            std::process::exit(2);
        }
    }
}

async fn run_host(bind_addr: SocketAddr) -> Result<()> {
    let (code, password) = generate_credentials();

    println!("======================================");
    println!("  ViewerKiller — hôte prêt");
    println!("  Écoute        : {bind_addr}");
    println!("  Code          : {code}");
    println!("  Mot de passe  : {password}");
    println!("======================================");

    let config = HostConfig {
        bind_addr,
        code,
        password,
        host_name: hostname(),
        tile_size: vk_media::DEFAULT_TILE_SIZE,
        quality: vk_media::DEFAULT_QUALITY,
        fps: 15,
        require_consent: false,
    };

    let mut make_capturer = || vk_platform::default_capturer();
    let mut make_injector = || vk_platform::default_injector();
    let mut guard = BruteForceGuard::new(5, Duration::from_secs(60));
    let mut consent = AutoAccept;
    serve(
        &config,
        &mut make_capturer,
        &mut make_injector,
        &mut guard,
        &mut consent,
    )
    .await
}

async fn run_connect(code: String, password: String, addr: SocketAddr) -> Result<()> {
    let config = ControllerConfig {
        code,
        password,
        port: addr.port(),
    };
    let enc = connect_to(addr, &config).await?;

    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
    let (_input_tx, input_rx) = mpsc::unbounded_channel();
    let session = tokio::spawn(controller_session(enc, events_tx, input_rx));

    let mut fb: Option<FrameBuffer> = None;
    let mut frames = 0u64;
    while let Some(event) = events_rx.recv().await {
        match event {
            SessionEvent::ScreenInfo { width, height } => {
                tracing::info!(width, height, "écran distant");
                fb = Some(FrameBuffer::new(width, height));
            }
            SessionEvent::Frame(update) => {
                if let Some(fb) = fb.as_mut() {
                    fb.apply(&update)?;
                }
                frames += 1;
                if frames % 15 == 0 {
                    tracing::info!(frames, "trames reçues");
                }
            }
            SessionEvent::Disconnected => {
                tracing::info!("session terminée");
                break;
            }
        }
    }
    let _ = session.await;
    Ok(())
}

/// Parse `ip` ou `ip:port` ; utilise `default_port` si le port est omis.
fn parse_addr(s: &str, default_port: u16) -> Result<SocketAddr> {
    if let Ok(addr) = s.parse::<SocketAddr>() {
        return Ok(addr);
    }
    let ip: IpAddr = s
        .parse()
        .context("format attendu : ip ou ip:port (ex. 10.0.0.5 ou 10.0.0.5:47600)")?;
    Ok(SocketAddr::new(ip, default_port))
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "viewerkiller-host".to_string())
}
