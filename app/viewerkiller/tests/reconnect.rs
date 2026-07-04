//! Test d'intégration de la reconnexion automatique côté contrôleur (J13).
//!
//! Un hôte minimal (handshake manuel) accepte deux connexions : il coupe la
//! première juste après avoir envoyé la géométrie de l'écran, puis sert
//! normalement la seconde. On vérifie que `run_controller` détecte la coupure,
//! émet [`SessionEvent::Reconnecting`], se reconnecte tout seul et reprend la
//! session (nouvelle géométrie reçue) — le tout sans réauthentification par
//! l'utilisateur.

use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use viewerkiller::{run_controller, ControllerConfig, ReconnectPolicy, SessionEvent};
use vk_core::crypto::derive_psk;
use vk_core::protocol::{ControllerMessage, DiscoveryMessage, HostMessage};
use vk_net::frame::{read_framed, write_framed};
use vk_net::transport::EncryptedStream;

/// Répond à la sonde de vérification de code (phase en clair) par un succès.
async fn accept_probe(sock: &mut TcpStream, code: &str) {
    let probe: DiscoveryMessage = read_framed(sock).await.unwrap();
    let matches = matches!(&probe, DiscoveryMessage::Probe { code: c, .. } if c == code);
    write_framed(
        sock,
        &DiscoveryMessage::ProbeResult {
            matches,
            host_name: "hote-reco".into(),
        },
    )
    .await
    .unwrap();
    assert!(matches, "code refusé lors d'une (re)connexion");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn controller_reconnects_after_drop() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let psk = derive_psk("mot-de-passe-fort");
    let code = "424242".to_string();

    let host_code = code.clone();
    let host = tokio::spawn(async move {
        // 1re connexion : on envoie une géométrie « 100×80 » puis on coupe.
        let (mut sock, _) = listener.accept().await.unwrap();
        accept_probe(&mut sock, &host_code).await;
        let mut enc = EncryptedStream::accept(sock, &psk).await.unwrap();
        enc.send(&HostMessage::ScreenInfo {
            width: 100,
            height: 80,
        })
        .await
        .unwrap();
        // Laisse le contrôleur recevoir la géométrie avant de couper : sinon, sur
        // certaines piles TCP (Windows), le RST de fermeture purge le tampon de
        // réception et ce message serait perdu (course tolérée sous Linux).
        tokio::time::sleep(Duration::from_millis(300)).await;
        drop(enc); // simule une coupure réseau (VPN qui tombe)

        // 2e connexion (reconnexion) : géométrie « 200×160 », puis on attend le
        // Bye de fin de test.
        let (mut sock, _) = listener.accept().await.unwrap();
        accept_probe(&mut sock, &host_code).await;
        let mut enc = EncryptedStream::accept(sock, &psk).await.unwrap();
        enc.send(&HostMessage::ScreenInfo {
            width: 200,
            height: 160,
        })
        .await
        .unwrap();
        loop {
            match enc.recv::<ControllerMessage>().await {
                Ok(ControllerMessage::Bye) | Err(_) => break,
                Ok(_) => {}
            }
        }
    });

    let cfg = ControllerConfig {
        code,
        password: "mot-de-passe-fort".into(),
        port: addr.port(),
    };
    let enc = viewerkiller::controller::connect_to(addr, &cfg)
        .await
        .unwrap();

    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    // Backoff court pour un test rapide ; reconnexion activée.
    let policy = ReconnectPolicy {
        enabled: true,
        max_attempts: 40,
        initial_backoff: Duration::from_millis(30),
        max_backoff: Duration::from_millis(120),
    };
    let session = tokio::spawn(run_controller(
        enc, addr, cfg, events_tx, input_rx, false, policy,
    ));

    // Séquence attendue : ScreenInfo(100) → Reconnecting → ScreenInfo(200).
    let mut saw_first = false;
    let mut saw_reconnecting = false;
    let mut saw_second = false;
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(ev) = events_rx.recv().await {
            match ev {
                SessionEvent::ScreenInfo { width: 100, .. } => saw_first = true,
                SessionEvent::Reconnecting => saw_reconnecting = true,
                SessionEvent::ScreenInfo { width: 200, .. } => {
                    saw_second = true;
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("timeout : la reconnexion automatique n'a pas abouti");

    assert!(saw_first, "1re géométrie non reçue");
    assert!(saw_reconnecting, "événement Reconnecting non émis");
    assert!(saw_second, "géométrie après reconnexion non reçue");

    // Fin propre : fermer le canal d'entrée termine la session (Bye envoyé).
    drop(input_tx);
    let _ = session.await;
    host.await.unwrap();
}
