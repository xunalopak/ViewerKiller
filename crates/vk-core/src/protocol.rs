//! Définition du protocole ViewerKiller : messages de découverte et de session.

use serde::{Deserialize, Serialize};

/// Version du protocole, incrémentée à chaque changement incompatible.
pub const PROTO_VERSION: u16 = 2;

/// Port TCP par défaut de l'agent hôte.
pub const DEFAULT_PORT: u16 = 47600;

/// Messages échangés AVANT la session chiffrée, pour la vérification du code
/// de connexion.
///
/// Ils transitent en clair sur le réseau ; la sécurité réelle de la session
/// repose sur le handshake Noise authentifié par mot de passe qui suit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiscoveryMessage {
    /// Contrôleur → agent : « es-tu l'hôte qui affiche ce code ? »
    Probe { proto_version: u16, code: String },
    /// Agent → contrôleur : réponse à une sonde.
    ProbeResult { matches: bool, host_name: String },
}

/// Boutons de souris pris en charge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Événement d'entrée envoyé par le contrôleur vers l'hôte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputEvent {
    /// Position absolue dans les coordonnées écran de l'hôte.
    MouseMove {
        x: i32,
        y: i32,
    },
    MouseButton {
        button: MouseButton,
        pressed: bool,
    },
    MouseScroll {
        dx: i32,
        dy: i32,
    },
    /// Touche clavier (code de touche virtuel Windows), pressée ou relâchée.
    /// Réservé aux touches non imprimables, modificateurs et raccourcis ; le
    /// texte passe par [`InputEvent::Char`].
    Key {
        key: u32,
        pressed: bool,
    },
    /// Caractère de texte (majuscules, accents, symboles…), déjà résolu par la
    /// disposition clavier du contrôleur. Injecté en Unicode côté hôte.
    ///
    /// NB : nouveau variant ajouté **en fin d'enum** — postcard encode le
    /// discriminant par ordre de déclaration, ne pas réordonner.
    Char {
        c: char,
    },
}

/// Codec d'une tuile d'image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TileCodec {
    /// JPEG (avec perte) — privilégié pour les zones photographiques.
    Jpeg,
    /// BGRA brut compressé par deflate (sans perte) — utile pour le texte/UI.
    DeflateBgra,
}

/// Une tuile rectangulaire mise à jour de l'écran distant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tile {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub codec: TileCodec,
    pub data: Vec<u8>,
}

/// Un lot de tuiles modifiées correspondant à une trame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameUpdate {
    /// Numéro de séquence croissant (diagnostic et ordonnancement).
    pub seq: u64,
    pub tiles: Vec<Tile>,
}

/// Messages contrôleur → hôte durant une session chiffrée.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControllerMessage {
    Input(InputEvent),
    /// Demande une retransmission complète de l'écran (ex. après resize).
    RequestFullFrame,
    Bye,
}

/// Messages hôte → contrôleur durant une session chiffrée.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostMessage {
    /// Géométrie de l'écran partagé, envoyée en début de session.
    ScreenInfo {
        width: u32,
        height: u32,
    },
    /// Mise à jour d'image.
    Frame(FrameUpdate),
    Bye,
}
