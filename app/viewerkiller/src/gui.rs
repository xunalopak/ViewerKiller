//! Interface graphique ViewerKiller (egui/eframe).
//!
//! Trois écrans : accueil (héberger / se connecter), hôte (affiche le code et le
//! mot de passe), et session contrôleur (rendu de l'écran distant + capture
//! clavier/souris). Le réseau tourne sur un runtime tokio en arrière-plan ;
//! l'UI communique avec lui par canaux.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use viewerkiller::{
    controller::discover_and_connect, controller_session, generate_credentials, serve, AutoAccept,
    BruteForceGuard, ControllerConfig, HostConfig, SessionEvent,
};
use vk_core::protocol::{InputEvent, MouseButton, DEFAULT_PORT};
use vk_media::FrameBuffer;

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([960.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "ViewerKiller",
        options,
        Box::new(|_cc| Ok(Box::new(App::new()))),
    )
}

enum Screen {
    Home,
    Host(HostScreen),
    Connect(ConnectForm),
    Connecting,
    Session(SessionScreen),
    Error(String),
}

struct App {
    rt: tokio::runtime::Runtime,
    screen: Screen,
}

impl App {
    fn new() -> Self {
        Self {
            rt: tokio::runtime::Runtime::new().expect("runtime tokio"),
            screen: Screen::Home,
        }
    }
}

// --- Écran hôte ------------------------------------------------------------

struct HostScreen {
    code: String,
    password: String,
    interface: String,
    ip: Ipv4Addr,
}

// --- Formulaire de connexion ----------------------------------------------

#[derive(Default)]
struct ConnectForm {
    code: String,
    password: String,
    subnet: String,
}

// --- Session contrôleur ----------------------------------------------------

struct SessionScreen {
    events_rx: UnboundedReceiver<SessionEvent>,
    input_tx: UnboundedSender<InputEvent>,
    fb: Option<FrameBuffer>,
    texture: Option<egui::TextureHandle>,
    remote_size: Option<(u32, u32)>,
    dirty: bool,
    primary_down: bool,
    secondary_down: bool,
    disconnected: bool,
}

impl SessionScreen {
    fn new(
        events_rx: UnboundedReceiver<SessionEvent>,
        input_tx: UnboundedSender<InputEvent>,
    ) -> Self {
        Self {
            events_rx,
            input_tx,
            fb: None,
            texture: None,
            remote_size: None,
            dirty: false,
            primary_down: false,
            secondary_down: false,
            disconnected: false,
        }
    }

    /// Draine les événements réseau et met à jour le tampon image.
    fn pump(&mut self) {
        while let Ok(event) = self.events_rx.try_recv() {
            match event {
                SessionEvent::ScreenInfo { width, height } => {
                    self.fb = Some(FrameBuffer::new(width, height));
                    self.remote_size = Some((width, height));
                    self.dirty = true;
                }
                SessionEvent::Frame(update) => {
                    if let Some(fb) = self.fb.as_mut() {
                        if fb.apply(&update).is_ok() {
                            self.dirty = true;
                        }
                    }
                }
                SessionEvent::Disconnected => self.disconnected = true,
            }
        }
    }

    fn refresh_texture(&mut self, ctx: &egui::Context) {
        if !self.dirty {
            return;
        }
        if let (Some(fb), Some((w, h))) = (self.fb.as_ref(), self.remote_size) {
            let image =
                egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &fb.rgba);
            match self.texture.as_mut() {
                Some(tex) => tex.set(image, egui::TextureOptions::LINEAR),
                None => {
                    self.texture =
                        Some(ctx.load_texture("ecran-distant", image, egui::TextureOptions::LINEAR))
                }
            }
        }
        self.dirty = false;
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut next: Option<Screen> = None;

