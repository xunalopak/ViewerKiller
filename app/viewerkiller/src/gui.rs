// Pas de console Windows à côté de la fenêtre (les logs tracing deviennent
// invisibles en GUI ; utiliser la CLI pour diagnostiquer).
#![cfg_attr(windows, windows_subsystem = "windows")]

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
use tokio::sync::oneshot;

use viewerkiller::{
    controller::connect_to, generate_credentials, local_ipv4_addresses, run_controller, serve,
    BruteForceGuard, ControllerConfig, HostConfig, ReconnectPolicy, SessionEvent,
};
use vk_core::protocol::{CursorKind, InputEvent, MouseButton, DEFAULT_PORT};
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
    /// Tâche `serve` de l'hôte en cours, pour pouvoir l'arrêter au retour à
    /// l'accueil (sinon l'ancien listener — et son ancien code — survivrait).
    host_task: Option<tokio::task::JoinHandle<()>>,
    /// Résultat (unique) de la vérification de mise à jour en arrière-plan (J16a).
    update_rx: std_mpsc::Receiver<viewerkiller::update::UpdateInfo>,
    /// Nouvelle version disponible, une fois la vérification aboutie.
    update_info: Option<viewerkiller::update::UpdateInfo>,
    /// Réglages d'hébergement (modifiables sur l'accueil avant de démarrer).
    host_fps: u32,
    host_quality: u8,
    /// Hook clavier système (Alt+Tab, touche Windows…) ; no-op hors Windows.
    system_hook: Box<dyn vk_platform::SystemKeyHook>,
}

impl App {
    fn new() -> Self {
        // Vérification de version sur un thread dédié (ureq est bloquant) ;
        // silencieuse hors ligne, ne retarde pas l'ouverture de la fenêtre.
        let (update_tx, update_rx) = std_mpsc::channel();
        std::thread::spawn(move || {
            if let Some(info) = viewerkiller::update::check_latest() {
                let _ = update_tx.send(info);
            }
        });
        Self {
            rt: tokio::runtime::Runtime::new().expect("runtime tokio"),
            screen: Screen::Home,
            host_task: None,
            update_rx,
            update_info: None,
            host_fps: 15,
            host_quality: vk_media::DEFAULT_QUALITY,
            system_hook: vk_platform::default_system_key_hook(),
        }
    }
}

// --- Écran hôte ------------------------------------------------------------

struct HostScreen {
    code: String,
    password: String,
    bind_addr: SocketAddr,
    /// Adresses IPv4 locales (Wi-Fi, Ethernet…) à communiquer au contrôleur.
    addresses: Vec<(String, Ipv4Addr)>,
    /// Demandes de consentement et fins de session en provenance du fil réseau.
    consent_rx: UnboundedReceiver<ConsentMsg>,
    /// Demande en attente d'une décision de l'utilisateur.
    pending: Option<PendingConsent>,
    /// Pair actuellement connecté (bannière « session en cours »).
    active: Option<SocketAddr>,
}

/// Une demande de connexion en attente de la décision de l'utilisateur.
struct PendingConsent {
    peer: SocketAddr,
    reply: oneshot::Sender<bool>,
}

/// Message du fil réseau (hôte) vers l'écran hôte.
enum ConsentMsg {
    /// Un contrôleur authentifié demande la main ; répondre via `reply`.
    Request {
        peer: SocketAddr,
        reply: oneshot::Sender<bool>,
    },
    /// La session avec `peer` s'est terminée.
    Ended { peer: SocketAddr },
}

/// Impl [`viewerkiller::Consent`] branchée sur l'UI egui : chaque demande est
/// transmise à l'écran hôte, qui répond via un canal oneshot. Sans réponse sous
/// 30 s (ou si l'UI a disparu), la connexion est refusée.
struct GuiConsent {
    tx: UnboundedSender<ConsentMsg>,
}

impl viewerkiller::Consent for GuiConsent {
    fn request(&mut self, peer: SocketAddr) -> viewerkiller::ConsentFuture {
        let (reply_tx, reply_rx) = oneshot::channel();
        let queued = self
            .tx
            .send(ConsentMsg::Request {
                peer,
                reply: reply_tx,
            })
            .is_ok();
        Box::pin(async move {
            if !queued {
                return false;
            }
            matches!(
                tokio::time::timeout(Duration::from_secs(30), reply_rx).await,
                Ok(Ok(true))
            )
        })
    }

