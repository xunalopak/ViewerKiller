//! Test de bout en bout headless : un hôte (capteur factice + injecteur
//! enregistreur) et un contrôleur communiquent sur une boucle TCP locale.
//!
//! Valide toute la chaîne sans matériel Windows ni affichage : découverte,
//! handshake Noise, diffusion écran (capture → encode → réseau → décode →
//! tampon RGBA) et remontée des entrées (réseau → injection).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::mpsc;

use viewerkiller::{run_controller, ControllerConfig, HostConfig, ReconnectPolicy, SessionEvent};
use vk_core::protocol::InputEvent;
use vk_media::FrameBuffer;
use vk_platform::{Frame, InputInjector, ScreenCapturer};

/// Capteur factice : produit une trame 320x240 dont tout le contenu change à
/// chaque appel (donc toutes les tuiles sont ré-émises).
struct ChangingStub {
    width: u32,
    height: u32,
    tick: u8,
}

impl ScreenCapturer for ChangingStub {
    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    fn capture(&mut self) -> anyhow::Result<Option<Frame>> {
        self.tick = self.tick.wrapping_add(40);
        let data = vec![self.tick; (self.width * self.height * 4) as usize];
        Ok(Some(Frame {
            width: self.width,
            height: self.height,
            data,
        }))
    }
}

/// Injecteur enregistreur : mémorise les événements reçus.
struct RecordingInjector(Arc<Mutex<Vec<InputEvent>>>);

impl InputInjector for RecordingInjector {
    fn mouse_move(&mut self, x: i32, y: i32) -> anyhow::Result<()> {
        self.0.lock().unwrap().push(InputEvent::MouseMove { x, y });
        Ok(())
    }
    fn mouse_button(
        &mut self,
        button: vk_core::protocol::MouseButton,
        pressed: bool,
    ) -> anyhow::Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(InputEvent::MouseButton { button, pressed });
        Ok(())
    }
    fn mouse_scroll(&mut self, dx: i32, dy: i32) -> anyhow::Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(InputEvent::MouseScroll { dx, dy });
        Ok(())
    }
    fn key(&mut self, key: u32, pressed: bool) -> anyhow::Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(InputEvent::Key { key, pressed });
        Ok(())
    }
    fn char_input(&mut self, c: char) -> anyhow::Result<()> {
        self.0.lock().unwrap().push(InputEvent::Char { c });
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_pipeline_screen_and_input() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let config = HostConfig {
        bind_addr: addr,
        code: "424242".into(),
        password: "motdepasse-fort".into(),
        host_name: "hote-test".into(),
        tile_size: 64,
        quality: 75,
        fps: 60,
        require_consent: false,
        share_clipboard: false,
    };

    let recorded = Arc::new(Mutex::new(Vec::new()));
    let recorded_host = recorded.clone();

    let host = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let capturer: Box<dyn ScreenCapturer> = Box::new(ChangingStub {
            width: 320,
            height: 240,
            tick: 0,
        });
        let injector: Box<dyn InputInjector> = Box::new(RecordingInjector(recorded_host));
        let mut guard = viewerkiller::BruteForceGuard::new(5, Duration::from_secs(60));
        let mut consent = viewerkiller::AutoAccept;
        viewerkiller::handle_connection(
            stream,
            &config,
            &mut guard,
            &mut consent,
            capturer,
            injector,
        )
        .await
        .unwrap();
    });

    let cfg = ControllerConfig {
        code: "424242".into(),
        password: "motdepasse-fort".into(),
        port: addr.port(),
    };
    let enc = viewerkiller::controller::connect_to(addr, &cfg)
        .await
        .unwrap();

    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (_monitor_tx, monitor_rx) = mpsc::unbounded_channel();
    let session = tokio::spawn(run_controller(
        enc,
        addr,
        cfg,
        events_tx,
        input_rx,
        monitor_rx,
        false,
        ReconnectPolicy::disabled(),
    ));

    // 1. Géométrie de l'écran.
    let mut fb = match events_rx.recv().await.unwrap() {
        SessionEvent::ScreenInfo { width, height } => FrameBuffer::new(width, height),
        other => panic!("attendu ScreenInfo, reçu {other:?}"),
    };
    assert_eq!((fb.width, fb.height), (320, 240));

    // 2. Trois trames appliquées avec succès au tampon RGBA.
    let mut frames = 0;
    while frames < 3 {
        match events_rx.recv().await.unwrap() {
            SessionEvent::Frame(update) => {
                fb.apply(&update).unwrap();
                frames += 1;
            }
            SessionEvent::ScreenInfo { .. } => {}
            SessionEvent::Monitors(_) => {}
            SessionEvent::Cursor { .. } => {}
            SessionEvent::Reconnecting => {}
            SessionEvent::Disconnected => panic!("déconnexion prématurée"),
        }
    }

    // 3. Une entrée souris et un caractère Unicode sont bien injectés côté
    //    hôte (attente active ≤ 2 s).
    input_tx
        .send(InputEvent::MouseMove { x: 100, y: 50 })
        .unwrap();
    input_tx.send(InputEvent::Char { c: 'é' }).unwrap();
    let mut injected = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let recorded = recorded.lock().unwrap();
        let mouse = recorded
            .iter()
            .any(|e| matches!(e, InputEvent::MouseMove { x: 100, y: 50 }));
        let text = recorded
            .iter()
            .any(|e| matches!(e, InputEvent::Char { c: 'é' }));
        if mouse && text {
            injected = true;
            break;
        }
    }
    assert!(
        injected,
        "entrées non injectées : {:?}",
        recorded.lock().unwrap()
    );

    // 4. Fermeture propre.
    drop(input_tx);
    let _ = session.await;
    host.abort();
}