        match &mut self.screen {
            Screen::Home => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(120.0);
                        ui.heading("ViewerKiller");
                        ui.label("Contrôle à distance sécurisé sur VPN");
                        ui.add_space(40.0);
                        if ui.button("🖥  Héberger (être contrôlé)").clicked() {
                            next = Some(start_host(&self.rt));
                        }
                        ui.add_space(10.0);
                        if ui.button("🔗  Se connecter (contrôler)").clicked() {
                            next = Some(Screen::Connect(ConnectForm::default()));
                        }
                    });
                });
            }

            Screen::Host(host) => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(80.0);
                        ui.heading("Hôte en écoute");
                        ui.add_space(20.0);
                        ui.label(format!("Interface VPN : {} ({})", host.interface, host.ip));
                        ui.add_space(20.0);
                        ui.label(egui::RichText::new("Code").strong());
                        ui.label(egui::RichText::new(&host.code).size(36.0).monospace());
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new("Mot de passe").strong());
                        ui.label(egui::RichText::new(&host.password).size(24.0).monospace());
                        ui.add_space(30.0);
                        ui.label("Transmettez ces identifiants au contrôleur.");
                        ui.add_space(20.0);
                        if ui.button("Retour").clicked() {
                            next = Some(Screen::Home);
                        }
                    });
                });
            }

            Screen::Connect(form) => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.add_space(60.0);
                    ui.vertical_centered(|ui| {
                        ui.heading("Se connecter à un hôte");
                    });
                    ui.add_space(20.0);
                    egui::Grid::new("form")
                        .num_columns(2)
                        .spacing([12.0, 12.0])
                        .show(ui, |ui| {
                            ui.label("Code");
                            ui.text_edit_singleline(&mut form.code);
                            ui.end_row();
                            ui.label("Mot de passe");
                            ui.add(egui::TextEdit::singleline(&mut form.password).password(true));
                            ui.end_row();
                            ui.label("Sous-réseau (option.)");
                            ui.text_edit_singleline(&mut form.subnet);
                            ui.end_row();
                        });
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(
                            "Sous-réseau vide = détection auto de l'interface VPN.",
                        )
                        .weak(),
                    );
                    ui.add_space(20.0);
                    ui.horizontal(|ui| {
                        if ui.button("Se connecter").clicked() {
                            match start_connect(&self.rt, form) {
                                Ok(screen) => next = Some(screen),
                                Err(e) => next = Some(Screen::Error(e)),
                            }
                        }
                        if ui.button("Retour").clicked() {
                            next = Some(Screen::Home);
                        }
                    });
                });
            }

            Screen::Connecting => {
                // Le résultat arrive via le canal stocké dans CONNECT_RX (ci-dessous).
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(160.0);
                        ui.spinner();
                        ui.label("Recherche de l'hôte sur le VPN…");
                    });
                });
                ctx.request_repaint_after(Duration::from_millis(100));
            }

            Screen::Session(session) => {
                session.pump();
                session.refresh_texture(ctx);
                if session.disconnected {
                    next = Some(Screen::Error("Session terminée.".into()));
                } else {
                    egui::TopBottomPanel::top("barre").show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            if let Some((w, h)) = session.remote_size {
                                ui.label(format!("Écran distant {w}×{h}"));
                            } else {
                                ui.label("Connexion établie…");
                            }
                            if ui.button("Déconnecter").clicked() {
                                next = Some(Screen::Home);
                            }
                        });
                    });
                    egui::CentralPanel::default().show(ctx, |ui| {
                        draw_session(ui, session);
                    });
                    ctx.request_repaint_after(Duration::from_millis(16));
                }
            }

            Screen::Error(msg) => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(140.0);
                        ui.colored_label(egui::Color32::LIGHT_RED, msg.clone());
                        ui.add_space(20.0);
                        if ui.button("Retour à l'accueil").clicked() {
                            next = Some(Screen::Home);
                        }
                    });
                });
            }
        }

        // Transition d'écran « Connecting » : on consulte le canal de résultat.
        if matches!(self.screen, Screen::Connecting) {
            if let Some(result) = poll_connect() {
                next = Some(match result {
                    ConnectResult::Ready {
                        events_rx,
                        input_tx,
                    } => Screen::Session(SessionScreen::new(events_rx, input_tx)),
                    ConnectResult::Failed(e) => Screen::Error(e),
                });
            }
        }

        if let Some(screen) = next {
            self.screen = screen;
        }
    }
}

