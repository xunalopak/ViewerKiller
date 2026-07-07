//! Abstractions plateforme : capture d'écran et injection d'entrées.
//!
//! Les implémentations réelles sont spécifiques à Windows (capture DXGI/Desktop
//! Duplication, injection `SendInput`). Sur les autres plateformes, des stubs
//! permettent au reste du projet (protocole, réseau, crypto, UI) de compiler et
//! d'être testé pendant le développement.

use vk_core::protocol::{MonitorInfo, MouseButton};

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
    /// Dimensions courantes de l'écran capturé (moniteur sélectionné).
    fn dimensions(&self) -> (u32, u32);
    /// Capture la prochaine trame ; `Ok(None)` si aucune n'est disponible.
    fn capture(&mut self) -> anyhow::Result<Option<Frame>>;

    /// Liste des moniteurs disponibles (J12). Défaut : un seul moniteur,
    /// correspondant à [`dimensions`](Self::dimensions).
    fn monitors(&self) -> Vec<MonitorInfo> {
        let (width, height) = self.dimensions();
        vec![MonitorInfo {
            index: 0,
            width,
            height,
            primary: true,
        }]
    }

    /// Bascule la capture vers le moniteur `index` (J12). Défaut : seul l'index 0
    /// est valide (mono-écran).
    fn select_monitor(&mut self, index: u32) -> anyhow::Result<()> {
        if index == 0 {
            Ok(())
        } else {
            anyhow::bail!("moniteur {index} inexistant (mono-écran)")
        }
    }
}

/// Accès au presse-papiers texte du système (synchronisation façon RDP).
pub trait Clipboard: Send {
    /// Texte courant du presse-papiers, ou `None` s'il est vide/indisponible.
    fn get_text(&mut self) -> Option<String>;
    /// Remplace le texte du presse-papiers.
    fn set_text(&mut self, text: &str);
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

/// Une frappe de touche « système » captée par le hook clavier bas niveau du
/// contrôleur (Alt+Tab, touche Windows…), à relayer telle quelle à l'hôte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyStroke {
    /// Code de touche virtuel Windows (VK_*).
    pub vk: u32,
    pub pressed: bool,
}

/// Décide si une frappe doit être **captée** (supprimée localement et relayée à
/// l'hôte) au lieu d'être laissée à l'OS / à l'UI du contrôleur. Cible les combos
/// que le système intercepte avant l'application : touche Windows, `Win+<x>`,
/// `Alt+<x>` (hors AltGr = Ctrl+Alt, réservé à la saisie de texte), `Ctrl+Échap`.
///
/// Fonction **pure** (testable hors Windows) ; le hook Windows lui fournit l'état
/// courant des modificateurs.
pub fn should_capture_system_key(vk: u32, alt: bool, ctrl: bool, win: bool) -> bool {
    const VK_SHIFT: u32 = 0x10;
    const VK_CONTROL: u32 = 0x11;
    const VK_MENU: u32 = 0x12; // Alt
    const VK_ESCAPE: u32 = 0x1B;
    const VK_LWIN: u32 = 0x5B;
    const VK_RWIN: u32 = 0x5C;
    const VK_LSHIFT: u32 = 0xA0;
    const VK_RSHIFT: u32 = 0xA1;
    const VK_LCONTROL: u32 = 0xA2;
    const VK_RCONTROL: u32 = 0xA3;
    const VK_LMENU: u32 = 0xA4;
    const VK_RMENU: u32 = 0xA5;

    // La touche Windows n'est jamais délivrée à egui : on la capte pour la relayer
    // et supprimer l'ouverture locale du menu Démarrer.
    if vk == VK_LWIN || vk == VK_RWIN {
        return true;
    }
    // Les modificateurs nus (Alt/Ctrl/Maj) restent gérés par egui (suivi de
    // modificateurs → envoi à l'hôte) : on ne les capte pas.
    if matches!(
        vk,
        VK_SHIFT
            | VK_CONTROL
            | VK_MENU
            | VK_LSHIFT
            | VK_RSHIFT
            | VK_LCONTROL
            | VK_RCONTROL
            | VK_LMENU
            | VK_RMENU
    ) {
        return false;
    }
    // Win + <touche> (Win+D, Win+E…).
    if win {
        return true;
    }
    // Alt + <touche> (Alt+Tab, Alt+Échap, Alt+F4, Alt+Espace…), SAUF AltGr
    // (Ctrl+Alt) qui compose du texte (€, caractères accentués → laissés à egui).
    if alt && !ctrl {
        return true;
    }
    // Ctrl+Échap (menu Démarrer).
    if ctrl && vk == VK_ESCAPE {
        return true;
    }
    false
}

