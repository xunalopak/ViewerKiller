//! Binaire ViewerKiller (CLI).
//!
//! - `viewerkiller host` : démarre l'hôte ; affiche un code + un mot de passe à
//!   transmettre au contrôleur. Écoute uniquement sur l'interface VPN.
//! - `viewerkiller connect <code> <mot_de_passe> [sous-réseau]` : découvre
//!   l'hôte sur le VPN et établit la session.
//!
//! L'interface graphique (rendu de l'écran distant, capture clavier/souris) est
//! ajoutée au jalon 6 ; cette CLI sert à valider la chaîne réseau de bout en
//! bout.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::mpsc;

use viewerkiller::{
    controller_session, generate_credentials, serve, AutoAccept, BruteForceGuard, ControllerConfig,
    HostConfig, SessionEvent,
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
        Some("host") => run_host().await,
        Some("connect") => {
            let code = args
                .get(2)
                .context("usage : viewerkiller connect <code> <mot_de_passe> [sous-réseau]")?;
            let password = args.get(3).context("mot de passe manquant")?;
            let subnet = args.get(4).map(|s| parse_subnet(s)).transpose()?;
            run_connect(code.clone(), password.clone(), subnet).await
        }
        _ => {
            eprintln!("Usage :");
            eprintln!("  viewerkiller host");
            eprintln!("  viewerkiller connect <code> <mot_de_passe> [ip/prefixe]");
            std::process::exit(2);
        }
    }
}

async fn run_host() -> Result<()> {
    let iface = vk_net::discovery::guess_wireguard_interface()
        .context("aucune interface VPN détectée ; ViewerKiller ne s'expose que sur le VPN")?;
    let bind_addr = SocketAddr::new(IpAddr::V4(iface.ip), DEFAULT_PORT);
    let (code, password) = generate_credentials();

    println!("======================================");
    println!("  ViewerKiller — hôte prêt");
    println!("  Interface VPN : {} ({})", iface.name, iface.ip);
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

async fn run_connect(code: String, password: String, subnet: Option<(Ipv4Addr, u8)>) -> Result<()> {
    let config = ControllerConfig {
        code,
        password,
        port: DEFAULT_PORT,
    };
    let enc =
        viewerkiller::controller::discover_and_connect(subnet, &config, Duration::from_millis(400))
            .await?;

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

fn parse_subnet(s: &str) -> Result<(Ipv4Addr, u8)> {
    let (ip, prefix) = s
        .split_once('/')
        .context("format attendu : ip/prefixe (ex. 10.0.0.0/24)")?;
    Ok((ip.parse()?, prefix.parse()?))
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "viewerkiller-host".to_string())
}
