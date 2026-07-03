//! Synchronisation du presse-papiers texte entre hôte et contrôleur, façon RDP.
//!
//! Les deux extrémités sondent périodiquement leur presse-papiers local et
//! envoient tout changement à l'autre. Pour éviter une boucle de renvoi (A → B
//! → A …), on mémorise le dernier texte synchronisé : ni un texte que l'on
//! vient d'émettre, ni un texte que l'on vient de recevoir n'est réémis.

use vk_platform::Clipboard;

/// État de synchronisation d'un presse-papiers avec le pair distant.
pub struct ClipboardSync {
    clip: Box<dyn Clipboard>,
    last: Option<String>,
}

impl ClipboardSync {
    pub fn new(clip: Box<dyn Clipboard>) -> Self {
        Self { clip, last: None }
    }

    /// Sonde le presse-papiers local ; renvoie le texte s'il a changé depuis la
    /// dernière synchronisation (donc à transmettre au pair), sinon `None`.
    pub fn poll_local(&mut self) -> Option<String> {
        let current = self.clip.get_text()?;
        if self.last.as_deref() == Some(current.as_str()) {
            return None;
        }
        self.last = Some(current.clone());
        Some(current)
    }

    /// Applique un texte reçu du pair au presse-papiers local, sans le renvoyer
    /// (il devient le dernier texte synchronisé).
    pub fn apply_remote(&mut self, text: String) {
        self.clip.set_text(&text);
        self.last = Some(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Presse-papiers en mémoire pour les tests (pas de système de fenêtrage).
    #[derive(Default)]
    struct MockClipboard {
        text: Option<String>,
    }
    impl Clipboard for MockClipboard {
        fn get_text(&mut self) -> Option<String> {
            self.text.clone()
        }
        fn set_text(&mut self, text: &str) {
            self.text = Some(text.to_owned());
        }
    }

    fn with_text(text: &str) -> Box<dyn Clipboard> {
        Box::new(MockClipboard {
            text: Some(text.to_owned()),
        })
    }

    #[test]
    fn local_change_reported_once() {
        let mut sync = ClipboardSync::new(with_text("bonjour"));
        assert_eq!(sync.poll_local().as_deref(), Some("bonjour"));
        // Inchangé : plus rien à transmettre.
        assert_eq!(sync.poll_local(), None);
    }

    #[test]
    fn empty_clipboard_reports_nothing() {
        let mut sync = ClipboardSync::new(Box::new(MockClipboard::default()));
        assert_eq!(sync.poll_local(), None);
    }

    #[test]
    fn applied_remote_is_not_echoed_back() {
        let mut sync = ClipboardSync::new(Box::new(MockClipboard::default()));
        sync.apply_remote("depuis le pair".to_owned());
        // Le texte reçu ne doit pas repartir vers le pair au prochain sondage.
        assert_eq!(sync.poll_local(), None);
    }
}
