//! Cœur partagé de ViewerKiller.
//!
//! Ce crate regroupe ce qui est indépendant de la plateforme et du runtime
//! réseau : la définition du protocole (`protocol`) et le cadrage des messages
//! (`codec`). Il compile sur toutes les plateformes et constitue la base testée
//! par le reste du projet.

pub mod codec;
pub mod crypto;
pub mod protocol;
