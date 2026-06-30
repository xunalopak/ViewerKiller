//! Tests d'intégration du durcissement : anti-bruteforce sur le mot de passe et
//! refus par consentement, sur boucle TCP locale.

use std::time::Duration;

use tokio::net::TcpListener;

use viewerkiller::{
    AutoAccept, BruteForceGuard, ConnectionOutcome, ControllerConfig, HostConfig, RejectAll,
};

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
    }
}

fn capturer() -> Box<dyn vk_platform::ScreenCapturer> {
    vk_platform::default_capturer().unwrap()
}
fn injector() -> Box<dyn vk_platform::InputInjector> {
    vk_platform::default_injector().unwrap()
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
