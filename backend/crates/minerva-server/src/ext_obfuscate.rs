//! Deterministic identity pseudonymization for external viewers.
//!
//! When the requester is an `ext:` user they should not see real names or
//! eppns of other non-admin users; external collaborators are granted
//! narrow access and the rest of the user population is off-limits.
//! Identities are replaced with two-word passphrases ("Wombling Wombat",
//! `wombling.wombat@domain`) derived from a hash of the target eppn, so
//! the same user always maps to the same pseudonym within a request and
//! is stable across requests until the DB population changes.
//!
//! Collisions are resolved by iterating the hash input: eppns are sorted
//! alphabetically, and if a rolled pair is already taken by an earlier
//! eppn the attempt counter is bumped and the RNG reseeded. No DB table
//! is involved; the map is rebuilt per-request from `users.list_all`.
//! Adding a new user can shift the pseudonym of later-sorting colliding
//! users; acceptable, since the alternative is persisting assignments.
//!
//! Admins and the viewer themselves are passed through unchanged so the
//! UI can still render "signed in as ..." and surface admin contacts.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use minerva_core::models::User;
use rand_chacha::ChaCha8Rng;
use rsa::rand_core::{RngCore, SeedableRng};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;

const WORDLIST_RAW: &str = include_str!("../resources/eff_large_wordlist.txt");

fn wordlist() -> &'static [&'static str] {
    static WORDS: OnceLock<Vec<&'static str>> = OnceLock::new();
    WORDS.get_or_init(|| {
        WORDLIST_RAW
            .lines()
            .filter_map(|l| l.split_whitespace().nth(1))
            .collect()
    })
}

pub struct Pseudonymizer {
    map: HashMap<Uuid, (String, String)>,
}

impl Pseudonymizer {
    /// Builds a pseudonymizer if the viewer is an external user. Returns
    /// `None` for Shibboleth/integration viewers so callers can skip the
    /// obfuscation work entirely.
    pub async fn for_viewer(
        db: &PgPool,
        viewer: &User,
        salt: &str,
    ) -> Result<Option<Self>, AppError> {
        if !viewer.eppn.starts_with("ext:") {
            return Ok(None);
        }
        let rows = minerva_db::queries::users::list_all(db).await?;

        let mut targets: Vec<(Uuid, String)> = rows
            .into_iter()
            .filter(|r| r.role != "admin" && r.id != viewer.id)
            .map(|r| (r.id, r.eppn))
            .collect();
        targets.sort_by(|a, b| a.1.cmp(&b.1));

        let words = wordlist();
        let mut used: HashSet<(usize, usize)> = HashSet::new();
        let mut map: HashMap<Uuid, (String, String)> = HashMap::new();

        for (id, eppn) in targets {
            let (a, b) = roll_pair(&eppn, salt, words.len(), &used);
            used.insert((a, b));
            let w1 = words[a];
            let w2 = words[b];
            let display = format!("{} {}", capitalize(w1), capitalize(w2));
            let domain = eppn.rsplit_once('@').map(|(_, d)| d).unwrap_or("anon");
            let new_eppn = format!("{}.{}@{}", w1, w2, domain);
            map.insert(id, (display, new_eppn));
        }

        Ok(Some(Self { map }))
    }

    /// Returns the pseudonym for `user_id` or the original identity if the
    /// user is an admin, the viewer, or unknown to the map.
    pub fn apply(
        &self,
        user_id: Uuid,
        eppn: Option<String>,
        display: Option<String>,
    ) -> (Option<String>, Option<String>) {
        match self.map.get(&user_id) {
            Some((d, p)) => (Some(p.clone()), Some(d.clone())),
            None => (eppn, display),
        }
    }
}

/// Option-friendly helper so handlers don't need to branch on
/// `Some(&Pseudonymizer)` vs `None`.
pub fn apply(
    ps: Option<&Pseudonymizer>,
    user_id: Uuid,
    eppn: Option<String>,
    display: Option<String>,
) -> (Option<String>, Option<String>) {
    match ps {
        Some(p) => p.apply(user_id, eppn, display),
        None => (eppn, display),
    }
}

fn roll_pair(eppn: &str, salt: &str, n: usize, used: &HashSet<(usize, usize)>) -> (usize, usize) {
    let mut attempt: u32 = 0;
    loop {
        let mut hasher = Sha256::new();
        hasher.update(salt.as_bytes());
        hasher.update(b"\0");
        hasher.update(eppn.as_bytes());
        hasher.update(attempt.to_le_bytes());
        let digest = hasher.finalize();
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&digest);
        let mut rng = ChaCha8Rng::from_seed(seed);
        let a = (rng.next_u32() as usize) % n;
        let b = (rng.next_u32() as usize) % n;
        if a != b && !used.contains(&(a, b)) {
            return (a, b);
        }
        attempt = attempt.wrapping_add(1);
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wordlist_parses() {
        let w = wordlist();
        assert_eq!(w.len(), 7776);
        assert_eq!(w[0], "abacus");
    }

    #[test]
    fn roll_is_deterministic() {
        let used = HashSet::new();
        let a = roll_pair("alice@su.se", "salt", 7776, &used);
        let b = roll_pair("alice@su.se", "salt", 7776, &used);
        assert_eq!(a, b);
    }

    #[test]
    fn roll_salt_shifts_output() {
        let used = HashSet::new();
        let a = roll_pair("alice@su.se", "salt-a", 7776, &used);
        let b = roll_pair("alice@su.se", "salt-b", 7776, &used);
        assert_ne!(a, b);
    }

    #[test]
    fn roll_avoids_collisions() {
        let mut used = HashSet::new();
        let a = roll_pair("alice@su.se", "salt", 7776, &used);
        used.insert(a);
        let b = roll_pair("alice@su.se", "salt", 7776, &used);
        assert_ne!(a, b);
    }
}
