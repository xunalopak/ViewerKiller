//! Tests d'intégration du durcissement : anti-bruteforce sur le mot de passe et
//! refus par consentement, sur boucle TCP locale.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::net::TcpListener;

use viewerkiller::{
    AutoAccept, BruteForceGuard, ConnectionOutcome, ControllerConfig, HostConfig, RejectAll,
};
use vk_core::protocol::{ControllerMessage, HostMessage};
use vk_platform::{Frame, InputInjector, ScreenCapturer};

fn host_config(addr: std::net::SocketAddr, require_consent: bool) -> HostConfig {
    HostConfig {
        bind_addr: addr,
        code: "111222".into(),
        password: "bon-mot-de-passe".into(),
        host_name: "hote".into(),
        tile_size: 64,
        quality: 75,
        fps: 60,
        require_consent,
        share_clipboard: false,
    }
}

// Stubs neutres, indépendants de la plateforme (la capture n'est jamais atteinte
// dans ces scénarios : verrouillage / mauvais mot de passe / refus).
struct NullCapturer;
impl ScreenCapturer for NullCapturer {
    fn dimensions(&self) -> (u32, u32) {
        (1, 1)
    }
    fn capture(&mut self) -> anyhow::Result<Option<Frame>> {
        Ok(None)
    }
}

struct NullInjector;
impl InputInjector for NullInjector {
    fn mouse_move(&mut self, _x: i32, _y: i32) -> anyhow::Result<()> {
        Ok(())
    }
    fn mouse_button(
        &mut self,
        _button: vk_core::protocol::MouseButton,
        _pressed: bool,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    fn mouse_scroll(&mut self, _dx: i32, _dy: i32) -> anyhow::Result<()> {
        Ok(())
    }
    fn key(&mut self, _key: u32, _pressed: bool) -> anyhow::Result<()> {
        Ok(())
    }
    fn char_input(&mut self, _c: char) -> anyhow::Result<()> {
        Ok(())
    }
}

fn capturer() -> Box<dyn ScreenCapturer> {
    Box::new(NullCapturer)
}
fn injector() -> Box<dyn InputInjector> {
    Box::new(NullInjector)
}

#[tokio::test]
async fn wrong_password_counts_and_locks_out() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let config = host_config(addr, false);

    let mut guard = BruteForceGuard::new(2, Duration::from_secs(60));
    let mut consent = AutoAccept;

    // Deux tentatives avec un mauvais mot de passe → AuthFailed.
    for _ in 0..2 {
        let cfg = ControllerConfig {
            code: "111222".into(),
            password: "MAUVAIS".into(),
            port: addr.port(),
        };
        let ctrl = tokio::spawn(async move {
            let _ = viewerkiller::controller::connect_to(addr, &cfg).await;
        });

        let (stream, _) = listener.accept().await.unwrap();
        let outcome = viewerkiller::handle_connection(
            stream,
            &config,
            &mut guard,
            &mut consent,
            capturer(),
            injector(),
        )
        .await
        .unwrap();
        assert_eq!(outcome, ConnectionOutcome::AuthFailed);
        let _ = ctrl.await;
    }

    // L'IP est désormais verrouillée : même le bon mot de passe est refusé.
    let cfg = ControllerConfig {
        code: "111222".into(),
        password: "bon-mot-de-passe".into(),
        port: addr.port(),
    };
    let ctrl = tokio::spawn(async move {
        let _ = viewerkiller::controller::connect_to(addr, &cfg).await;
    });
    let (stream, _) = listener.accept().await.unwrap();
    let outcome = viewerkiller::handle_connection(
        stream,
        &config,
        &mut guard,
        &mut consent,
        capturer(),
        injector(),
    )
    .await
    .unwrap();
    assert_eq!(outcome, ConnectionOutcome::Locked);
    ctrl.abort();
}

#[tokio::test]
async fn consent_refusal_blocks_session() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let config = host_config(addr, true); // consentement requis

    let mut guard = BruteForceGuard::new(5, Duration::from_secs(60));
    let mut consent = RejectAll;

    let cfg = ControllerConfig {
        code: "111222".into(),
        password: "bon-mot-de-passe".into(),
        port: addr.port(),
    };
    // Le contrôleur s'authentifie avec succès puis reçoit Bye (refus).
    let ctrl = tokio::spawn(async move {
        let _ = viewerkiller::controller::connect_to(addr, &cfg).await;
    });

    let (stream, _) = listener.accept().await.unwrap();
    let outcome = viewerkiller::handle_connection(
        stream,
        &config,
        &mut guard,
        &mut consent,
        capturer(),
        injector(),
    )
    .await
    .unwrap();
    assert_eq!(outcome, ConnectionOutcome::Refused);
    let _ = ctrl.await;
}

/// Journal des pairs dont la session s'est terminée.
#[derive(Clone, Default)]
struct EndLog(Arc<Mutex<Vec<SocketAddr>>>);

/// Consentement qui accepte et enregistre chaque fin de session.
struct AcceptAndLog {
    log: EndLog,
}
impl viewerkiller::Consent for AcceptAndLog {
    fn request(&mut self, _peer: SocketAddr) -> viewerkiller::ConsentFuture {
        Box::pin(async { true })
    }
    fn session_ended(&mut self, peer: SocketAddr) {
        self.log.0.lock().unwrap().push(peer);
    }
}

#[tokio::test]
async fn accepted_session_reports_end() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let config = host_config(addr, true);

    let mut guard = BruteForceGuard::new(5, Duration::from_secs(60));
    let log = EndLog::default();
    let mut consent = AcceptAndLog { log: log.clone() };

    let cfg = ControllerConfig {
        code: "111222".into(),
        password: "bon-mot-de-passe".into(),
        port: addr.port(),
    };
    // Le contrôleur s'authentifie, reçoit la géométrie, puis se déconnecte.
    let ctrl = tokio::spawn(async move {
        let mut enc = viewerkiller::controller::connect_to(addr, &cfg)
            .await
            .unwrap();
        let _: HostMessage = enc.recv().await.unwrap(); // ScreenInfo
        enc.send(&ControllerMessage::Bye).await.unwrap();
    });

    let (stream, _) = listener.accept().await.unwrap();
    let outcome = viewerkiller::handle_connection(
        stream,
        &config,
        &mut guard,
        &mut consent,
        capturer(),
        injector(),
    )
    .await
    .unwrap();
    assert_eq!(outcome, ConnectionOutcome::Completed);
    ctrl.await.unwrap();

    // La fin de session a bien été signalée une fois.
    assert_eq!(log.0.lock().unwrap().len(), 1);
}
