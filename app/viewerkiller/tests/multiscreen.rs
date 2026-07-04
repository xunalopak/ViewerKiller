//! Test d'intégration du multi-écrans (J12) : l'hôte annonce ses moniteurs, et
//! une demande de bascule du contrôleur change la géométrie diffusée.
//!
//! Toute la logique (annonce, sélection, nouvelle géométrie) est validée ici via
//! un capteur factice à deux moniteurs ; seule l'énumération Windows réelle
//! (`EnumDisplayMonitors`) reste à valider au runtime.

use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::mpsc;

use viewerkiller::{
    run_controller, AutoAccept, BruteForceGuard, ControllerConfig, HostConfig, ReconnectPolicy,
    SessionEvent,
};
use vk_core::protocol::{MonitorInfo, MouseButton};
use vk_platform::{Frame, InputInjector, ScreenCapturer};

/// Capteur factice à plusieurs moniteurs ; `capture` renvoie une trame noire de
/// la taille du moniteur sélectionné.
struct MultiMonitorStub {
    selected: usize,
    sizes: Vec<(u32, u32)>,
}

impl ScreenCapturer for MultiMonitorStub {
    fn dimensions(&self) -> (u32, u32) {
        self.sizes[self.selected]
    }
    fn capture(&mut self) -> anyhow::Result<Option<Frame>> {
        let (width, height) = self.sizes[self.selected];
        Ok(Some(Frame {
            width,
            height,
            data: vec![0u8; (width * height * 4) as usize],
        }))
    }
    fn monitors(&self) -> Vec<MonitorInfo> {
        self.sizes
            .iter()
            .enumerate()
            .map(|(i, &(width, height))| MonitorInfo {
                index: i as u32,
                width,
                height,
                primary: i == 0,
            })
            .collect()
    }
    fn select_monitor(&mut self, index: u32) -> anyhow::Result<()> {
        let i = index as usize;
        if i >= self.sizes.len() {
            anyhow::bail!("moniteur {index} inexistant");
        }
        self.selected = i;
        Ok(())
    }
}

/// Injecteur qui ignore tout (la session ne teste pas les entrées ici).
struct NullInjector;
impl InputInjector for NullInjector {
    fn mouse_move(&mut self, _x: i32, _y: i32) -> anyhow::Result<()> {
        Ok(())
    }
    fn mouse_button(&mut self, _b: MouseButton, _p: bool) -> anyhow::Result<()> {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn controller_can_switch_monitor() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let config = HostConfig {
        bind_addr: addr,
        code: "424242".into(),
        password: "mot-de-passe-fort".into(),
        host_name: "hote".into(),
        tile_size: 64,
        quality: 75,
        fps: 60,
        require_consent: false,
        share_clipboard: false,
    };

    let host = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let capturer: Box<dyn ScreenCapturer> = Box::new(MultiMonitorStub {
            selected: 0,
            sizes: vec![(320, 240), (640, 480)],
        });
        let injector: Box<dyn InputInjector> = Box::new(NullInjector);
        let mut guard = BruteForceGuard::new(5, Duration::from_secs(60));
        let mut consent = AutoAccept;
        let _ = viewerkiller::handle_connection(
            stream,
            &config,
            &mut guard,
            &mut consent,
            capturer,
            injector,
        )
        .await;
    });

    let cfg = ControllerConfig {
        code: "424242".into(),
        password: "mot-de-passe-fort".into(),
        port: addr.port(),
    };
    let enc = viewerkiller::controller::connect_to(addr, &cfg)
        .await
        .unwrap();

    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (monitor_tx, monitor_rx) = mpsc::unbounded_channel();
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

    // Annonce des 2 moniteurs + géométrie initiale (320×240, moniteur 0).
    let mut monitors: Option<Vec<MonitorInfo>> = None;
    let mut got_initial = false;
    tokio::time::timeout(Duration::from_secs(10), async {
        while monitors.is_none() || !got_initial {
            match events_rx.recv().await.unwrap() {
                SessionEvent::Monitors(list) => monitors = Some(list),
                SessionEvent::ScreenInfo {
                    width: 320,
                    height: 240,
                } => got_initial = true,
                _ => {}
            }
        }
    })
    .await
    .expect("annonce moniteurs / géométrie initiale non reçue");
    assert_eq!(monitors.unwrap().len(), 2, "deux moniteurs attendus");

    // Bascule vers le moniteur 1 → nouvelle géométrie 640×480.
    monitor_tx.send(1).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let SessionEvent::ScreenInfo {
                width: 640,
                height: 480,
            } = events_rx.recv().await.unwrap()
            {
                break;
            }
        }
    })
    .await
    .expect("nouvelle géométrie après bascule de moniteur non reçue");

    drop(input_tx);
    drop(monitor_tx);
    let _ = session.await;
    host.abort();
}
