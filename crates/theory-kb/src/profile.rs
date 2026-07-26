//! Profile inheritance, override merging and score-weight normalisation.
//!
//! A [`StyleProfile`] as authored is only half a grammar: it may inherit from a
//! parent, its weights are un-normalised, and its `rule_overrides` are additive
//! on top of its ancestors'. [`ResolvedProfile`] is the fully materialised
//! form with no unresolved references, which is what the rule engine and the
//! scoring layer actually consume.
//!
//! Resolution walks from the **root down to the leaf** so the child always
//! wins, exactly as `docs/THEORY_MODEL.md` §4 specifies.

use crate::error::KbError;
use crate::model::{RuleOverride, StyleProfile, TheoryRule};
use music_domain::candidate::SCORE_COMPONENTS;
use qjson::{Json, JsonMap};
use std::collections::BTreeMap;

/// A profile with its parent chain flattened and its weights normalised.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedProfile {
    /// The leaf profile's id.
    pub id: String,
    /// `self -> parent -> … -> root`.
    pub chain: Vec<String>,
    /// Normalised weights; all thirteen `SCORE_COMPONENTS` are present and the
    /// values sum to `1.0`.
    pub weights: BTreeMap<String, f64>,
    /// Every scalar and structural field, child overriding parent.
    pub fields: JsonMap,
    /// Every rule override, child overriding parent.
    pub overrides: BTreeMap<String, RuleOverride>,
}

impl ResolvedProfile {
    /// The normalised weight of one score component; `0.0` if unknown.
    pub fn weight(&self, component: &str) -> f64 {
        self.weights.get(component).copied().unwrap_or(0.0)
    }

    /// The multiplier to apply to a rule's score delta. `0.0` when the profile
    /// disables the rule, `1.0` when it says nothing about it.
    pub fn rule_multiplier(&self, rule_id: &str) -> f64 {
        match self.overrides.get(rule_id) {
            Some(RuleOverride::Disabled) => 0.0,
            Some(RuleOverride::Multiplier(m)) => *m,
            None => 1.0,
        }
    }

    /// False only when the profile explicitly disables the rule. A multiplier
    /// of `0.0` still counts as enabled: the trace reports it as `applied`
    /// with no effect, which is different information from `not_applicable`.
    pub fn is_rule_enabled(&self, rule_id: &str) -> bool {
        !matches!(self.overrides.get(rule_id), Some(RuleOverride::Disabled))
    }

    /// A flattened field as a number. Dotted keys reach into nested objects:
    /// `"register_spacing.open_voicing_bias"`.
    pub fn field_f64(&self, key: &str) -> Option<f64> {
        self.field(key).and_then(Json::as_f64)
    }

    /// A flattened field as a string, with the same dotted-key support.
    pub fn field_str(&self, key: &str) -> Option<&str> {
        self.field(key).and_then(Json::as_str)
    }

    /// A flattened field as a boolean, with the same dotted-key support.
    pub fn field_bool(&self, key: &str) -> Option<bool> {
        self.field(key).and_then(Json::as_bool)
    }

    /// A flattened field as raw JSON, resolving dotted paths.
    pub fn field(&self, key: &str) -> Option<&Json> {
        let mut parts = key.split('.');
        let mut cur = self.fields.get(parts.next()?)?;
        for p in parts {
            cur = cur.get(p)?;
        }
        Some(cur)
    }

    /// True when the rule declares any profile in this chain, i.e. the rule is
    /// active for this profile or for one of its ancestors.
    pub fn applies_to(&self, rule: &TheoryRule) -> bool {
        rule.profiles
            .iter()
            .any(|p| self.chain.iter().any(|c| c == p))
    }

    /// JSON form, powering `theory://profiles/{id}`.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("id", Json::Str(self.id.clone()));
        m.insert(
            "chain",
            Json::Arr(self.chain.iter().map(|c| Json::Str(c.clone())).collect()),
        );
        let mut w = JsonMap::new();
        for c in SCORE_COMPONENTS {
            w.insert(*c, Json::Float(self.weight(c)));
        }
        m.insert("weights", Json::Obj(w));
        m.insert("fields", Json::Obj(self.fields.clone()));
        let mut o = JsonMap::new();
        for (k, v) in &self.overrides {
            o.insert(k.clone(), v.to_json());
        }
        m.insert("rule_overrides", Json::Obj(o));
        Json::Obj(m)
    }
}

