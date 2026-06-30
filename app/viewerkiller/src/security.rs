//! Durcissement côté hôte : protection anti-bruteforce sur le mot de passe et
//! demande de consentement avant l'ouverture d'une session.

use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::time::{Duration, Instant};

/// Limiteur de tentatives par adresse IP.
///
/// Après `max_failures` échecs d'authentification consécutifs, l'IP est
/// verrouillée pendant `lockout`. Une authentification réussie remet le compteur
/// à zéro.
pub struct BruteForceGuard {
    max_failures: u32,
    lockout: Duration,
    records: HashMap<IpAddr, Record>,
}

#[derive(Default)]
struct Record {
    failures: u32,
    locked_until: Option<Instant>,
}

impl BruteForceGuard {
    pub fn new(max_failures: u32, lockout: Duration) -> Self {
        Self {
            max_failures: max_failures.max(1),
            lockout,
            records: HashMap::new(),
        }
    }

    /// `true` si l'IP est autorisée à tenter une connexion maintenant.
    pub fn check(&self, ip: IpAddr) -> bool {
        match self.records.get(&ip).and_then(|r| r.locked_until) {
            Some(until) => Instant::now() >= until,
            None => true,
        }
    }

    /// Enregistre un échec d'authentification ; verrouille si le seuil est atteint.
    pub fn record_failure(&mut self, ip: IpAddr) {
        let record = self.records.entry(ip).or_default();
        record.failures += 1;
        if record.failures >= self.max_failures {
            record.locked_until = Some(Instant::now() + self.lockout);
        }
    }

    /// Réinitialise le compteur d'une IP après un succès.
    pub fn record_success(&mut self, ip: IpAddr) {
        self.records.remove(&ip);
    }
}

/// Future renvoyée par [`Consent::request`] (objet-sûr, donc utilisable en
/// `dyn`).
pub type ConsentFuture = Pin<Box<dyn Future<Output = bool> + Send>>;

/// Décide si une connexion authentifiée est autorisée à prendre la main.
///
/// Branché par l'UI (boîte de dialogue accepter/refuser) ; en CLI sans
/// surveillance on utilise [`AutoAccept`].
pub trait Consent: Send {
    fn request(&mut self, peer: SocketAddr) -> ConsentFuture;
}

/// Accepte toute connexion (mode non surveillé).
pub struct AutoAccept;

impl Consent for AutoAccept {
    fn request(&mut self, _peer: SocketAddr) -> ConsentFuture {
        Box::pin(async { true })
    }
}

/// Refuse toute connexion (utile en tests et pour mettre l'hôte « en pause »).
pub struct RejectAll;

impl Consent for RejectAll {
    fn request(&mut self, _peer: SocketAddr) -> ConsentFuture {
        Box::pin(async { false })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip() -> IpAddr {
        "10.0.0.7".parse().unwrap()
    }

    #[test]
    fn locks_out_after_threshold() {
        let mut guard = BruteForceGuard::new(3, Duration::from_secs(60));
        assert!(guard.check(ip()));
        guard.record_failure(ip());
        guard.record_failure(ip());
        assert!(guard.check(ip()), "pas encore au seuil");
        guard.record_failure(ip());
        assert!(!guard.check(ip()), "verrouillé au 3e échec");
    }

    #[test]
    fn success_resets_counter() {
        let mut guard = BruteForceGuard::new(2, Duration::from_secs(60));
        guard.record_failure(ip());
        guard.record_failure(ip());
        assert!(!guard.check(ip()));
        guard.record_success(ip());
        assert!(guard.check(ip()));
    }

    #[test]
    fn zero_lockout_is_never_locked() {
        let mut guard = BruteForceGuard::new(1, Duration::ZERO);
        guard.record_failure(ip());
        assert!(guard.check(ip()));
    }
}
