use minerva_core::models::{RuleOperator, UserRole};
use regex::Regex;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Attribute names admins may reference in rule conditions. Mirrors the
/// Shibboleth headers we read in auth.rs. The frontend equivalent is
/// `ROLE_RULE_ATTRIBUTES` in `frontend/src/lib/types.ts`; keep them in
/// sync (or hop both behind a generated schema if this grows).
pub const SUPPORTED_ATTRIBUTES: &[&str] = &[
    "eppn",
    "displayName",
    "affiliation",
    "entitlement",
    "mail",
    "cn",
    "sn",
    "givenName",
];

/// Pre-compiled rule, ready for eval. Built from DB rows by
/// `RuleCache::reload` so each request avoids per-rule SQL + per-condition
/// `Regex::new`. Disabled rules and rules with no conditions are dropped at
/// compile time; if it's in the cache it's a candidate to fire.
#[derive(Debug, Clone)]
pub struct CompiledRule {
    pub target_role: UserRole,
    pub conditions: Vec<CompiledCondition>,
}

#[derive(Debug, Clone)]
pub enum CompiledCondition {
    Contains { attribute: String, value: String },
    NotContains { attribute: String, value: String },
    Regex { attribute: String, regex: Regex },
    NotRegex { attribute: String, regex: Regex },
}

impl CompiledCondition {
    fn attribute(&self) -> &str {
        match self {
            Self::Contains { attribute, .. }
            | Self::NotContains { attribute, .. }
            | Self::Regex { attribute, .. }
            | Self::NotRegex { attribute, .. } => attribute,
        }
    }

    fn matches(&self, attrs: &HashMap<String, String>) -> bool {
        // Negated operators require the attribute to be PRESENT (see
        // CompiledCondition variant docs). Otherwise an external user with
        // no `affiliation` header would match `affiliation not_contains
        // alumni -> teacher` simply by lacking affiliation; definitely
        // not the admin's intent.
        let Some(header_value) = attrs.get(self.attribute()).map(String::as_str) else {
            return false;
        };
        match self {
            Self::Contains { value, .. } => contains_value(header_value, value),
            Self::NotContains { value, .. } => !contains_value(header_value, value),
            Self::Regex { regex, .. } => regex.is_match(header_value),
            Self::NotRegex { regex, .. } => !regex.is_match(header_value),
        }
    }
}

/// In-memory cache of compiled rules. Loaded once at startup and refreshed
/// after every admin mutation (rule create/update/delete + condition
/// create/delete); see `routes/admin.rs`. Reads are O(rules*conds) over
/// already-compiled regexes; writes are rare. We hand out an Arc snapshot
/// so the auth middleware can drop the read lock immediately.
pub struct RuleCache {
    inner: RwLock<Arc<Vec<CompiledRule>>>,
}

impl RuleCache {
    pub async fn load(db: &PgPool) -> Result<Self, sqlx::Error> {
        let compiled = compile_from_db(db).await?;
        Ok(Self {
            inner: RwLock::new(Arc::new(compiled)),
        })
    }

    /// Replace the cached compiled rules. Called after any admin write
    /// path; failure here surfaces as 500 so the admin retries.
    pub async fn reload(&self, db: &PgPool) -> Result<(), sqlx::Error> {
        let compiled = compile_from_db(db).await?;
        let mut guard = self.inner.write().await;
        *guard = Arc::new(compiled);
        Ok(())
    }

    /// Cheap snapshot for read paths; clones an Arc, doesn't lock for
    /// the duration of evaluation.
    pub async fn snapshot(&self) -> Arc<Vec<CompiledRule>> {
        self.inner.read().await.clone()
    }
}