/// Walks `id` up to its root, returning `self -> … -> root`.
///
/// Fails on a missing parent and on any cycle, including a self-parent.
pub fn ancestry(profiles: &[StyleProfile], id: &str) -> Result<Vec<String>, KbError> {
    let mut chain: Vec<String> = Vec::new();
    let mut cursor = id.to_string();
    loop {
        if chain.iter().any(|c| *c == cursor) {
            chain.push(cursor.clone());
            return Err(KbError::profile_cycle(
                format!("knowledge/profiles/{id}.json"),
                format!("profile inheritance forms a cycle: {}", chain.join(" -> ")),
            ));
        }
        let p = profiles.iter().find(|p| p.id == cursor).ok_or_else(|| {
            if chain.is_empty() {
                KbError::not_found(
                    format!("knowledge/profiles/{cursor}.json"),
                    format!("no style profile with id '{cursor}'"),
                )
            } else {
                KbError::missing_parent(
                    format!("knowledge/profiles/{}.json", chain[chain.len() - 1]),
                    format!("parent profile '{cursor}' does not exist"),
                )
            }
        })?;
        chain.push(cursor.clone());
        match &p.parent {
            Some(parent) => cursor = parent.clone(),
            None => return Ok(chain),
        }
        if chain.len() > profiles.len() + 1 {
            return Err(KbError::profile_cycle(
                format!("knowledge/profiles/{id}.json"),
                format!("profile inheritance forms a cycle: {}", chain.join(" -> ")),
            ));
        }
    }
}

