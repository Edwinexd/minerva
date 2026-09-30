//! Which Minerva user an LTI person is. Shared by the launch handler and the
//! NRPS roster sync: both must land on the same account for the same person,
//! so the order of identities and the account lookup live here only.

use minerva_db::queries::users::UserRow;
use rust_decimal::Decimal;
use sqlx::PgPool;

/// The identities an LMS shared for one person, most trusted first:
///   a) custom `user_eppn` param, b) Moodle's username (`ext.user_username`
///      on a launch, `ext_user_username` in a roster; an eppn at DSV),
///   c) email, d) synthetic `lti_<source_id>_<sub>`.
/// Empty values are dropped per source, so a blank username still falls back
/// to the email. Everything is lowercased.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LtiIdentity {
    claimed: Vec<String>,
    synthetic: String,
}

impl LtiIdentity {
    pub fn new(
        user_eppn: Option<&str>,
        username: Option<&str>,
        email: Option<&str>,
        source_identifier: &str,
        sub: &str,
    ) -> Self {
        let mut claimed: Vec<String> = Vec::new();
        for value in [user_eppn, username, email].into_iter().flatten() {
            let value = value.trim().to_lowercase();
            if !value.is_empty() && !claimed.contains(&value) {
                claimed.push(value);
            }
        }
        Self {
            claimed,
            synthetic: format!("lti_{}_{}", source_identifier, sub).to_lowercase(),
        }
    }

    /// Drop the claims outside a platform's eppn scope, so the strongest one
    /// left is the person's identity: a username the scope excludes still
    /// leaves an in-scope email to go by. `Err` carries the strongest claim
    /// when the LMS shared identities and none is in scope; that person
    /// must not be resolved at all. The synthetic form is tagged with the
    /// source id, trivially distinct from any real eppn, and needs no scope
    /// check.
    pub fn within_scope(mut self, allowed_domains: Option<&[String]>) -> Result<Self, String> {
        let Some(strongest) = self.claimed.first().cloned() else {
            return Ok(self);
        };
        self.claimed
            .retain(|eppn| eppn_in_allowed_domains(allowed_domains, eppn));
        if self.claimed.is_empty() {
            return Err(strongest);
        }
        Ok(self)
    }

    /// The strongest identity the LMS shared, or `None` when it shared
    /// nothing.
    pub fn primary_claim(&self) -> Option<&str> {
        self.claimed.first().map(String::as_str)
    }

    /// The eppn a brand-new account for this person is created under.
    pub fn primary(&self) -> &str {
        self.primary_claim().unwrap_or(&self.synthetic)
    }

    /// Weaker identities the same person may already have an account under,
    /// strongest first.
    fn fallbacks(&self) -> impl Iterator<Item = &str> {
        self.claimed
            .iter()
            .skip(1)
            .map(String::as_str)
            .chain((!self.claimed.is_empty()).then_some(self.synthetic.as_str()))
    }
}

/// Whether `eppn` is inside a platform's eppn scope. No scope (or an empty
/// one) allows everything. `@<domain>` suffix, not substring, so
/// `x@su.se.evil.org` does not pass for `su.se`.
pub fn eppn_in_allowed_domains(allowed_domains: Option<&[String]>, eppn: &str) -> bool {
    let Some(domains) = allowed_domains.filter(|d| !d.is_empty()) else {
        return true;
    };
    domains
        .iter()
        .any(|d| eppn.ends_with(&format!("@{}", d.to_lowercase())))
}

