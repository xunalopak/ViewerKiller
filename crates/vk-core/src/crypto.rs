//! Couche cryptographique : handshake et transport chiffré via le framework
//! Noise (crate `snow`).
//!
//! Motif retenu : **NNpsk0**. Aucune clé statique n'est nécessaire ;
//! l'authentification mutuelle repose entièrement sur un secret pré-partagé
//! (PSK) dérivé du mot de passe affiché par l'hôte. Un mot de passe erroné fait
//! échouer le handshake (échec de vérification du tag AEAD), donc aucune session
//! ne s'établit et un homme du milieu sans le mot de passe est exclu.
//!
//! Ce chiffrement de bout en bout protège l'écran et les frappes sur le réseau
//! local : même un autre appareil du LAN qui capterait le trafic ne voit jamais
//! rien en clair sans le mot de passe.

use snow::params::NoiseParams;

/// Paramètres Noise : NNpsk0, DH X25519, AEAD ChaCha20-Poly1305, hachage BLAKE2s.
pub const NOISE_PARAMS: &str = "Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s";

/// Charge utile maximale d'un message Noise (65535 − 16 octets de tag AEAD).
pub const MAX_NOISE_PAYLOAD: usize = 65519;

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("erreur Noise : {0}")]
    Noise(#[from] snow::Error),
    #[error("charge utile trop grande pour un message Noise : {0} octets")]
    PayloadTooLarge(usize),
}

/// Dérive la clé pré-partagée (PSK) de 32 octets à partir du mot de passe.
///
/// On utilise `blake3::derive_key` avec un contexte dédié pour la séparation de
/// domaine (le même mot de passe ne produira pas la même clé pour un autre usage).
pub fn derive_psk(password: &str) -> [u8; 32] {
    blake3::derive_key("viewerkiller noise psk v1", password.as_bytes())
}

fn params() -> NoiseParams {
    // `NOISE_PARAMS` est une constante valide : le parse ne peut pas échouer.
    NOISE_PARAMS.parse().expect("paramètres Noise valides")
}

/// État de handshake Noise.
///
/// Convention ViewerKiller : le **contrôleur** est l'initiateur, l'**hôte** est
/// le répondeur.
pub struct Handshake {
    state: snow::HandshakeState,
}

impl Handshake {
    /// Crée l'initiateur (côté contrôleur).
    pub fn initiator(psk: &[u8; 32]) -> Result<Self, CryptoError> {
        let state = snow::Builder::new(params()).psk(0, psk).build_initiator()?;
        Ok(Self { state })
    }

    /// Crée le répondeur (côté hôte).
    pub fn responder(psk: &[u8; 32]) -> Result<Self, CryptoError> {
        let state = snow::Builder::new(params()).psk(0, psk).build_responder()?;
        Ok(Self { state })
    }

    /// Écrit le prochain message de handshake dans `out`, renvoie sa longueur.
    pub fn write_message(&mut self, payload: &[u8], out: &mut [u8]) -> Result<usize, CryptoError> {
        Ok(self.state.write_message(payload, out)?)
    }

    /// Lit un message de handshake reçu, écrit la charge utile dans `out`.
    pub fn read_message(&mut self, msg: &[u8], out: &mut [u8]) -> Result<usize, CryptoError> {
        Ok(self.state.read_message(msg, out)?)
    }

    /// Indique si le handshake est terminé des deux côtés du point de vue local.
    pub fn is_finished(&self) -> bool {
        self.state.is_handshake_finished()
    }

    /// Passe en mode transport une fois le handshake terminé.
    pub fn into_transport(self) -> Result<Transport, CryptoError> {
        Ok(Transport {
            state: self.state.into_transport_mode()?,
        })
    }
}

/// Session de transport chiffrée (post-handshake).
///
/// Chaque appel chiffre/déchiffre **un fragment** d'au plus
/// [`MAX_NOISE_PAYLOAD`] octets. Le découpage d'un flux applicatif plus grand en
/// fragments relève de la couche transport (voir le binaire applicatif).
pub struct Transport {
    state: snow::TransportState,
}

impl Transport {
    /// Chiffre un fragment de texte clair (≤ [`MAX_NOISE_PAYLOAD`]).
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if plaintext.len() > MAX_NOISE_PAYLOAD {
            return Err(CryptoError::PayloadTooLarge(plaintext.len()));
        }
        let mut out = vec![0u8; plaintext.len() + 16];
        let n = self.state.write_message(plaintext, &mut out)?;
        out.truncate(n);
        Ok(out)
    }

    /// Déchiffre un fragment reçu.
    pub fn decrypt(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let mut out = vec![0u8; ciphertext.len()];
        let n = self.state.read_message(ciphertext, &mut out)?;
        out.truncate(n);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Déroule un handshake NNpsk0 complet en mémoire et renvoie les deux
    /// sessions de transport (initiateur, répondeur).
    fn run_handshake(
        psk_initiator: &[u8; 32],
        psk_responder: &[u8; 32],
    ) -> Result<(Transport, Transport), CryptoError> {
        let mut ini = Handshake::initiator(psk_initiator)?;
        let mut res = Handshake::responder(psk_responder)?;
        let mut buf = [0u8; 1024];
        let mut tmp = [0u8; 1024];

        // -> e
        let n = ini.write_message(&[], &mut buf)?;
        res.read_message(&buf[..n], &mut tmp)?;

        // <- e, ee
        let n = res.write_message(&[], &mut buf)?;
        ini.read_message(&buf[..n], &mut tmp)?;

        assert!(ini.is_finished());
        assert!(res.is_finished());

        Ok((ini.into_transport()?, res.into_transport()?))
    }

    #[test]
    fn matching_password_establishes_session() {
        let psk = derive_psk("hunter2");
        let (mut a, mut b) = run_handshake(&psk, &psk).unwrap();

        let ct = a.encrypt(b"bonjour").unwrap();
        assert_eq!(b.decrypt(&ct).unwrap(), b"bonjour");

        let ct = b.encrypt(b"salut").unwrap();
        assert_eq!(a.decrypt(&ct).unwrap(), b"salut");
    }

    #[test]
    fn mismatched_password_fails() {
        let good = derive_psk("hunter2");
        let bad = derive_psk("mauvais");
        assert!(
            run_handshake(&good, &bad).is_err(),
            "le handshake doit échouer si les mots de passe diffèrent"
        );
    }

    #[test]
    fn payload_too_large_rejected() {
        let psk = derive_psk("x");
        let (mut a, _b) = run_handshake(&psk, &psk).unwrap();
        let big = vec![0u8; MAX_NOISE_PAYLOAD + 1];
        assert!(matches!(
            a.encrypt(&big),
            Err(CryptoError::PayloadTooLarge(_))
        ));
    }

    #[test]
    fn derive_psk_is_deterministic_and_sensitive() {
        assert_eq!(derive_psk("abc"), derive_psk("abc"));
        assert_ne!(derive_psk("abc"), derive_psk("abd"));
    }
}