/// Flattens `id`'s parent chain into a [`ResolvedProfile`].
///
/// Fields, weights and overrides are merged root-first so a descendant always
/// wins. Weights are then normalised across the thirteen `SCORE_COMPONENTS` so
/// they sum to exactly `1.0`; a profile that zeroes every weight falls back to
/// a uniform distribution rather than producing a division by zero.
pub fn resolve(profiles: &[StyleProfile], id: &str) -> Result<ResolvedProfile, KbError> {
    let chain = ancestry(profiles, id)?;

    let mut fields = JsonMap::new();
    let mut raw_weights: BTreeMap<String, f64> = BTreeMap::new();
    let mut overrides: BTreeMap<String, RuleOverride> = BTreeMap::new();

    // Root first, leaf last: every later write wins.
    for name in chain.iter().rev() {
        let p = profiles
            .iter()
            .find(|p| p.id == *name)
            .ok_or_else(|| KbError::missing_parent("knowledge/profiles", name.clone()))?;
        if let Some(obj) = p.raw.as_obj() {
            for (k, v) in obj.iter() {
                if k != "score_weights" && k != "rule_overrides" {
                    fields.insert(k, v.clone());
                }
            }
        }
        for (k, v) in &p.score_weights {
            raw_weights.insert(k.clone(), *v);
        }
        for (k, v) in &p.rule_overrides {
            overrides.insert(k.clone(), *v);
        }
    }

    let mut weights = BTreeMap::new();
    let total: f64 = SCORE_COMPONENTS
        .iter()
        .map(|c| raw_weights.get(*c).copied().unwrap_or(0.0).max(0.0))
        .sum();
    let n = SCORE_COMPONENTS.len() as f64;
    for c in SCORE_COMPONENTS {
        let raw = raw_weights.get(*c).copied().unwrap_or(0.0).max(0.0);
        let w = if total > 0.0 { raw / total } else { 1.0 / n };
        weights.insert((*c).to_string(), w);
    }

    Ok(ResolvedProfile {
        id: id.to_string(),
        chain,
        weights,
        fields,
        overrides,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RuleDomain;
    use qjson::json_obj;

    fn profile(
        id: &str,
        parent: Option<&str>,
        weights: &[(&str, f64)],
        ov: &[(&str, RuleOverride)],
        extra: &[(&str, Json)],
    ) -> StyleProfile {
        let mut raw = JsonMap::new();
        raw.insert("id", Json::Str(id.into()));
        raw.insert(
            "parent",
            parent.map(|p| Json::Str(p.into())).unwrap_or(Json::Null),
        );
        for (k, v) in extra {
            raw.insert(*k, v.clone());
        }
        StyleProfile {
            id: id.into(),
            name: id.into(),
            parent: parent.map(str::to_string),
            summary: String::new(),
            score_weights: weights
                .iter()
                .map(|(k, v)| ((*k).to_string(), *v))
                .collect(),
            rule_overrides: ov.iter().map(|(k, v)| ((*k).to_string(), *v)).collect(),
            notes: String::new(),
            raw: Json::Obj(raw),
        }
    }

    fn uniform() -> Vec<(&'static str, f64)> {
        SCORE_COMPONENTS.iter().map(|c| (*c, 1.0)).collect()
    }

    #[test]
    fn ancestry_runs_leaf_to_root() {
        let ps = vec![
            profile("root", None, &uniform(), &[], &[]),
            profile("mid", Some("root"), &uniform(), &[], &[]),
            profile("leaf", Some("mid"), &uniform(), &[], &[]),
        ];
        assert_eq!(ancestry(&ps, "leaf").unwrap(), vec!["leaf", "mid", "root"]);
        assert_eq!(ancestry(&ps, "root").unwrap(), vec!["root"]);
    }

    #[test]
    fn ancestry_detects_a_two_node_cycle() {
        let ps = vec![
            profile("a", Some("b"), &uniform(), &[], &[]),
            profile("b", Some("a"), &uniform(), &[], &[]),
        ];
        let e = ancestry(&ps, "a").unwrap_err();
        assert_eq!(e.code, "KB_PROFILE_CYCLE");
        assert!(e.message.contains("a -> b -> a"), "{}", e.message);
    }

    #[test]
    fn ancestry_detects_a_self_cycle() {
        let ps = vec![profile("a", Some("a"), &uniform(), &[], &[])];
        assert_eq!(ancestry(&ps, "a").unwrap_err().code, "KB_PROFILE_CYCLE");
    }

    #[test]
    fn ancestry_reports_a_missing_parent() {
        let ps = vec![profile("a", Some("ghost"), &uniform(), &[], &[])];
        let e = ancestry(&ps, "a").unwrap_err();
        assert_eq!(e.code, "KB_MISSING_PARENT");
        assert!(e.message.contains("ghost"));
    }

    #[test]
    fn ancestry_reports_an_unknown_leaf() {
        let ps = vec![profile("a", None, &uniform(), &[], &[])];
        assert_eq!(ancestry(&ps, "nope").unwrap_err().code, "KB_NOT_FOUND");
    }

    #[test]
    fn child_fields_win_over_parent_fields() {
        let ps = vec![
            profile(
                "root",
                None,
                &uniform(),
                &[],
                &[
                    ("modal_tolerance", Json::Float(0.2)),
                    ("only_root", Json::Bool(true)),
                ],
            ),
            profile(
                "leaf",
                Some("root"),
                &uniform(),
                &[],
                &[("modal_tolerance", Json::Float(0.9))],
            ),
        ];
        let r = resolve(&ps, "leaf").unwrap();
        assert_eq!(r.field_f64("modal_tolerance"), Some(0.9));
        assert_eq!(r.field_bool("only_root"), Some(true));
        assert_eq!(r.field_str("id"), Some("leaf"));
    }

    #[test]
    fn child_weights_replace_parent_weights_per_key() {
        let mut parent = uniform();
        let mut child: Vec<(&str, f64)> = Vec::new();
        parent[0] = ("melody_fit", 5.0);
        child.push(("melody_fit", 1.0));
        let ps = vec![
            profile("root", None, &parent, &[], &[]),
            profile("leaf", Some("root"), &child, &[], &[]),
        ];
        let r = resolve(&ps, "leaf").unwrap();
        // 12 inherited weights of 1.0 plus the child's 1.0 => uniform.
        assert!((r.weight("melody_fit") - 1.0 / 13.0).abs() < 1e-12);
    }

    #[test]
    fn weights_are_normalised_to_one() {
        let scaled: Vec<(&str, f64)> = SCORE_COMPONENTS
            .iter()
            .enumerate()
            .map(|(i, c)| (*c, (i + 1) as f64))
            .collect();
        let ps = vec![profile("a", None, &scaled, &[], &[])];
        let r = resolve(&ps, "a").unwrap();
        let sum: f64 = r.weights.values().sum();
        assert!((sum - 1.0).abs() < 1e-12, "sum was {sum}");
        assert_eq!(r.weights.len(), 13);
        // 91 = 1+2+…+13
        assert!((r.weight("melody_fit") - 1.0 / 91.0).abs() < 1e-12);
    }

    #[test]
    fn all_zero_weights_fall_back_to_uniform() {
        let zeros: Vec<(&str, f64)> = SCORE_COMPONENTS.iter().map(|c| (*c, 0.0)).collect();
        let ps = vec![profile("a", None, &zeros, &[], &[])];
        let r = resolve(&ps, "a").unwrap();
        assert!((r.weights.values().sum::<f64>() - 1.0).abs() < 1e-12);
        assert!((r.weight("voice_leading") - 1.0 / 13.0).abs() < 1e-12);
    }

    #[test]
    fn a_zero_weight_survives_normalisation_as_zero() {
        let mut w = uniform();
        w[3] = ("voice_leading", 0.0);
        let ps = vec![profile("a", None, &w, &[], &[])];
        let r = resolve(&ps, "a").unwrap();
        assert_eq!(r.weight("voice_leading"), 0.0);
        assert!((r.weights.values().sum::<f64>() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn overrides_merge_with_the_child_winning() {
        let ps = vec![
            profile(
                "root",
                None,
                &uniform(),
                &[
                    ("a.one", RuleOverride::Multiplier(2.0)),
                    ("a.two", RuleOverride::Disabled),
                ],
                &[],
            ),
            profile(
                "leaf",
                Some("root"),
                &uniform(),
                &[("a.two", RuleOverride::Multiplier(0.5))],
                &[],
            ),
        ];
        let r = resolve(&ps, "leaf").unwrap();
        assert_eq!(r.rule_multiplier("a.one"), 2.0);
        assert_eq!(r.rule_multiplier("a.two"), 0.5);
        assert!(r.is_rule_enabled("a.two"));
        assert_eq!(r.rule_multiplier("a.unknown"), 1.0);
    }

    #[test]
    fn a_zero_multiplier_is_still_enabled_but_disabled_is_not() {
        let ps = vec![profile(
            "a",
            None,
            &uniform(),
            &[
                ("r.zero", RuleOverride::Multiplier(0.0)),
                ("r.off", RuleOverride::Disabled),
            ],
            &[],
        )];
        let r = resolve(&ps, "a").unwrap();
        assert!(r.is_rule_enabled("r.zero"));
        assert_eq!(r.rule_multiplier("r.zero"), 0.0);
        assert!(!r.is_rule_enabled("r.off"));
        assert_eq!(r.rule_multiplier("r.off"), 0.0);
    }

    #[test]
    fn applies_to_matches_any_ancestor() {
        let ps = vec![
            profile("root", None, &uniform(), &[], &[]),
            profile("leaf", Some("root"), &uniform(), &[], &[]),
        ];
        let r = resolve(&ps, "leaf").unwrap();
        let mut rule = TheoryRule {
            id: "harmony.x".into(),
            domain: RuleDomain::Harmony,
            kind: crate::model::RuleKind::TheoryDefault,
            summary: String::new(),
            trigger: crate::model::RuleTrigger::from_json(
                &json_obj! { "event" => "chord_pair" },
                "t",
            )
            .unwrap(),
            conditions: vec![],
            effect: crate::model::RuleEffect {
                score_delta: 1.0,
                severity: music_domain::candidate::Severity::Minor,
                score_component: "melody_fit".into(),
            },
            profiles: vec!["root".into()],
            exceptions: vec![],
            source_refs: vec![],
            confidence: 1.0,
            rationale: String::new(),
            test_ids: vec![],
            version: 1,
        };
        assert!(
            r.applies_to(&rule),
            "an ancestor's rule is active in the child"
        );
        rule.profiles = vec!["elsewhere".into()];
        assert!(!r.applies_to(&rule));
    }

    #[test]
    fn dotted_field_lookup_reaches_nested_objects() {
        let ps = vec![profile(
            "a",
            None,
            &uniform(),
            &[],
            &[(
                "register_spacing",
                json_obj! { "open_voicing_bias" => 0.75, "source" => "instrument_profile" },
            )],
        )];
        let r = resolve(&ps, "a").unwrap();
        assert_eq!(
            r.field_f64("register_spacing.open_voicing_bias"),
            Some(0.75)
        );
        assert_eq!(
            r.field_str("register_spacing.source"),
            Some("instrument_profile")
        );
        assert_eq!(r.field_f64("register_spacing.missing"), None);
        assert_eq!(r.field_f64("missing.entirely"), None);
    }

    #[test]
    fn to_json_lists_all_thirteen_weights() {
        let ps = vec![profile("a", None, &uniform(), &[], &[])];
        let r = resolve(&ps, "a").unwrap();
        let j = r.to_json();
        assert_eq!(j.get("weights").and_then(Json::as_obj).unwrap().len(), 13);
        assert_eq!(j.get("id").and_then(Json::as_str), Some("a"));
    }
}
