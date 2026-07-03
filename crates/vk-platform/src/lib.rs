//! Abstractions plateforme : capture d'écran et injection d'entrées.
//!
//! Les implémentations réelles sont spécifiques à Windows (capture DXGI/Desktop
//! Duplication, injection `SendInput`). Sur les autres plateformes, des stubs
//! permettent au reste du projet (protocole, réseau, crypto, UI) de compiler et
//! d'être testé pendant le développement.

use vk_core::protocol::MouseButton;

/// Une trame d'écran capturée, au format **BGRA** (4 octets/pixel, ordre des
/// canaux fourni par DXGI sous Windows).
#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Frame {
    /// Vérifie que la taille du tampon correspond aux dimensions annoncées.
    pub fn is_well_formed(&self) -> bool {
        self.data.len() == (self.width as usize) * (self.height as usize) * 4
    }
}

/// Source de trames écran.
pub trait ScreenCapturer: Send {
    /// Dimensions courantes de l'écran capturé.
    fn dimensions(&self) -> (u32, u32);
    /// Capture la prochaine trame ; `Ok(None)` si aucune n'est disponible.
    fn capture(&mut self) -> anyhow::Result<Option<Frame>>;
}

/// Cible d'injection d'événements d'entrée (côté hôte).
pub trait InputInjector: Send {
    fn mouse_move(&mut self, x: i32, y: i32) -> anyhow::Result<()>;
    fn mouse_button(&mut self, button: MouseButton, pressed: bool) -> anyhow::Result<()>;
    fn mouse_scroll(&mut self, dx: i32, dy: i32) -> anyhow::Result<()>;
    fn key(&mut self, key: u32, pressed: bool) -> anyhow::Result<()>;
    /// Injecte un caractère de texte (frappe Unicode complète, indépendante de
    /// la disposition clavier de l'hôte).
    fn char_input(&mut self, c: char) -> anyhow::Result<()>;
}

#[cfg(windows)]
pub mod windows;

#[cfg(not(windows))]
pub mod stub;

/// Construit le capteur d'écran adapté à la plateforme courante.
pub fn default_capturer() -> anyhow::Result<Box<dyn ScreenCapturer>> {
    #[cfg(windows)]
    {
        Ok(Box::new(windows::WindowsCapturer::new()?))
    }
    #[cfg(not(windows))]
    {
        Ok(Box::new(stub::StubCapturer::new()))
    }
}

/// Construit l'injecteur d'entrées adapté à la plateforme courante.
pub fn default_injector() -> anyhow::Result<Box<dyn InputInjector>> {
    #[cfg(windows)]
    {
        Ok(Box::new(windows::WindowsInjector::new()?))
    }
    #[cfg(not(windows))]
    {
        Ok(Box::new(stub::StubInjector))
    }
}
