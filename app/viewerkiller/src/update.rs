//! Vérification de disponibilité d'une nouvelle version (J16a).
//!
//! Purement **informatif** : interroge l'API GitHub Releases, compare la version
//! publiée à la version courante et signale si une plus récente existe. Ne
//! télécharge et ne remplace **rien** (ce sera J16b). Conçu pour un usage sur
//! VPN potentiellement isolé d'Internet : l'appel réseau est court, borné par un
//! délai, et **échoue silencieusement** (renvoie `None`) hors ligne.
//!
//! À exécuter hors du fil principal (thread dédié en GUI, `spawn_blocking` en
//! CLI) : `ureq` est synchrone.

use std::sync::Arc;
use std::time::Duration;

/// Dépôt GitHub interrogé (public, pas d'authentification).
pub const REPO: &str = "xunalopak/ViewerKiller";

/// Version compilée de ce binaire (= version du workspace).
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Délai maximal de l'appel réseau (connexion + réponse).
const TIMEOUT: Duration = Duration::from_secs(5);

/// Une version plus récente est disponible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    /// Version publiée, sans le préfixe `v` (ex. `"0.2.0"`).
    pub latest: String,
    /// URL de la page de release (à ouvrir dans le navigateur).
    pub url: String,
}

/// Interroge `releases/latest` et renvoie `Some` si la dernière release est
/// strictement plus récente que [`CURRENT_VERSION`]. Renvoie `None` en cas
/// d'égalité, d'ancienneté, ou de toute erreur (réseau, hors ligne, JSON).
///
/// **Bloquant** : à appeler depuis un thread dédié.
pub fn check_latest() -> Option<UpdateInfo> {
    let connector = native_tls::TlsConnector::new().ok()?;
    let agent = ureq::AgentBuilder::new()
        .timeout(TIMEOUT)
        .tls_connector(Arc::new(connector))
        .build();

    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = agent
        .get(&url)
        .set(
            "User-Agent",
            concat!("viewerkiller/", env!("CARGO_PKG_VERSION")),
        )
        .set("Accept", "application/vnd.github+json")
        .call()
        .ok()?
        .into_string()
        .ok()?;

    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    let tag = json.get("tag_name")?.as_str()?;
    let url = json.get("html_url")?.as_str()?.to_string();
    let latest = tag.trim_start_matches('v').trim().to_string();

    if is_newer(&latest, CURRENT_VERSION) {
        Some(UpdateInfo { latest, url })
    } else {
        None
    }
}

/// `a` est-elle strictement plus récente que `b` ? Compare des versions
/// `major.minor.patch` ; tout segment non numérique (pré-release…) compte pour 0.
fn is_newer(a: &str, b: &str) -> bool {
    parse(a) > parse(b)
}

/// Décompose `"x.y.z"` en `(x, y, z)` ; segments manquants ou non numériques → 0.
fn parse(v: &str) -> (u64, u64, u64) {
    let mut it = v
        .split(['.', '-', '+'])
        .map(|p| p.trim().parse::<u64>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_versions_detected() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.1.9", "0.1.8"));
    }

    #[test]
    fn same_or_older_is_not_newer() {
        assert!(!is_newer("0.1.9", "0.1.9"));
        assert!(!is_newer("0.1.8", "0.1.9"));
        assert!(!is_newer("0.0.9", "0.1.0"));
    }

    #[test]
    fn parse_tolerates_prefix_and_junk() {
        assert_eq!(parse("0.1.9"), (0, 1, 9));
        assert_eq!(parse("1.2"), (1, 2, 0));
        assert_eq!(parse("2"), (2, 0, 0));
        // Pré-release : le suffixe est ignoré (compté 0), donc « pas plus récent ».
        assert_eq!(parse("0.2.0-rc1"), (0, 2, 0));
        assert!(!is_newer("0.2.0-rc1", "0.2.0"));
    }
}