/// Rendu de l'écran distant + capture des entrées.
fn draw_session(ui: &mut egui::Ui, session: &mut SessionScreen) {
    let Some(texture) = session.texture.as_ref() else {
        ui.centered_and_justified(|ui| {
            ui.label("En attente de la première image…");
        });
        return;
    };
    let Some((rw, rh)) = session.remote_size else {
        return;
    };

    let avail = ui.available_size();
    let sized = egui::load::SizedTexture::from_handle(texture);
    let image = egui::Image::new(sized)
        .fit_to_exact_size(avail)
        .sense(egui::Sense::click_and_drag());
    let response = ui.add(image);
    let rect = response.rect;

    // Position souris → coordonnées écran distant.
    if let Some(pos) = response.hover_pos() {
        if rect.width() > 0.0 && rect.height() > 0.0 {
            let rel_x = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            let rel_y = ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
            let x = (rel_x * rw as f32) as i32;
            let y = (rel_y * rh as f32) as i32;
            let _ = session.input_tx.send(InputEvent::MouseMove { x, y });
        }
    }

    // Boutons et molette.
    let (primary, secondary, scroll, key_events) = ui.input(|i| {
        (
            i.pointer.primary_down(),
            i.pointer.secondary_down(),
            i.raw_scroll_delta,
            i.events.clone(),
        )
    });

    if primary != session.primary_down {
        session.primary_down = primary;
        let _ = session.input_tx.send(InputEvent::MouseButton {
            button: MouseButton::Left,
            pressed: primary,
        });
    }
    if secondary != session.secondary_down {
        session.secondary_down = secondary;
        let _ = session.input_tx.send(InputEvent::MouseButton {
            button: MouseButton::Right,
            pressed: secondary,
        });
    }
    if scroll.y.abs() > 0.5 || scroll.x.abs() > 0.5 {
        let _ = session.input_tx.send(InputEvent::MouseScroll {
            dx: scroll.x as i32,
            dy: scroll.y as i32,
        });
    }

    // Clavier.
    for event in key_events {
        if let egui::Event::Key { key, pressed, .. } = event {
            if let Some(vk) = egui_key_to_vk(key) {
                let _ = session.input_tx.send(InputEvent::Key { key: vk, pressed });
            }
        }
    }
}

// --- Démarrage hôte / connexion -------------------------------------------

fn start_host(rt: &tokio::runtime::Runtime) -> Screen {
    let iface = match vk_net::discovery::guess_wireguard_interface() {
        Some(i) => i,
        None => {
            return Screen::Error(
                "Aucune interface VPN détectée. ViewerKiller ne s'expose que sur le VPN.".into(),
            )
        }
    };
    let (code, password) = generate_credentials();
    let config = HostConfig {
        bind_addr: SocketAddr::new(IpAddr::V4(iface.ip), DEFAULT_PORT),
        code: code.clone(),
        password: password.clone(),
        host_name: hostname(),
        tile_size: vk_media::DEFAULT_TILE_SIZE,
        quality: vk_media::DEFAULT_QUALITY,
        fps: 15,
        require_consent: false,
    };

    rt.spawn(async move {
        let mut guard = BruteForceGuard::new(5, Duration::from_secs(60));
        let mut consent = AutoAccept;
        let mut make_capturer = || vk_platform::default_capturer();
        let mut make_injector = || vk_platform::default_injector();
        if let Err(e) = serve(
            &config,
            &mut make_capturer,
            &mut make_injector,
            &mut guard,
            &mut consent,
        )
        .await
        {
            tracing::error!("hôte arrêté : {e:#}");
        }
    });

    Screen::Host(HostScreen {
        code,
        password,
        interface: iface.name,
        ip: iface.ip,
    })
}

enum ConnectResult {
    Ready {
        events_rx: UnboundedReceiver<SessionEvent>,
        input_tx: UnboundedSender<InputEvent>,
    },
    Failed(String),
}