async fn compile_from_db(db: &PgPool) -> Result<Vec<CompiledRule>, sqlx::Error> {
    let rule_rows = minerva_db::queries::role_rules::list_enabled(db).await?;
    if rule_rows.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<uuid::Uuid> = rule_rows.iter().map(|r| r.id).collect();
    let cond_rows = minerva_db::queries::role_rules::list_conditions_for_rules(db, &ids).await?;

    let mut by_rule: HashMap<uuid::Uuid, Vec<CompiledCondition>> = HashMap::new();
    for c in cond_rows {
        let Some(op) = RuleOperator::parse(&c.operator) else {
            tracing::warn!(rule = %c.rule_id, op = %c.operator, "skipping condition with unknown operator");
            continue;
        };
        let compiled = match op {
            RuleOperator::Contains => CompiledCondition::Contains {
                attribute: c.attribute,
                value: c.value,
            },
            RuleOperator::NotContains => CompiledCondition::NotContains {
                attribute: c.attribute,
                value: c.value,
            },
            RuleOperator::Regex => match Regex::new(&c.value) {
                Ok(re) => CompiledCondition::Regex {
                    attribute: c.attribute,
                    regex: re,
                },
                Err(e) => {
                    // We validate at save time so this should never fire,
                    // but a corrupt row shouldn't bring down auth.
                    tracing::error!(rule = %c.rule_id, error = %e, "skipping condition with invalid regex");
                    continue;
                }
            },
            RuleOperator::NotRegex => match Regex::new(&c.value) {
                Ok(re) => CompiledCondition::NotRegex {
                    attribute: c.attribute,
                    regex: re,
                },
                Err(e) => {
                    tracing::error!(rule = %c.rule_id, error = %e, "skipping condition with invalid regex");
                    continue;
                }
            },
        };
        by_rule.entry(c.rule_id).or_default().push(compiled);
    }

    Ok(rule_rows
        .into_iter()
        .filter_map(|r| {
            let conditions = by_rule.remove(&r.id).unwrap_or_default();
            // Drop empty-condition rules at compile time; they could only
            // ever match-everything and we'd rather they no-op silently.
            if conditions.is_empty() {
                return None;
            }
            Some(CompiledRule {
                target_role: UserRole::parse(&r.target_role),
                conditions,
            })
        })
        .collect())
}

/// Returns the highest-ranked target_role across all rules whose
/// conditions ALL hold against `attrs`. Returns None if no rule matches.
/// Empty-condition rules never match; otherwise `iter().all()` on an
/// empty slice would return true and the rule would silently promote
/// every user. The cache layer also filters these out at compile time;
/// this is belt + braces.
pub fn evaluate(rules: &[CompiledRule], attrs: &HashMap<String, String>) -> Option<UserRole> {
    let mut best: Option<UserRole> = None;
    for rule in rules {
        if rule.conditions.is_empty() {
            continue;
        }
        if rule.conditions.iter().all(|c| c.matches(attrs)) {
            best = Some(match best {
                Some(prev) => UserRole::max(prev, rule.target_role),
                None => rule.target_role,
            });
        }
    }
    best
}

/// Multi-valued Shib headers arrive as `value1;value2;value3` when
/// `ShibUseHeaders On` is set. `contains` checks list membership rather
/// than substring, so `affiliation contains student@su.se` matches
/// `student@su.se;employee@su.se` but not `prefix-student@su.se`.
fn contains_value(header_value: &str, needle: &str) -> bool {
    if header_value.is_empty() {
        return false;
    }
    header_value.split(';').any(|v| v == needle)
}