/// Find this person's account, or create one under their primary identity.
///
/// One person keeps one account even when the LMS starts sharing a stronger
/// identity than the one their account was created under (a tool that gained
/// the `user_eppn` param or name sharing later): the existing account is
/// reused and the stronger identity is registered as its alias, so later
/// lookups, including a direct Shibboleth login, resolve to it too.
///
/// The caller must have passed the identity through `within_scope` for a
/// scoped platform. Existing accounts are never modified beyond the alias.
pub async fn resolve_user(
    db: &PgPool,
    identity: &LtiIdentity,
    display_name: Option<&str>,
    default_owner_daily_cost_limit_usd: Decimal,
) -> Result<UserRow, sqlx::Error> {
    let primary = identity.primary();
    let (user, _created) = minerva_db::queries::users::find_or_create_by_any_eppn(
        db,
        primary,
        identity.fallbacks(),
        display_name,
        "student",
        default_owner_daily_cost_limit_usd,
    )
    .await?;
    if user.eppn != primary {
        if let Err(e) = minerva_db::queries::user_eppn_aliases::register(db, user.id, primary).await
        {
            tracing::warn!(
                "lti identity: could not alias {} to user {}: {}",
                primary,
                user.id,
                e
            );
        }
    }
    Ok(user)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "f2e650d3-fa5b-452e-b300-3ef9f6491186";

    fn su_scope() -> Vec<String> {
        vec!["su.se".into()]
    }

    #[test]
    fn username_outranks_email() {
        let id = LtiIdentity::new(
            None,
            Some("ABCD1234@su.se"),
            Some("abcd1234@student.su.se"),
            SOURCE,
            "9404",
        );
        assert_eq!(id.primary_claim(), Some("abcd1234@su.se"));
        assert_eq!(id.primary(), "abcd1234@su.se");
    }

    #[test]
    fn custom_user_eppn_outranks_username() {
        let id = LtiIdentity::new(
            Some("efgh5678@su.se"),
            Some("abcd1234@su.se"),
            None,
            SOURCE,
            "9404",
        );
        assert_eq!(id.primary(), "efgh5678@su.se");
    }

    #[test]
    fn empty_sources_fall_through_to_the_next() {
        let id = LtiIdentity::new(Some(""), Some("  "), Some("a@su.se"), SOURCE, "9404");
        assert_eq!(id.primary_claim(), Some("a@su.se"));
    }

    #[test]
    fn nothing_shared_is_synthetic() {
        let id = LtiIdentity::new(None, Some(""), None, SOURCE, "9404");
        assert_eq!(id.primary_claim(), None);
        assert_eq!(id.primary(), format!("lti_{}_9404", SOURCE));
        assert_eq!(id.fallbacks().count(), 0);
    }

    #[test]
    fn fallbacks_are_the_weaker_identities_in_order() {
        let id = LtiIdentity::new(
            Some("a@su.se"),
            Some("a@su.se"),
            Some("a@dsv.su.se"),
            SOURCE,
            "9404",
        );
        let synthetic = format!("lti_{}_9404", SOURCE);
        assert_eq!(
            id.fallbacks().collect::<Vec<_>>(),
            vec!["a@dsv.su.se", synthetic.as_str()]
        );
    }

    #[test]
    fn out_of_scope_fallbacks_are_dropped() {
        let id = LtiIdentity::new(
            None,
            Some("abcd1234@su.se"),
            Some("abcd1234@student.su.se"),
            SOURCE,
            "9404",
        )
        .within_scope(Some(&su_scope()))
        .unwrap();
        let synthetic = format!("lti_{}_9404", SOURCE);
        assert_eq!(id.primary(), "abcd1234@su.se");
        assert_eq!(id.fallbacks().collect::<Vec<_>>(), vec![synthetic.as_str()]);
    }

    #[test]
    fn out_of_scope_username_leaves_the_in_scope_email() {
        let id = LtiIdentity::new(None, Some("teacher1"), Some("t@su.se"), SOURCE, "9404")
            .within_scope(Some(&su_scope()))
            .unwrap();
        assert_eq!(id.primary_claim(), Some("t@su.se"));
    }

    #[test]
    fn nothing_in_scope_is_refused_with_the_strongest_claim() {
        let refused = LtiIdentity::new(
            None,
            Some("abcd1234@kth.se"),
            Some("abcd1234@student.su.se"),
            SOURCE,
            "9404",
        )
        .within_scope(Some(&su_scope()));
        assert_eq!(refused, Err("abcd1234@kth.se".to_string()));
    }

    #[test]
    fn synthetic_identity_is_exempt_from_scope() {
        let id = LtiIdentity::new(None, None, None, SOURCE, "9404")
            .within_scope(Some(&su_scope()))
            .unwrap();
        assert_eq!(id.primary_claim(), None);
    }

    #[test]
    fn scope_is_a_domain_suffix_match() {
        let scope = su_scope();
        assert!(eppn_in_allowed_domains(Some(&scope), "a@su.se"));
        assert!(!eppn_in_allowed_domains(Some(&scope), "a@student.su.se"));
        assert!(!eppn_in_allowed_domains(Some(&scope), "a@su.se.evil.org"));
        assert!(eppn_in_allowed_domains(None, "a@anywhere.org"));
        assert!(eppn_in_allowed_domains(Some(&[]), "a@anywhere.org"));
    }
}