// Canal global recevant le résultat de la tentative de connexion.
use std::sync::Mutex;
static CONNECT_RX: Mutex<Option<std_mpsc::Receiver<ConnectResult>>> = Mutex::new(None);

fn poll_connect() -> Option<ConnectResult> {
    let guard = CONNECT_RX.lock().unwrap();
    guard.as_ref().and_then(|rx| rx.try_recv().ok())
}

fn start_connect(rt: &tokio::runtime::Runtime, form: &ConnectForm) -> Result<Screen, String> {
    if form.code.trim().is_empty() || form.password.is_empty() {
        return Err("Code et mot de passe requis.".into());
    }
    let subnet = if form.subnet.trim().is_empty() {
        None
    } else {
        Some(parse_subnet(form.subnet.trim()).map_err(|e| e.to_string())?)
    };
    let config = ControllerConfig {
        code: form.code.trim().to_string(),
        password: form.password.clone(),
        port: DEFAULT_PORT,
    };

    let (tx, rx) = std_mpsc::channel();
    *CONNECT_RX.lock().unwrap() = Some(rx);

    rt.spawn(async move {
        match discover_and_connect(subnet, &config, Duration::from_millis(400)).await {
            Ok(enc) => {
                let (events_tx, events_rx) = mpsc::unbounded_channel();
                let (input_tx, input_rx) = mpsc::unbounded_channel();
                tokio::spawn(controller_session(enc, events_tx, input_rx));
                let _ = tx.send(ConnectResult::Ready {
                    events_rx,
                    input_tx,
                });
            }
            Err(e) => {
                let _ = tx.send(ConnectResult::Failed(format!("{e:#}")));
            }
        }
    });

    Ok(Screen::Connecting)
}

// --- Utilitaires -----------------------------------------------------------

fn parse_subnet(s: &str) -> anyhow::Result<(Ipv4Addr, u8)> {
    let (ip, prefix) = s
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("format attendu : ip/prefixe (ex. 10.0.0.0/24)"))?;
    Ok((ip.parse()?, prefix.parse()?))
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "viewerkiller-host".to_string())
}

/// Convertit une touche egui en code de touche virtuelle Windows (VK_*).
fn egui_key_to_vk(key: egui::Key) -> Option<u32> {
    use egui::Key::*;
    let vk = match key {
        A => 0x41,
        B => 0x42,
        C => 0x43,
        D => 0x44,
        E => 0x45,
        F => 0x46,
        G => 0x47,
        H => 0x48,
        I => 0x49,
        J => 0x4A,
        K => 0x4B,
        L => 0x4C,
        M => 0x4D,
        N => 0x4E,
        O => 0x4F,
        P => 0x50,
        Q => 0x51,
        R => 0x52,
        S => 0x53,
        T => 0x54,
        U => 0x55,
        V => 0x56,
        W => 0x57,
        X => 0x58,
        Y => 0x59,
        Z => 0x5A,
        Num0 => 0x30,
        Num1 => 0x31,
        Num2 => 0x32,
        Num3 => 0x33,
        Num4 => 0x34,
        Num5 => 0x35,
        Num6 => 0x36,
        Num7 => 0x37,
        Num8 => 0x38,
        Num9 => 0x39,
        Enter => 0x0D,
        Space => 0x20,
        Backspace => 0x08,
        Tab => 0x09,
        Escape => 0x1B,
        Delete => 0x2E,
        Insert => 0x2D,
        Home => 0x24,
        End => 0x23,
        PageUp => 0x21,
        PageDown => 0x22,
        ArrowLeft => 0x25,
        ArrowUp => 0x26,
        ArrowRight => 0x27,
        ArrowDown => 0x28,
        F1 => 0x70,
        F2 => 0x71,
        F3 => 0x72,
        F4 => 0x73,
        F5 => 0x74,
        F6 => 0x75,
        F7 => 0x76,
        F8 => 0x77,
        F9 => 0x78,
        F10 => 0x79,
        F11 => 0x7A,
        F12 => 0x7B,
        _ => return None,
    };
    Some(vk)
}
