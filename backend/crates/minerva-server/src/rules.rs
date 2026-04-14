use minerva_core::models::{RoleRuleCondition, RoleRuleWithConditions, RuleOperator, UserRole};
use std::collections::HashMap;

/// Attribute names admins may reference in rule conditions. Mirrors the
/// Shibboleth headers we read in auth.rs.
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

/// Returns the highest-ranked target_role across all rules whose
/// conditions ALL hold against `attrs`. Returns None if no rule matches.
/// Rules with no conditions never match (avoids accidental match-everything).
pub fn evaluate(
    rules: &[RoleRuleWithConditions],
    attrs: &HashMap<String, String>,
) -> Option<UserRole> {
    let mut best: Option<UserRole> = None;
    for rule in rules {
        if !rule.rule.enabled || rule.conditions.is_empty() {
            continue;
        }
        if rule.conditions.iter().all(|c| condition_holds(c, attrs)) {
            best = Some(match best {
                Some(prev) => max_role(prev, rule.rule.target_role),
                None => rule.rule.target_role,
            });
        }
    }
    best
}

fn condition_holds(cond: &RoleRuleCondition, attrs: &HashMap<String, String>) -> bool {
    let header_value = attrs
        .get(cond.attribute.as_str())
        .map(String::as_str)
        .unwrap_or("");
    match cond.operator {
        RuleOperator::Contains => contains_value(header_value, &cond.value),
        RuleOperator::NotContains => !contains_value(header_value, &cond.value),
        RuleOperator::Regex => regex_matches(&cond.value, header_value),
        RuleOperator::NotRegex => !regex_matches(&cond.value, header_value),
    }
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

fn regex_matches(pattern: &str, value: &str) -> bool {
    regex::Regex::new(pattern)
        .map(|re| re.is_match(value))
        .unwrap_or(false)
}

/// Validates a regex string at rule-condition save time so admins get a
/// clear 400 instead of a silently-broken rule.
pub fn validate_regex(pattern: &str) -> Result<(), String> {
    regex::Regex::new(pattern)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn max_role(a: UserRole, b: UserRole) -> UserRole {
    fn rank(r: UserRole) -> u8 {
        match r {
            UserRole::Student => 0,
            UserRole::Teacher => 1,
            UserRole::Admin => 2,
        }
    }
    if rank(a) >= rank(b) {
        a
    } else {
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn rule(
        target: UserRole,
        enabled: bool,
        conds: Vec<(&str, RuleOperator, &str)>,
    ) -> RoleRuleWithConditions {
        let rule_id = Uuid::new_v4();
        RoleRuleWithConditions {
            rule: minerva_core::models::RoleRule {
                id: rule_id,
                name: "test".into(),
                target_role: target,
                enabled,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
            conditions: conds
                .into_iter()
                .map(|(attr, op, val)| RoleRuleCondition {
                    id: Uuid::new_v4(),
                    rule_id,
                    attribute: attr.into(),
                    operator: op,
                    value: val.into(),
                    created_at: Utc::now(),
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
            true,
            vec![("affiliation", RuleOperator::Contains, "student@su.se")],
        );
        assert_eq!(
            evaluate(
                std::slice::from_ref(&r),
                &attrs(&[("affiliation", "student@su.se;employee@su.se")])
            ),
            Some(UserRole::Teacher),
        );
        // Substring match is rejected -- "student@su.se" is NOT a member of
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
            true,
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

    #[test]
    fn disabled_rule_is_skipped() {
        let r = rule(
            UserRole::Teacher,
            false,
            vec![("eppn", RuleOperator::Contains, "edsu8469@su.se")],
        );
        assert_eq!(evaluate(&[r], &attrs(&[("eppn", "edsu8469@su.se")])), None,);
    }

    #[test]
    fn regex_and_not_regex() {
        let r = rule(
            UserRole::Teacher,
            true,
            vec![("eppn", RuleOperator::Regex, r"^[a-z]+\d+@su\.se$")],
        );
        assert_eq!(
            evaluate(&[r], &attrs(&[("eppn", "edsu8469@su.se")])),
            Some(UserRole::Teacher),
        );

        let neg = rule(
            UserRole::Teacher,
            true,
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
    fn empty_conditions_never_match() {
        let r = rule(UserRole::Teacher, true, vec![]);
        assert_eq!(evaluate(&[r], &attrs(&[("eppn", "anything")])), None);
    }
}
