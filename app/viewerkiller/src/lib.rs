//! Orchestration de ViewerKiller : boucles de session côté hôte et contrôleur.
//!
//! La logique réseau est découplée de l'interface : le contrôleur communique
//! avec l'UI via des canaux ([`controller::SessionEvent`] en sortie,
//! [`vk_core::protocol::InputEvent`] en entrée), ce qui rend l'ensemble
//! testable sans affichage ni matériel Windows (voir `tests/e2e.rs`).

pub mod controller;
pub mod host;

pub use controller::{controller_session, ControllerConfig, SessionEvent};
pub use host::{handle_connection, serve, HostConfig};

/// Génère un code de connexion à 6 chiffres et un mot de passe aléatoire fort.
pub fn generate_credentials() -> (String, String) {
    use rand::{distributions::Alphanumeric, Rng};
    let mut rng = rand::thread_rng();
    let code = format!("{:06}", rng.gen_range(0..1_000_000u32));
    let password: String = (0..12).map(|_| rng.sample(Alphanumeric) as char).collect();
    (code, password)
}