/// Validates a regex string at rule-condition save time so admins get a
/// clear 400 instead of a silently-broken rule.
pub fn validate_regex(pattern: &str) -> Result<(), String> {
    Regex::new(pattern).map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(target: UserRole, conds: Vec<(&str, RuleOperator, &str)>) -> CompiledRule {
        CompiledRule {
            target_role: target,
            conditions: conds
                .into_iter()
                .map(|(attr, op, val)| match op {
                    RuleOperator::Contains => CompiledCondition::Contains {
                        attribute: attr.into(),
                        value: val.into(),
                    },
                    RuleOperator::NotContains => CompiledCondition::NotContains {
                        attribute: attr.into(),
                        value: val.into(),
                    },
                    RuleOperator::Regex => CompiledCondition::Regex {
                        attribute: attr.into(),
                        regex: Regex::new(val).expect("test regex compiles"),
                    },
                    RuleOperator::NotRegex => CompiledCondition::NotRegex {
                        attribute: attr.into(),
                        regex: Regex::new(val).expect("test regex compiles"),
                    },
                })
                .collect(),
        }
    }

    fn attrs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn contains_matches_list_member_not_substring() {
        let r = rule(
            UserRole::Teacher,
            vec![("affiliation", RuleOperator::Contains, "student@su.se")],
        );
        assert_eq!(
            evaluate(
                std::slice::from_ref(&r),
                &attrs(&[("affiliation", "student@su.se;employee@su.se")])
            ),
            Some(UserRole::Teacher),
        );
        // Substring match is rejected; "student@su.se" is NOT a member of
        // the single-element list "prefix-student@su.se".
        assert_eq!(
            evaluate(&[r], &attrs(&[("affiliation", "prefix-student@su.se")])),
            None,
        );
    }

    #[test]
    fn and_composition_requires_all_conditions() {
        let r = rule(
            UserRole::Teacher,
            vec![
                ("affiliation", RuleOperator::Contains, "employee@su.se"),
                (
                    "entitlement",
                    RuleOperator::Contains,
                    "urn:mace:swami.se:gmai:dsv-user:staff",
                ),
            ],
        );
        // Both present -> match
        assert_eq!(
            evaluate(
                std::slice::from_ref(&r),
                &attrs(&[
                    ("affiliation", "employee@su.se"),
                    ("entitlement", "urn:mace:swami.se:gmai:dsv-user:staff;other"),
                ]),
            ),
            Some(UserRole::Teacher),
        );
        // Only one -> no match
        assert_eq!(
            evaluate(&[r], &attrs(&[("affiliation", "employee@su.se")])),
            None,
        );
    }

    // (Disabled-rule and empty-condition handling now happens at the
    // RuleCache::compile_from_db layer; those rules never enter the
    // CompiledRule slice. The defensive empty-condition skip in
    // `evaluate` is there too, but isn't worth a unit test on its own.)

    #[test]
    fn regex_and_not_regex() {
        let r = rule(
            UserRole::Teacher,
            vec![("eppn", RuleOperator::Regex, r"^[a-z]+\d+@su\.se$")],
        );
        assert_eq!(
            evaluate(&[r], &attrs(&[("eppn", "edsu8469@su.se")])),
            Some(UserRole::Teacher),
        );

        let neg = rule(
            UserRole::Teacher,
            vec![("eppn", RuleOperator::NotRegex, r"@dsv\.su\.se$")],
        );
        assert_eq!(
            evaluate(
                std::slice::from_ref(&neg),
                &attrs(&[("eppn", "edsu8469@su.se")])
            ),
            Some(UserRole::Teacher),
        );
        assert_eq!(evaluate(&[neg], &attrs(&[("eppn", "x@dsv.su.se")])), None,);
    }

    #[test]
    fn negated_ops_require_attribute_present() {
        // Without this guard, an external user with no `affiliation`
        // header would match `affiliation not_contains alumni` and get
        // wrongly promoted. Negated ops must require the attribute exists.
        let nc = rule(
            UserRole::Teacher,
            vec![("affiliation", RuleOperator::NotContains, "alumni@su.se")],
        );
        // Header missing entirely -> rule does NOT match.
        assert_eq!(
            evaluate(
                std::slice::from_ref(&nc),
                &attrs(&[("eppn", "ext:partner@example.org")])
            ),
            None,
        );
        // Header present and value not in list -> rule MATCHES.
        assert_eq!(
            evaluate(
                &[nc],
                &attrs(&[("affiliation", "employee@su.se;member@su.se")])
            ),
            Some(UserRole::Teacher),
        );

        // Same semantic for not_regex.
        let nr = rule(
            UserRole::Teacher,
            vec![("entitlement", RuleOperator::NotRegex, "alumni")],
        );
        assert_eq!(
            evaluate(std::slice::from_ref(&nr), &attrs(&[("eppn", "x@su.se")])),
            None,
        );
        assert_eq!(
            evaluate(
                &[nr],
                &attrs(&[("entitlement", "urn:mace:swami.se:gmai:dsv-user:staff")])
            ),
            Some(UserRole::Teacher),
        );
    }

    #[test]
    fn user_role_max_picks_higher_rank() {
        assert_eq!(
            UserRole::max(UserRole::Student, UserRole::Teacher),
            UserRole::Teacher
        );
        assert_eq!(
            UserRole::max(UserRole::Admin, UserRole::Teacher),
            UserRole::Admin
        );
        assert_eq!(
            UserRole::max(UserRole::Student, UserRole::Student),
            UserRole::Student
        );
    }

    #[test]
    fn clamp_below_admin_demotes_admin_only() {
        assert_eq!(UserRole::Admin.clamp_below_admin(), UserRole::Teacher);
        assert_eq!(UserRole::Teacher.clamp_below_admin(), UserRole::Teacher);
        assert_eq!(UserRole::Student.clamp_below_admin(), UserRole::Student);
    }
}
