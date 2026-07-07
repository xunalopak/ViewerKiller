//! Vérification et application des mises à jour (J16a + J16b).
//!
//! - **J16a** : interroge l'API GitHub Releases et signale (sans rien télécharger)
//!   si une version plus récente existe. Silencieux hors ligne.
//! - **J16b** : sur demande explicite de l'utilisateur, télécharge le binaire de la
//!   dernière release, **vérifie son SHA256** contre le `SHA256SUMS.txt` publié,
//!   remplace l'exécutable courant et relance.
//!
//! `ureq` est synchrone : tout est **bloquant**, à exécuter hors du fil principal.

use std::fmt::Write as _;
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

/// Dépôt GitHub interrogé (public, pas d'authentification).
pub const REPO: &str = "xunalopak/ViewerKiller";

/// Version compilée de ce binaire (= version du workspace).
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Noms des assets de release à mettre à jour selon le binaire.
pub const ASSET_GUI: &str = "viewerkiller-gui.exe";
pub const ASSET_CLI: &str = "viewerkiller.exe";
/// Fichier de sommes de contrôle publié par le workflow de release.
const SHA256SUMS: &str = "SHA256SUMS.txt";

const USER_AGENT: &str = concat!("viewerkiller/", env!("CARGO_PKG_VERSION"));

/// Délai de l'appel de vérification (court).
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);
/// Délai des téléchargements (plus long : plusieurs Mio).
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// Une version plus récente est disponible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    /// Version publiée, sans le préfixe `v` (ex. `"0.2.0"`).
    pub latest: String,
    /// URL de la page de release (à ouvrir dans le navigateur).
    pub url: String,
}

/// Construit un agent HTTP (TLS `native-tls`), ou `None` si le TLS échoue.
fn agent(timeout: Duration) -> Option<ureq::Agent> {
    let connector = native_tls::TlsConnector::new().ok()?;
    Some(
        ureq::AgentBuilder::new()
            .timeout(timeout)
            .tls_connector(Arc::new(connector))
            .build(),
    )
}

/// Interroge `releases/latest` et renvoie `Some` si la dernière release est
/// strictement plus récente que [`CURRENT_VERSION`]. `None` en cas d'égalité,
/// d'ancienneté ou de toute erreur (réseau, hors ligne, JSON). **Bloquant.**
pub fn check_latest() -> Option<UpdateInfo> {
    let agent = agent(CHECK_TIMEOUT)?;
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = agent
        .get(&url)
        .set("User-Agent", USER_AGENT)
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

/// Télécharge l'asset `asset_name` de la dernière release, **vérifie son SHA256**
/// contre `SHA256SUMS.txt`, et renvoie ses octets. Échoue si l'intégrité n'est pas
/// confirmée. **Bloquant.**
pub fn download_and_verify(asset_name: &str) -> Result<Vec<u8>> {
    let agent = agent(DOWNLOAD_TIMEOUT).context("initialisation TLS")?;

    // 1. Métadonnées de la release : URLs de téléchargement des assets.
    let body = agent
        .get(&format!(
            "https://api.github.com/repos/{REPO}/releases/latest"
        ))
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .call()?
        .into_string()?;
    let json: serde_json::Value = serde_json::from_str(&body)?;
    let assets = json
        .get("assets")
        .and_then(|a| a.as_array())
        .context("release sans assets")?;
    let url_of = |name: &str| -> Option<String> {
        assets
            .iter()
            .find(|a| a.get("name").and_then(|n| n.as_str()) == Some(name))
            .and_then(|a| a.get("browser_download_url"))
            .and_then(|u| u.as_str())
            .map(String::from)
    };
    let asset_url =
        url_of(asset_name).with_context(|| format!("asset {asset_name} introuvable"))?;
    let sums_url = url_of(SHA256SUMS).context("SHA256SUMS.txt absent de la release")?;

    // 2. Somme attendue, puis binaire.
    let sums = agent
        .get(&sums_url)
        .set("User-Agent", USER_AGENT)
        .call()?
        .into_string()?;
    let expected = parse_sha256sums(&sums, asset_name)
        .with_context(|| format!("pas de somme SHA256 pour {asset_name}"))?;

    let mut data = Vec::new();
    agent
        .get(&asset_url)
        .set("User-Agent", USER_AGENT)
        .call()?
        .into_reader()
        .read_to_end(&mut data)?;

    // 3. Vérification d'intégrité AVANT toute utilisation du binaire.
    let actual = sha256_hex(&data);
    if actual != expected {
        bail!("intégrité invalide (SHA256 attendu {expected}, obtenu {actual}) — rejeté");
    }
    Ok(data)
}

/// Télécharge + vérifie + remplace l'exécutable courant + relance. Ne **revient**
/// qu'en cas d'échec (sinon le processus est remplacé par la nouvelle version).
/// **Bloquant.**
pub fn self_update(asset_name: &str) -> Result<()> {
    let data = download_and_verify(asset_name)?;
    apply_and_relaunch(&data)
}

/// Remplace l'exécutable courant par `new_exe` et relance. Un exe en cours ne peut
/// être écrasé, mais il peut être **renommé** : on renomme l'ancien en `.old`
/// (nettoyé au prochain démarrage), on écrit le neuf, on relance et on quitte.
fn apply_and_relaunch(new_exe: &[u8]) -> Result<()> {
    let exe = std::env::current_exe().context("chemin de l'exécutable courant")?;
    let old = exe.with_extension("old");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).context("renommage de l'exécutable courant")?;
    if let Err(e) = std::fs::write(&exe, new_exe) {
        // Restaure l'ancien binaire si l'écriture échoue.
        let _ = std::fs::rename(&old, &exe);
        return Err(anyhow::anyhow!("écriture du nouveau binaire : {e}"));
    }
    std::process::Command::new(&exe)
        .spawn()
        .context("relancement de la nouvelle version")?;
    std::process::exit(0);
}

/// Supprime un éventuel binaire `.old` laissé par une mise à jour précédente. À
/// appeler au démarrage.
pub fn cleanup_old_update() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(exe.with_extension("old"));
    }
}

/// SHA256 d'un tampon, en hexadécimal minuscule (64 caractères).
fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut s = String::with_capacity(64);
    for b in digest {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Extrait le SHA256 (hex minuscule) de `asset_name` depuis un fichier au format
/// `sha256sum` (`<hash>  <nom>` par ligne). `None` si absent/malformé.
fn parse_sha256sums(text: &str, asset_name: &str) -> Option<String> {
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(hash), Some(name)) = (it.next(), it.next()) else {
            continue;
        };
        if name == asset_name && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
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
        assert_eq!(parse("0.2.0-rc1"), (0, 2, 0));
        assert!(!is_newer("0.2.0-rc1", "0.2.0"));
    }

    #[test]
    fn sha256_known_vector() {
        // SHA256("") et SHA256("abc") — vecteurs standard.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn parse_sha256sums_finds_asset() {
        let sums = "\
aaaa...  autre.zip
ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  viewerkiller.exe
deadbeef  court
";
        assert_eq!(
            parse_sha256sums(sums, "viewerkiller.exe").as_deref(),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
        assert!(parse_sha256sums(sums, "absent.exe").is_none());
        // Hash trop court → ignoré.
        assert!(parse_sha256sums("deadbeef  court", "court").is_none());
    }
}