    fn session_ended(&mut self, peer: SocketAddr) {
        let _ = self.tx.send(ConsentMsg::Ended { peer });
    }
}

// --- Formulaire de connexion ----------------------------------------------

#[derive(Default)]
struct ConnectForm {
    code: String,
    password: String,
    address: String,
}

// --- Session contrôleur ----------------------------------------------------

struct SessionScreen {
    events_rx: UnboundedReceiver<SessionEvent>,
    input_tx: UnboundedSender<InputEvent>,
    /// Envoi d'un index de moniteur à basculer (multi-écrans, J12).
    monitor_tx: UnboundedSender<u32>,
    /// Moniteurs annoncés par l'hôte (vide ou 1 seul = pas de sélecteur).
    monitors: Vec<vk_core::protocol::MonitorInfo>,
    fb: Option<FrameBuffer>,
    texture: Option<egui::TextureHandle>,
    remote_size: Option<(u32, u32)>,
    dirty: bool,
    primary_down: bool,
    secondary_down: bool,
    /// Dernier état des modificateurs envoyé à l'hôte (suivi par transitions).
    mods: egui::Modifiers,
    disconnected: bool,
    /// Connexion perdue, reconnexion automatique en cours (bannière).
    reconnecting: bool,
    /// Type de curseur de l'hôte (curseur distant, J12) : le curseur local du
    /// contrôleur s'y adapte au survol de l'image.
    cursor_kind: CursorKind,
    cursor_visible: bool,
}

