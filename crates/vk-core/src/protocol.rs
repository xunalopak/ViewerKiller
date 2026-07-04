//! Définition du protocole ViewerKiller : messages de découverte et de session.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Version du protocole, incrémentée à chaque changement incompatible.
pub const PROTO_VERSION: u16 = 5;

/// Port TCP par défaut de l'agent hôte.
pub const DEFAULT_PORT: u16 = 47600;

/// Intervalle d'émission des messages de maintien de session
/// ([`ControllerMessage::Ping`] / [`HostMessage::Ping`]). Chaque pair émet un
/// Ping quand il n'a rien d'autre à envoyer, prouvant à l'autre que la connexion
/// est vivante même écran figé et sans saisie.
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);

/// Sans **aucun** message reçu du pair pendant ce délai, la session est réputée
/// morte (VPN coupé, machine éteinte, câble débranché…) et fermée. Doit rester
/// nettement supérieur à [`KEEPALIVE_INTERVAL`] pour tolérer quelques pertes.
pub const SESSION_TIMEOUT: Duration = Duration::from_secs(15);

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

/// Description d'un moniteur de l'hôte, pour le choix côté contrôleur (J12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorInfo {
    /// Index stable (0 = premier moniteur) utilisé pour la sélection.
    pub index: u32,
    pub width: u32,
    pub height: u32,
    /// Vrai pour le moniteur principal.
    pub primary: bool,
}

/// Messages contrôleur → hôte durant une session chiffrée.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControllerMessage {
    Input(InputEvent),
    /// Demande une retransmission complète de l'écran (ex. après resize).
    RequestFullFrame,
    Bye,
    /// Le presse-papiers texte du contrôleur a changé (synchronisation façon
    /// RDP). Variant ajouté **en fin d'enum** (postcard, ordre = discriminant).
    Clipboard(String),
    /// Maintien de connexion (keepalive). Variant ajouté **en fin d'enum**.
    Ping,
    /// Demande de bascule vers le moniteur `index` (J12). **Fin d'enum.**
    SelectMonitor {
        index: u32,
    },
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
    /// Le presse-papiers texte de l'hôte a changé (synchronisation façon RDP).
    /// Variant ajouté **en fin d'enum** (postcard, ordre = discriminant).
    Clipboard(String),
    /// Maintien de connexion (keepalive). Variant ajouté **en fin d'enum**.
    Ping,
    /// Liste des moniteurs disponibles, envoyée en début de session (J12).
    /// **Fin d'enum.**
    Monitors(Vec<MonitorInfo>),
}