/// Hook clavier système du contrôleur : capte les combinaisons interceptées par
/// l'OS (Alt+Tab, touche Windows…) pour les relayer à l'hôte. Sans effet hors
/// Windows.
pub trait SystemKeyHook {
    /// Active/désactive la capture. **À n'activer qu'en session au premier plan**
    /// (sinon l'Alt+Tab de l'utilisateur serait détourné sur tout le système).
    fn set_capture(&self, active: bool);
    /// Récupère les frappes captées depuis le dernier appel.
    fn poll(&mut self) -> Vec<KeyStroke>;
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

/// État courant du curseur de l'hôte (type sémantique + visibilité), pour le
/// curseur distant (J12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorState {
    pub kind: vk_core::protocol::CursorKind,
    pub visible: bool,
}

/// Interroge le curseur courant du système. `None` hors Windows ou si
/// indisponible (le contrôleur garde alors son curseur par défaut).
pub fn probe_cursor() -> Option<CursorState> {
    #[cfg(windows)]
    {
        windows::probe_cursor()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Construit l'accès presse-papiers adapté à la plateforme courante.
pub fn default_clipboard() -> Box<dyn Clipboard> {
    #[cfg(windows)]
    {
        Box::new(windows::WindowsClipboard::new())
    }
    #[cfg(not(windows))]
    {
        Box::new(stub::StubClipboard)
    }
}

/// Installe le hook clavier système adapté à la plateforme (no-op hors Windows).
pub fn default_system_key_hook() -> Box<dyn SystemKeyHook> {
    #[cfg(windows)]
    {
        Box::new(windows::WindowsSystemKeyHook::new())
    }
    #[cfg(not(windows))]
    {
        Box::new(stub::StubSystemKeyHook)
    }
}

#[cfg(test)]
mod tests {
    use super::should_capture_system_key as cap;

    const TAB: u32 = 0x09;
    const ESC: u32 = 0x1B;
    const F4: u32 = 0x73;
    const LWIN: u32 = 0x5B;
    const ALT: u32 = 0x12;
    const LETTER_A: u32 = 0x41;
    const LETTER_D: u32 = 0x44;
    const LETTER_E: u32 = 0x45; // AltGr+E = € sur AZERTY

    #[test]
    fn captures_alt_tab_and_win() {
        assert!(cap(TAB, true, false, false)); // Alt+Tab
        assert!(cap(LWIN, false, false, false)); // touche Windows
        assert!(cap(LETTER_D, false, false, true)); // Win+D
        assert!(cap(F4, true, false, false)); // Alt+F4
        assert!(cap(ESC, false, true, false)); // Ctrl+Échap
    }

    #[test]
    fn ignores_plain_altgr_and_bare_modifiers() {
        assert!(!cap(LETTER_A, false, false, false)); // « a » nu
        assert!(!cap(LETTER_E, true, true, false)); // AltGr+E (€) → texte, laissé à egui
        assert!(!cap(ALT, true, false, false)); // Alt nu → suivi par egui
        assert!(!cap(TAB, false, false, false)); // Tab nu → navigation locale/egui
    }
}