impl SessionScreen {
    fn new(
        events_rx: UnboundedReceiver<SessionEvent>,
        input_tx: UnboundedSender<InputEvent>,
        monitor_tx: UnboundedSender<u32>,
    ) -> Self {
        Self {
            events_rx,
            input_tx,
            monitor_tx,
            monitors: Vec::new(),
            fb: None,
            texture: None,
            remote_size: None,
            dirty: false,
            primary_down: false,
            secondary_down: false,
            mods: egui::Modifiers::default(),
            disconnected: false,
            reconnecting: false,
            cursor_kind: CursorKind::Default,
            cursor_visible: true,
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
                    self.reconnecting = false; // une reconnexion vient d'aboutir
                }
                SessionEvent::Frame(update) => {
                    if let Some(fb) = self.fb.as_mut() {
                        if fb.apply(&update).is_ok() {
                            self.dirty = true;
                        }
                    }
                }
                SessionEvent::Monitors(list) => self.monitors = list,
                SessionEvent::Cursor { kind, visible } => {
                    self.cursor_kind = kind;
                    self.cursor_visible = visible;
                }
                SessionEvent::Reconnecting => self.reconnecting = true,
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
        let mut new_host_task: Option<tokio::task::JoinHandle<()>> = None;

        // Hook clavier système : capture active UNIQUEMENT en session ET fenêtre au
        // premier plan (sinon l'Alt+Tab de l'utilisateur serait détourné sur tout
        // le système). Les frappes captées (Alt+Tab, touche Windows…) sont relayées
        // à l'hôte dans la branche Session ci-dessous.
        let hook_capture = matches!(self.screen, Screen::Session(_)) && ctx.input(|i| i.focused);
        self.system_hook.set_capture(hook_capture);
        let hook_keys = if hook_capture {
            self.system_hook.poll()
        } else {
            Vec::new()
        };

        // Récupère (une fois) le résultat de la vérification de mise à jour.
        if self.update_info.is_none() {
            if let Ok(info) = self.update_rx.try_recv() {
                self.update_info = Some(info);
            }
        }

        match &mut self.screen {
            Screen::Home => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(120.0);
                        ui.heading("ViewerKiller");
                        ui.label("Contrôle à distance sécurisé, chiffré de bout en bout");
                        ui.add_space(40.0);
                        if ui.button("🖥  Héberger (être contrôlé)").clicked() {
                            let (screen, task) =
                                start_host(&self.rt, self.host_fps, self.host_quality);
                            next = Some(screen);
                            new_host_task = Some(task);
                        }
                        ui.add_space(10.0);
                        if ui.button("🔗  Se connecter (contrôler)").clicked() {
                            next = Some(Screen::Connect(ConnectForm::default()));
                        }
                        ui.add_space(18.0);
                        ui.collapsing("⚙ Réglages d'hébergement", |ui| {
                            ui.add(
                                egui::Slider::new(&mut self.host_fps, 5..=30).text("Images / s"),
                            );
                            ui.add(
                                egui::Slider::new(&mut self.host_quality, 40..=95)
                                    .text("Qualité JPEG"),
                            );
                            ui.label(
                                egui::RichText::new(
                                    "Plus d'images/s et de qualité = plus fluide, mais plus de \
                                     bande passante. À appliquer avant de démarrer l'hébergement.",
                                )
                                .weak()
                                .small(),
                            );
                        });
                        if let Some(info) = &self.update_info {
                            ui.add_space(30.0);
                            ui.colored_label(
                                egui::Color32::from_rgb(0x40, 0xA0, 0xE0),
                                format!(
                                    "⬆ Nouvelle version disponible : v{} (actuelle v{})",
                                    info.latest,
                                    viewerkiller::update::CURRENT_VERSION
                                ),
                            );
                            ui.hyperlink_to("Voir la release", &info.url);
                        }
                    });
                });
            }

            Screen::Host(host) => {
                // Draine les messages du fil réseau (demandes / fins de session).
                while let Ok(msg) = host.consent_rx.try_recv() {
                    match msg {
                        ConsentMsg::Request { peer, reply } => {
                            // L'hôte ne sert qu'une session à la fois ; une
                            // nouvelle demande remplace (et refuse) l'ancienne.
                            if let Some(prev) = host.pending.replace(PendingConsent { peer, reply })
                            {
                                let _ = prev.reply.send(false);
                            }
                        }
                        ConsentMsg::Ended { peer } => {
                            if host.active == Some(peer) {
                                host.active = None;
                            }
                        }
                    }
                }

                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(80.0);
                        ui.heading("Hôte en écoute");
                        ui.add_space(20.0);
                        ui.label(format!("Écoute sur : {}", host.bind_addr));
                        if !host.addresses.is_empty() {
                            ui.add_space(10.0);
                            ui.label(
                                egui::RichText::new("Adresses IP (pour le contrôleur)").strong(),
                            );
                            for (name, ip) in &host.addresses {
                                ui.label(egui::RichText::new(format!("{name} : {ip}")).monospace());
                            }
                        }
                        ui.add_space(20.0);
                        ui.label(egui::RichText::new("Code").strong());
                        ui.label(egui::RichText::new(&host.code).size(36.0).monospace());
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new("Mot de passe").strong());
                        ui.label(egui::RichText::new(&host.password).size(24.0).monospace());
                        ui.add_space(20.0);
                        if let Some(peer) = host.active {
                            ui.colored_label(
                                egui::Color32::from_rgb(0xE0, 0x50, 0x50),
                                format!("🔴 Session en cours depuis {peer}"),
                            );
                        } else {
                            ui.label(
                                egui::RichText::new("Transmettez ces identifiants au contrôleur.")
                                    .weak(),
                            );
                        }
                        ui.add_space(20.0);
                        if ui.button("Retour (arrêter l'hébergement)").clicked() {
                            next = Some(Screen::Home);
                        }
                    });
                });

                // Boîte de dialogue de consentement, au-dessus de l'écran hôte.
                if let Some(peer) = host.pending.as_ref().map(|p| p.peer) {
                    egui::Window::new("Demande de connexion")
                        .collapsible(false)
                        .resizable(false)
                        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                        .show(ctx, |ui| {
                            ui.label(format!(
                                "{peer} souhaite prendre le contrôle de cet ordinateur."
                            ));
                            ui.add_space(12.0);
                            ui.horizontal(|ui| {
                                if ui.button("Accepter").clicked() {
                                    if let Some(p) = host.pending.take() {
                                        let _ = p.reply.send(true);
                                        host.active = Some(p.peer);
                                    }
                                }
                                if ui.button("Refuser").clicked() {
                                    if let Some(p) = host.pending.take() {
                                        let _ = p.reply.send(false);
                                    }
                                }
                            });
                        });
                }

                // Rafraîchir pour traiter les demandes même sans interaction.
                ctx.request_repaint_after(Duration::from_millis(200));
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
                            ui.label("Adresse de l'hôte");
                            ui.text_edit_singleline(&mut form.address);
                            ui.end_row();
                        });
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(
                            "Adresse IP (et port optionnel) de la machine à contrôler, ex. 10.0.0.5 ou 10.0.0.5:47600.",
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
                        ui.label("Connexion à l'hôte…");
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
                    // Capture clavier EXCLUSIVE, avant de dessiner l'UI : on retire
                    // les événements clavier de la file egui (sinon Tab déplace le
                    // focus vers « Déconnecter » et Entrée l'active — `num_presses`
                    // lit `events`), et on les transmet à l'hôte. Alt+Tab, touche
                    // Windows et Ctrl+Alt+Suppr restent captés par l'OS local
                    // (nécessiteraient un hook clavier bas niveau).
                    let (mods, key_events) = ctx.input_mut(|i| {
                        let captured = (i.modifiers, i.events.clone());
                        i.events.retain(|e| !is_keyboard_event(e));
                        captured
                    });
                    send_modifier_transitions(session, mods);
                    let mut to_send = Vec::new();
                    for event in &key_events {
                        translate_key_event(event, &mut to_send);
                    }
                    for ev in to_send {
                        let _ = session.input_tx.send(ev);
                    }
                    // Touches système captées par le hook bas niveau (Alt+Tab,
                    // touche Windows, Alt+F4/Échap, Ctrl+Échap…) → relayées à l'hôte.
                    for ks in &hook_keys {
                        let _ = session.input_tx.send(InputEvent::Key {
                            key: ks.vk,
                            pressed: ks.pressed,
                        });
                    }

                    egui::TopBottomPanel::top("barre").show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            if let Some((w, h)) = session.remote_size {
                                ui.label(format!("Écran distant {w}×{h}"));
                            } else {
                                ui.label("Connexion établie…");
                            }
                            if session.reconnecting {
                                ui.colored_label(
                                    egui::Color32::from_rgb(0xE0, 0xB0, 0x40),
                                    "⟳ Connexion perdue — reconnexion…",
                                );
                            }
                            if ui.button("Déconnecter").clicked() {
                                // Relâche les modificateurs tenus, sinon l'hôte
                                // garderait Ctrl/Alt/Shift enfoncés.
                                send_modifier_transitions(session, egui::Modifiers::default());
                                next = Some(Screen::Home);
                            }
                            // Sélecteur de moniteur (seulement si l'hôte en a plusieurs).
                            if session.monitors.len() > 1 {
                                ui.separator();
                                ui.label("Écran :");
                                let monitors = session.monitors.clone();
                                for m in &monitors {
                                    let label = if m.primary {
                                        format!("{} (principal)", m.index + 1)
                                    } else {
                                        format!("{}", m.index + 1)
                                    };
                                    if ui.button(label).clicked() {
                                        let _ = session.monitor_tx.send(m.index);
                                    }
                                }
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
                        monitor_tx,
                    } => Screen::Session(SessionScreen::new(events_rx, input_tx, monitor_tx)),
                    ConnectResult::Failed(e) => Screen::Error(e),
                });
            }
        }

        // Cycle de vie de la tâche hôte : on retient la nouvelle, et tout
        // retour à l'accueil arrête celle en cours (libère le port d'écoute).
        if let Some(task) = new_host_task {
            if let Some(old) = self.host_task.replace(task) {
                old.abort();
            }
        }
        if matches!(next, Some(Screen::Home)) {
            if let Some(task) = self.host_task.take() {
                task.abort();
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

    // Curseur distant (J12) : au survol de l'image, le curseur local du contrôleur
    // adopte la forme du curseur de l'hôte (texte, main, redimensionnement…) ;
    // masqué côté hôte → masqué ici.
    if response.hovered() {
        let icon = if session.cursor_visible {
            cursor_icon_of(session.cursor_kind)
        } else {
            egui::CursorIcon::None
        };
        ui.ctx().set_cursor_icon(icon);
    }

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

    // Boutons souris + molette. (Le clavier est capté en amont, au niveau App,
    // pour être retiré de la file egui avant que l'UI locale ne l'utilise.)
    let (primary, secondary, scroll) = ui.input(|i| {
        (
            i.pointer.primary_down(),
            i.pointer.secondary_down(),
            i.raw_scroll_delta,
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
}

/// Un événement egui est-il d'origine clavier ? (utilisé pour retirer ces
/// événements de la file egui et capter le clavier en exclusivité pour l'hôte).
fn is_keyboard_event(e: &egui::Event) -> bool {
    matches!(
        e,
        egui::Event::Key { .. }
            | egui::Event::Text(_)
            | egui::Event::Copy
            | egui::Event::Cut
            | egui::Event::Paste(_)
    )
}

/// Associe un type de curseur distant (J12) à l'icône egui correspondante.
fn cursor_icon_of(kind: CursorKind) -> egui::CursorIcon {
    match kind {
        CursorKind::Default => egui::CursorIcon::Default,
        CursorKind::Text => egui::CursorIcon::Text,
        CursorKind::Hand => egui::CursorIcon::PointingHand,
        CursorKind::Wait => egui::CursorIcon::Wait,
        CursorKind::Progress => egui::CursorIcon::Progress,
        CursorKind::Crosshair => egui::CursorIcon::Crosshair,
        CursorKind::Move => egui::CursorIcon::Move,
        CursorKind::NotAllowed => egui::CursorIcon::NotAllowed,
        CursorKind::ResizeNS => egui::CursorIcon::ResizeVertical,
        CursorKind::ResizeEW => egui::CursorIcon::ResizeHorizontal,
        CursorKind::ResizeNESW => egui::CursorIcon::ResizeNeSw,
        CursorKind::ResizeNWSE => egui::CursorIcon::ResizeNwSe,
    }
}

// Codes de touches virtuelles Windows des modificateurs.
const VK_SHIFT: u32 = 0x10;
const VK_CONTROL: u32 = 0x11;
const VK_MENU: u32 = 0x12; // Alt

// Codes de touches virtuelles Windows pour copier / couper / coller.
const VK_C: u32 = 0x43;
const VK_X: u32 = 0x58;
const VK_V: u32 = 0x56;

/// Traduit un événement clavier egui en `InputEvent`s pour l'hôte (hors
/// transitions de modificateurs, gérées à part par [`send_modifier_transitions`]).
///
/// Trois sources :
/// - `Event::Text` : caractères déjà résolus (majuscules, accents, AltGr) →
///   injectés en Unicode côté hôte.
/// - `Event::Copy`/`Cut`/`Paste` : **egui intercepte** Ctrl+C/X/V (et Ctrl+Inser,
///   Maj+Suppr/Inser) et pousse ces événements sémantiques **à la place** de
///   `Event::Key{C/X/V}` (retour anticipé dans egui-winit), en supprimant aussi le
///   `Text`. Sans traitement explicite, copier/couper/coller n'est **jamais**
///   transmis. On rejoue la lettre correspondante ; le contrôleur tient déjà Ctrl
///   enfoncé (transitions de modificateurs), donc l'hôte exécute bien Ctrl+C/X/V.
///   Le collage s'appuie sur le presse-papiers de l'hôte, maintenu synchro avec
///   celui du contrôleur (J11).
/// - `Event::Key` : touches non imprimables (Entrée, flèches, F1-F12…) et
///   raccourcis Ctrl/Alt ; les lettres/espaces nus passent par `Text` (pas de
///   doublon).
fn translate_key_event(event: &egui::Event, out: &mut Vec<InputEvent>) {
    match event {
        egui::Event::Text(text) => {
            for c in text.chars() {
                out.push(InputEvent::Char { c });
            }
        }
        egui::Event::Copy => push_key_tap(out, VK_C),
        egui::Event::Cut => push_key_tap(out, VK_X),
        egui::Event::Paste(_) => push_key_tap(out, VK_V),
        egui::Event::Key {
            key,
            pressed,
            modifiers,
            ..
        } => {
            // AltGr = Ctrl+Alt : le caractère composé arrive via `Text`.
            let altgr = modifiers.ctrl && modifiers.alt;
            let shortcut = (modifiers.ctrl || modifiers.alt) && !altgr;
            if !key_produces_text(*key) || shortcut {
                if let Some(vk) = egui_key_to_vk(*key) {
                    out.push(InputEvent::Key {
                        key: vk,
                        pressed: *pressed,
                    });
                }
            }
        }
        _ => {}
    }
}

/// Empile un appui + relâchement d'une touche virtuelle (raccourci ponctuel).
fn push_key_tap(out: &mut Vec<InputEvent>, vk: u32) {
    out.push(InputEvent::Key {
        key: vk,
        pressed: true,
    });
    out.push(InputEvent::Key {
        key: vk,
        pressed: false,
    });
}

/// Envoie les changements d'état des modificateurs depuis la dernière frame.
fn send_modifier_transitions(session: &mut SessionScreen, mods: egui::Modifiers) {
    let prev = session.mods;
    for (was, is, vk) in [
        (prev.shift, mods.shift, VK_SHIFT),
        (prev.ctrl, mods.ctrl, VK_CONTROL),
        (prev.alt, mods.alt, VK_MENU),
    ] {
        if was != is {
            let _ = session.input_tx.send(InputEvent::Key {
                key: vk,
                pressed: is,
            });
        }
    }
    session.mods = mods;
}

/// Touches dont la frappe produit du texte (acheminé par `Event::Text`) : on
/// ne les envoie en VK que pour les raccourcis Ctrl/Alt.
fn key_produces_text(key: egui::Key) -> bool {
    use egui::Key::*;
    !matches!(
        key,
        Enter
            | Tab
            | Backspace
            | Escape
            | Delete
            | Insert
            | Home
            | End
            | PageUp
            | PageDown
            | ArrowLeft
            | ArrowRight
            | ArrowUp
            | ArrowDown
            | F1
            | F2
            | F3
            | F4
            | F5
            | F6
            | F7
            | F8
            | F9
            | F10
            | F11
            | F12
    )
}

// --- Démarrage hôte / connexion -------------------------------------------

fn start_host(
    rt: &tokio::runtime::Runtime,
    fps: u32,
    quality: u8,
) -> (Screen, tokio::task::JoinHandle<()>) {
    let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), DEFAULT_PORT);
    let (code, password) = generate_credentials();
    let config = HostConfig {
        bind_addr,
        code: code.clone(),
        password: password.clone(),
        host_name: hostname(),
        tile_size: vk_media::DEFAULT_TILE_SIZE,
        quality,
        fps,
        require_consent: true,
        share_clipboard: true,
    };

    let (consent_tx, consent_rx) = mpsc::unbounded_channel();
    let task = rt.spawn(async move {
        let mut guard = BruteForceGuard::new(5, Duration::from_secs(60));
        let mut consent = GuiConsent { tx: consent_tx };
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

    let screen = Screen::Host(HostScreen {
        code,
        password,
        bind_addr,
        addresses: local_ipv4_addresses(),
        consent_rx,
        pending: None,
        active: None,
    });
    (screen, task)
}

enum ConnectResult {
    Ready {
        events_rx: UnboundedReceiver<SessionEvent>,
        input_tx: UnboundedSender<InputEvent>,
        monitor_tx: UnboundedSender<u32>,
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
    let addr = parse_addr(form.address.trim()).map_err(|e| e.to_string())?;
    let config = ControllerConfig {
        code: form.code.trim().to_string(),
        password: form.password.clone(),
        port: addr.port(),
    };

    let (tx, rx) = std_mpsc::channel();
    *CONNECT_RX.lock().unwrap() = Some(rx);

    rt.spawn(async move {
        match connect_to(addr, &config).await {
            Ok(enc) => {
                let (events_tx, events_rx) = mpsc::unbounded_channel();
                let (input_tx, input_rx) = mpsc::unbounded_channel();
                let (monitor_tx, monitor_rx) = mpsc::unbounded_channel();
                tokio::spawn(run_controller(
                    enc,
                    addr,
                    config,
                    events_tx,
                    input_rx,
                    monitor_rx,
                    true,
                    ReconnectPolicy::default(),
                ));
                let _ = tx.send(ConnectResult::Ready {
                    events_rx,
                    input_tx,
                    monitor_tx,
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

/// Parse `ip` ou `ip:port` ; utilise [`DEFAULT_PORT`] si le port est omis.
fn parse_addr(s: &str) -> anyhow::Result<SocketAddr> {
    if let Ok(addr) = s.parse::<SocketAddr>() {
        return Ok(addr);
    }
    let ip: IpAddr = s
        .parse()
        .map_err(|_| anyhow::anyhow!("format attendu : ip ou ip:port (ex. 10.0.0.5)"))?;
    Ok(SocketAddr::new(ip, DEFAULT_PORT))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Traduit une liste d'événements en `InputEvent`s (comme la boucle clavier).
    fn translate(events: &[egui::Event]) -> Vec<InputEvent> {
        let mut out = Vec::new();
        for e in events {
            translate_key_event(e, &mut out);
        }
        out
    }

    fn key_event(key: egui::Key, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        }
    }

    /// Régression : egui pousse `Event::Copy` pour Ctrl+C (pas `Event::Key{C}`).
    /// Sans traitement dédié, copier ne passait jamais côté hôte.
    #[test]
    fn ctrl_c_copy_injects_c_tap() {
        assert_eq!(
            translate(&[egui::Event::Copy]),
            vec![
                InputEvent::Key {
                    key: VK_C,
                    pressed: true
                },
                InputEvent::Key {
                    key: VK_C,
                    pressed: false
                },
            ]
        );
    }

    #[test]
    fn cut_and_paste_inject_x_and_v_taps() {
        assert_eq!(
            translate(&[egui::Event::Cut]),
            vec![
                InputEvent::Key {
                    key: VK_X,
                    pressed: true
                },
                InputEvent::Key {
                    key: VK_X,
                    pressed: false
                },
            ]
        );
        // Le contenu du Paste est ignoré : l'hôte colle son propre presse-papiers.
        assert_eq!(
            translate(&[egui::Event::Paste("peu importe".into())]),
            vec![
                InputEvent::Key {
                    key: VK_V,
                    pressed: true
                },
                InputEvent::Key {
                    key: VK_V,
                    pressed: false
                },
            ]
        );
    }

    #[test]
    fn plain_text_becomes_char() {
        assert_eq!(
            translate(&[egui::Event::Text("é".into())]),
            vec![InputEvent::Char { c: 'é' }]
        );
    }

    #[test]
    fn ctrl_a_shortcut_sent_as_vk() {
        // Ctrl+A n'a pas d'événement sémantique → Event::Key avec ctrl.
        let ev = key_event(
            egui::Key::A,
            true,
            egui::Modifiers {
                ctrl: true,
                ..Default::default()
            },
        );
        assert_eq!(
            translate(&[ev]),
            vec![InputEvent::Key {
                key: 0x41,
                pressed: true
            }]
        );
    }

    #[test]
    fn plain_letter_key_not_duplicated() {
        // Une lettre sans modificateur passe par Text : le Key correspondant ne
        // doit rien émettre (sinon la lettre sortirait deux fois).
        let ev = key_event(egui::Key::A, true, egui::Modifiers::default());
        assert!(translate(&[ev]).is_empty());
    }

    #[test]
    fn special_key_sent_without_modifier() {
        let ev = key_event(egui::Key::Enter, true, egui::Modifiers::default());
        assert_eq!(
            translate(&[ev]),
            vec![InputEvent::Key {
                key: 0x0D,
                pressed: true
            }]
        );
    }

    #[test]
    fn cursor_kind_maps_to_egui_icon() {
        assert_eq!(cursor_icon_of(CursorKind::Text), egui::CursorIcon::Text);
        assert_eq!(
            cursor_icon_of(CursorKind::Hand),
            egui::CursorIcon::PointingHand
        );
        assert_eq!(
            cursor_icon_of(CursorKind::ResizeNS),
            egui::CursorIcon::ResizeVertical
        );
        assert_eq!(
            cursor_icon_of(CursorKind::Default),
            egui::CursorIcon::Default
        );
    }
}
