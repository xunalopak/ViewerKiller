//! Couche réseau de ViewerKiller (asynchrone, tokio).
//!
//! - [`frame`] : lecture/écriture d'un message unique cadré (en clair), utilisé
//!   pour la phase de découverte.
//! - [`transport`] : flux chiffré de bout en bout ([`transport::EncryptedStream`])
//!   au-dessus d'une connexion TCP, avec pilotage du handshake Noise et
//!   fragmentation transparente des gros messages.
//! - [`discovery`] : énumération des interfaces, calcul du sous-réseau VPN et
//!   balayage parallèle à la recherche de l'hôte affichant un code donné.

pub mod discovery;
pub mod frame;
pub mod transport;

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("E/S réseau : {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Codec(#[from] vk_core::codec::CodecError),
    #[error(transparent)]
    Crypto(#[from] vk_core::crypto::CryptoError),
    #[error("message inattendu durant la phase de découverte")]
    UnexpectedMessage,
}

pub type Result<T> = std::result::Result<T, NetError>;
