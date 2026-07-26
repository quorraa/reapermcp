//! The loaded bundle: parsing `knowledge/` into typed records and answering
//! lookups.
//!
//! Two entry points matter. [`KnowledgeBase::embedded`] returns the bundle
//! compiled into the binary, parsed once behind a `OnceLock`, so the server can
//! never start without its theory data. [`KnowledgeBase::load_dir`] reads and
//! *fully validates* an external override directory, which is the only way
//! unverified knowledge can enter the process.
//!
//! The content hash is always recomputed from the files themselves and never
//! trusted from the manifest.

use crate::embedded::EMBEDDED_FILES;
use crate::error::KbError;
use crate::model::{
    ArrangementPattern, CadenceSchema, ChordQuality, ChordSymbolAlias, FunctionEntry,
    InstrumentProfile, IntervalRecord, Manifest, ModeRecord, ProgressionSchema, ScaleRecord,
    SourceRecord, StyleProfile, TheoryRule, VoicingTemplate,
};
use crate::model::{RuleDomain, RuleEvent};
use crate::profile::{resolve, ResolvedProfile};
use crate::validate::{validate, ValidateOptions};
use qjson::{Json, JsonMap};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

/// The file name of the bundle manifest, which is excluded from the hash.
pub const MANIFEST_FILE: &str = "manifest.json";

/// Every knowledge record, parsed and indexed.
#[derive(Clone, Debug)]
pub struct KnowledgeBase {
    manifest: Manifest,
    files: BTreeMap<String, String>,
    docs: BTreeMap<String, Json>,
    content_hash: String,
    origin: String,

    sources: Vec<SourceRecord>,
    rules: Vec<TheoryRule>,
    profiles: Vec<StyleProfile>,
    scales: Vec<ScaleRecord>,
    chord_qualities: Vec<ChordQuality>,
    chord_symbols: Vec<ChordSymbolAlias>,
    functions: Vec<FunctionEntry>,
    voicing_templates: Vec<VoicingTemplate>,
    progressions: Vec<ProgressionSchema>,
    cadences: Vec<CadenceSchema>,
    arrangement_patterns: Vec<ArrangementPattern>,
    instrument_profiles: Vec<InstrumentProfile>,
    modes: Vec<ModeRecord>,
    intervals: Vec<IntervalRecord>,
}

/// Computes the bundle content hash.
///
/// # Exact byte layout
///
/// The value published as `manifest.content_sha256` is
/// `SHA-256(B₁ ‖ B₂ ‖ … ‖ Bₙ)` where:
///
/// * the inputs are **every `*.json` file under the knowledge root except
///   `manifest.json`**, identified by its path relative to that root with
///   forward slashes;
/// * the files are ordered by a **byte-wise ascending sort of those relative
///   paths** (so `profiles/blues.json` precedes `progressions.json`, because
///   `f` < `r`);
/// * `Bᵢ` is the **UTF-8 encoding of `Json::to_canonical_string()`** of the
///   parsed file — RFC-8785-style: object keys sorted, no insignificant
///   whitespace, shortest round-trip float rendering;
/// * the byte strings are concatenated with **no separator, no path bytes, no
///   length prefix and no trailing newline**. Each `Bᵢ` is a complete,
///   self-delimiting JSON value, so the concatenation is unambiguous for a
///   fixed file set.
///
/// The digest is rendered as **64 lower-case hexadecimal characters**.
///
/// Reformatting a knowledge file therefore never changes the hash, but
/// changing any value, key or file does.
pub fn content_hash(docs: &BTreeMap<String, Json>) -> String {
    let mut h = qjson::sha256::Sha256::new();
    for (path, doc) in docs {
        if path == MANIFEST_FILE || !path.ends_with(".json") {
            continue;
        }
        h.update(doc.to_canonical_string().as_bytes());
    }
    h.finish_hex()
}

/// Reads the `items` array of a knowledge file and maps each entry.
fn items<T>(
    docs: &BTreeMap<String, Json>,
    file: &str,
    f: impl Fn(&Json, &str) -> Result<T, KbError>,
) -> Result<Vec<T>, KbError> {
    let doc = docs
        .get(file)
        .ok_or_else(|| KbError::missing_file(file, format!("{file} is missing from the bundle")))?;
    let arr = doc
        .arr_field("items")
        .map_err(|e| KbError::shape(format!("{file}.items"), e.message))?;
    let mut out = Vec::with_capacity(arr.len());
    for v in arr {
        out.push(f(v, file)?);
    }
    Ok(out)
}

impl KnowledgeBase {
    /// The bundle compiled into the binary.
    ///
    /// Infallible by construction: the embedded files are validated by this
    /// crate's tests and by `cargo run -p xtask -- check-all`, so a failure
    /// here means the binary itself was built from a broken tree.
    ///
    /// Use [`KnowledgeBase::try_embedded`] to handle that case explicitly.
    pub fn embedded() -> &'static KnowledgeBase {
        match KnowledgeBase::try_embedded() {
            Ok(kb) => kb,
            // Not reachable from any input: the bundle is a compile-time
            // constant and is proved loadable by `embedded_bundle_is_valid`.
            Err(e) => panic!("the embedded knowledge bundle is corrupt: {e}"),
        }
    }

    /// The embedded bundle, with the load error surfaced instead of panicking.
    pub fn try_embedded() -> Result<&'static KnowledgeBase, KbError> {
        static EMBEDDED: OnceLock<Result<KnowledgeBase, KbError>> = OnceLock::new();
        EMBEDDED
            .get_or_init(|| {
                let files: BTreeMap<String, String> = EMBEDDED_FILES
                    .iter()
                    .map(|(p, c)| ((*p).to_string(), (*c).to_string()))
                    .collect();
                KnowledgeBase::from_files(files, "knowledge (embedded)", true)
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// Loads and fully validates an external override directory.
    ///
    /// Unlike the embedded bundle, an external directory whose manifest still
    /// reads `"PENDING"` is rejected: unstamped knowledge has no provenance.
    pub fn load_dir(path: &Path) -> Result<KnowledgeBase, KbError> {
        let mut files = BTreeMap::new();
        read_tree(path, path, &mut files)?;
        if files.is_empty() {
            return Err(KbError::missing_file(
                path.display().to_string(),
                "the knowledge directory contains no files",
            ));
        }
        KnowledgeBase::from_files(files, &path.display().to_string(), false)
    }

    /// Parses and validates an in-memory bundle.
    ///
    /// `allow_pending_hash` is true only for the embedded bundle, which
    /// `xtask stamp-manifest` is responsible for stamping.
    pub fn from_files(
        files: BTreeMap<String, String>,
        origin: &str,
        allow_pending_hash: bool,
    ) -> Result<KnowledgeBase, KbError> {
        let kb = KnowledgeBase::parse(files, origin)?;
        validate(
            &kb,
            &ValidateOptions {
                allow_pending_hash,
                ..ValidateOptions::default()
            },
        )
        .map_err(|errors| {
            errors.into_iter().next().unwrap_or_else(|| {
                KbError::inconsistent(origin, "validation failed without reporting a reason")
            })
        })?;
        Ok(kb)
    }

    /// Parses an in-memory bundle **without** validating it.
    ///
    /// Used by `xtask` so a broken bundle can still be inspected, and by the
    /// validator's own tests.
    pub fn parse(files: BTreeMap<String, String>, origin: &str) -> Result<KnowledgeBase, KbError> {
        let mut docs = BTreeMap::new();
        for (path, text) in &files {
            if path.ends_with(".json") {
                let doc = Json::parse(text)
                    .map_err(|e| KbError::parse(format!("{origin}/{path}"), e.to_string()))?;
                docs.insert(path.clone(), doc);
            }
        }

        let manifest_doc = docs.get(MANIFEST_FILE).ok_or_else(|| {
            KbError::missing_file(
                format!("{origin}/{MANIFEST_FILE}"),
                "the bundle has no manifest",
            )
        })?;
        let manifest = Manifest::from_json(manifest_doc, MANIFEST_FILE)?;

        let mut profiles = Vec::new();
        for (path, doc) in &docs {
            if path.starts_with("profiles/") {
                for p in items(&docs, path, StyleProfile::from_json)? {
                    let _ = doc;
                    profiles.push(p);
                }
            }
        }
        profiles.sort_by(|a, b| a.id.cmp(&b.id));

        let mut rules = Vec::new();
        for path in docs.keys() {
            if path.starts_with("rules/") {
                rules.extend(items(&docs, path, TheoryRule::from_json)?);
            }
        }

        let content_hash = content_hash(&docs);

        Ok(KnowledgeBase {
            sources: items(&docs, "sources.json", SourceRecord::from_json)?,
            scales: items(&docs, "scales.json", |v, p| {
                ScaleRecord::from_json(v).map_err(|e| KbError::shape(p, e.message))
            })?,
            chord_qualities: items(&docs, "chord_qualities.json", ChordQuality::from_json)?,
            chord_symbols: items(&docs, "chord_symbols.json", ChordSymbolAlias::from_json)?,
            functions: items(&docs, "functions.json", FunctionEntry::from_json)?,
            voicing_templates: items(&docs, "voicings.json", VoicingTemplate::from_json)?,
            progressions: items(&docs, "progressions.json", ProgressionSchema::from_json)?,
            cadences: items(&docs, "cadences.json", CadenceSchema::from_json)?,
            arrangement_patterns: items(
                &docs,
                "arrangement_patterns.json",
                ArrangementPattern::from_json,
            )?,
            instrument_profiles: items(
                &docs,
                "instrument_profiles.json",
                InstrumentProfile::from_json,
            )?,
            modes: items(&docs, "modes.json", ModeRecord::from_json)?,
            intervals: items(&docs, "intervals.json", IntervalRecord::from_json)?,
            rules,
            profiles,
            manifest,
            content_hash,
            origin: origin.to_string(),
            files,
            docs,
        })
    }

    // --- identity -------------------------------------------------------

    /// The manifest's `knowledge_version`.
    pub fn version(&self) -> &str {
        &self.manifest.knowledge_version
    }

    /// The manifest's `schema_version`.
    pub fn schema_version(&self) -> &str {
        &self.manifest.schema_version
    }

    /// The hash recomputed from the files, never read from the manifest.
    pub fn content_hash(&self) -> &str {
        &self.content_hash
    }

    /// The parsed manifest.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Where the bundle came from, for error messages.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Raw file contents by relative path.
    pub fn files(&self) -> &BTreeMap<String, String> {
        &self.files
    }

    /// Parsed JSON documents by relative path.
    pub fn docs(&self) -> &BTreeMap<String, Json> {
        &self.docs
    }

    /// Record counts, using the same keys as the manifest.
    pub fn counts(&self) -> BTreeMap<String, usize> {
        let mut c = BTreeMap::new();
        c.insert("sources".into(), self.sources.len());
        c.insert("scales".into(), self.scales.len());
        c.insert("chord_qualities".into(), self.chord_qualities.len());
        c.insert("rules".into(), self.rules.len());
        c.insert("profiles".into(), self.profiles.len());
        c.insert("voicing_templates".into(), self.voicing_templates.len());
        c.insert(
            "progression_schemas".into(),
            self.progressions.len() + self.cadences.len(),
        );
        c.insert(
            "arrangement_patterns".into(),
            self.arrangement_patterns.len(),
        );
        c.insert("modes".into(), self.modes.len());
        c.insert("intervals".into(), self.intervals.len());
        c.insert("chord_symbols".into(), self.chord_symbols.len());
        c.insert("functions".into(), self.functions.len());
        c.insert("progressions".into(), self.progressions.len());
        c.insert("cadences".into(), self.cadences.len());
        c.insert("instrument_profiles".into(), self.instrument_profiles.len());
        c
    }

    /// Rule counts per domain, using the same keys as the manifest.
    pub fn rule_counts_by_domain(&self) -> BTreeMap<String, usize> {
        let mut c = BTreeMap::new();
        for d in RuleDomain::all() {
            c.insert(d.id().to_string(), 0);
        }
        for r in &self.rules {
            *c.entry(r.domain.id().to_string()).or_insert(0) += 1;
        }
        c.retain(|_, v| *v > 0);
        c
    }

    // --- accessors ------------------------------------------------------

    /// Every source record, in file order.
    pub fn sources(&self) -> &[SourceRecord] {
        &self.sources
    }

    /// One source record by id.
    pub fn source(&self, id: &str) -> Option<&SourceRecord> {
        self.sources.iter().find(|s| s.id == id)
    }

    /// Every rule, hard kinds interleaved in file order.
    pub fn rules(&self) -> &[TheoryRule] {
        &self.rules
    }

    /// One rule by id.
    pub fn rule(&self, id: &str) -> Option<&TheoryRule> {
        self.rules.iter().find(|r| r.id == id)
    }

    /// Every rule in one domain, in file order.
    pub fn rules_in_domain(&self, d: RuleDomain) -> Vec<&TheoryRule> {
        self.rules.iter().filter(|r| r.domain == d).collect()
    }

    /// Every rule listening for `e` that the profile has not switched off.
    pub fn rules_for_event(&self, e: RuleEvent, profile: &ResolvedProfile) -> Vec<&TheoryRule> {
        self.rules
            .iter()
            .filter(|r| {
                r.trigger.event == e && profile.applies_to(r) && profile.is_rule_enabled(&r.id)
            })
            .collect()
    }

    /// Every style profile, sorted by id.
    pub fn profiles(&self) -> &[StyleProfile] {
        &self.profiles
    }

    /// One style profile by id.
    pub fn profile(&self, id: &str) -> Option<&StyleProfile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    /// Flattens the parent chain, merges overrides and normalises weights.
    pub fn resolve_profile(&self, id: &str) -> Result<ResolvedProfile, KbError> {
        resolve(&self.profiles, id)
    }

    /// Every scale collection, in file order.
    pub fn scales(&self) -> &[ScaleRecord] {
        &self.scales
    }

    /// One scale by id.
    pub fn scale(&self, id: &str) -> Option<&ScaleRecord> {
        self.scales.iter().find(|s| s.id == id)
    }

    /// Every chord quality, in file order.
    pub fn chord_qualities(&self) -> &[ChordQuality] {
        &self.chord_qualities
    }

    /// One chord quality by id.
    pub fn chord_quality(&self, id: &str) -> Option<&ChordQuality> {
        self.chord_qualities.iter().find(|q| q.id == id)
    }

    /// Every chord-symbol token, in file order.
    pub fn chord_symbols(&self) -> &[ChordSymbolAlias] {
        &self.chord_symbols
    }

    /// Every voicing template, in file order.
    pub fn voicing_templates(&self) -> &[VoicingTemplate] {
        &self.voicing_templates
    }

    /// One voicing template by id.
    pub fn voicing_template(&self, id: &str) -> Option<&VoicingTemplate> {
        self.voicing_templates.iter().find(|v| v.id == id)
    }

    /// Every progression schema, in file order.
    pub fn progressions(&self) -> &[ProgressionSchema] {
        &self.progressions
    }

    /// Every cadence schema, in file order.
    pub fn cadences(&self) -> &[CadenceSchema] {
        &self.cadences
    }

    /// Every arrangement pattern, in file order.
    pub fn arrangement_patterns(&self) -> &[ArrangementPattern] {
        &self.arrangement_patterns
    }

    /// Every instrument profile, in file order.
    pub fn instrument_profiles(&self) -> &[InstrumentProfile] {
        &self.instrument_profiles
    }

    /// One instrument profile by id.
    pub fn instrument_profile(&self, id: &str) -> Option<&InstrumentProfile> {
        self.instrument_profiles.iter().find(|p| p.id == id)
    }

    /// Every functional entry, in file order.
    pub fn functions(&self) -> &[FunctionEntry] {
        &self.functions
    }

    /// Every modal character record, in file order.
    pub fn modes(&self) -> &[ModeRecord] {
        &self.modes
    }

    /// Every interval record, in file order.
    pub fn intervals(&self) -> &[IntervalRecord] {
        &self.intervals
    }

    /// Every distinct `test_id` named by any rule, sorted.
    pub fn test_ids(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .rules
            .iter()
            .flat_map(|r| r.test_ids.iter().cloned())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Every distinct predicate used by any rule, sorted.
    pub fn predicates_used(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .rules
            .iter()
            .flat_map(|r| r.conditions.iter().chain(r.exceptions.iter()).cloned())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// The catalogue summary behind `theory://catalog`.
    pub fn catalog_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("knowledge_version", Json::Str(self.version().to_string()));
        m.insert(
            "schema_version",
            Json::Str(self.schema_version().to_string()),
        );
        m.insert("content_sha256", Json::Str(self.content_hash.clone()));
        m.insert(
            "manifest_sha256",
            Json::Str(self.manifest.content_sha256.clone()),
        );
        m.insert(
            "generated_at",
            Json::Str(self.manifest.generated_at.clone()),
        );

        let mut counts = JsonMap::new();
        for (k, v) in self.counts() {
            counts.insert(k, Json::Int(v as i64));
        }
        m.insert("counts", Json::Obj(counts));

        let mut by_domain = JsonMap::new();
        for (k, v) in self.rule_counts_by_domain() {
            by_domain.insert(k, Json::Int(v as i64));
        }
        m.insert("rules_by_domain", Json::Obj(by_domain));

        let mut collections = JsonMap::new();
        collections.insert(
            "profiles",
            id_name_list(
                self.profiles
                    .iter()
                    .map(|p| (p.id.as_str(), p.name.as_str())),
            ),
        );
        collections.insert(
            "scales",
            id_name_list(self.scales.iter().map(|s| (s.id.as_str(), s.name.as_str()))),
        );
        collections.insert(
            "chord_qualities",
            id_name_list(
                self.chord_qualities
                    .iter()
                    .map(|q| (q.id.as_str(), q.name.as_str())),
            ),
        );
        collections.insert(
            "voicing_templates",
            id_name_list(
                self.voicing_templates
                    .iter()
                    .map(|v| (v.id.as_str(), v.name.as_str())),
            ),
        );
        collections.insert(
            "progressions",
            id_name_list(
                self.progressions
                    .iter()
                    .map(|p| (p.id.as_str(), p.name.as_str())),
            ),
        );
        collections.insert(
            "cadences",
            id_name_list(
                self.cadences
                    .iter()
                    .map(|c| (c.schema.id.as_str(), c.schema.name.as_str())),
            ),
        );
        collections.insert(
            "arrangement_patterns",
            id_name_list(
                self.arrangement_patterns
                    .iter()
                    .map(|a| (a.id.as_str(), a.name.as_str())),
            ),
        );
        collections.insert(
            "instrument_profiles",
            id_name_list(
                self.instrument_profiles
                    .iter()
                    .map(|p| (p.id.as_str(), p.name.as_str())),
            ),
        );
        collections.insert(
            "modes",
            id_name_list(
                self.modes
                    .iter()
                    .map(|m| (m.id.as_str(), m.scale_id.as_str())),
            ),
        );
        collections.insert(
            "intervals",
            id_name_list(
                self.intervals
                    .iter()
                    .map(|i| (i.id.as_str(), i.name.as_str())),
            ),
        );
        collections.insert(
            "sources",
            id_name_list(
                self.sources
                    .iter()
                    .map(|s| (s.id.as_str(), s.title.as_str())),
            ),
        );
        m.insert("collections", Json::Obj(collections));

        m.insert(
            "predicates",
            Json::Arr(
                crate::rules::RuleEngine::known_predicates()
                    .iter()
                    .map(|p| Json::Str((*p).to_string()))
                    .collect(),
            ),
        );
        Json::Obj(m)
    }
}

/// Renders an id/name pair list.
fn id_name_list<'a>(items: impl Iterator<Item = (&'a str, &'a str)>) -> Json {
    Json::Arr(
        items
            .map(|(id, name)| {
                let mut m = JsonMap::new();
                m.insert("id", Json::Str(id.to_string()));
                m.insert("name", Json::Str(name.to_string()));
                Json::Obj(m)
            })
            .collect(),
    )
}

/// Recursively reads every file under `dir`, keyed by forward-slash path
/// relative to `root`.
pub fn read_tree(
    root: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, String>,
) -> Result<(), KbError> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| KbError::io(dir.display().to_string(), e.to_string()))?;
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    for e in entries {
        let e = e.map_err(|e| KbError::io(dir.display().to_string(), e.to_string()))?;
        paths.push(e.path());
    }
    paths.sort();
    for p in paths {
        if p.is_dir() {
            read_tree(root, &p, out)?;
        } else {
            let rel = p
                .strip_prefix(root)
                .map_err(|_| KbError::io(p.display().to_string(), "path escaped the root"))?
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let text = std::fs::read_to_string(&p)
                .map_err(|e| KbError::io(p.display().to_string(), e.to_string()))?;
            out.insert(rel, text);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_bundle_loads_and_validates() {
        let kb = KnowledgeBase::try_embedded().expect("the embedded bundle must load");
        assert_eq!(kb.version(), "1.0.0");
        assert_eq!(kb.content_hash().len(), 64);
        assert!(kb
            .content_hash()
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
    }

    #[test]
    fn embedded_counts_match_the_manifest() {
        let kb = KnowledgeBase::embedded();
        for (k, v) in kb.counts() {
            assert_eq!(
                kb.manifest().counts.get(&k),
                Some(&v),
                "count mismatch for {k}"
            );
        }
        assert_eq!(kb.rules().len(), 147);
        assert_eq!(kb.profiles().len(), 10);
        assert_eq!(kb.sources().len(), 9);
        assert_eq!(kb.scales().len(), 35);
        assert_eq!(kb.chord_qualities().len(), 51);
        assert_eq!(kb.voicing_templates().len(), 38);
        assert_eq!(kb.arrangement_patterns().len(), 28);
        assert_eq!(kb.instrument_profiles().len(), 14);
        assert_eq!(kb.progressions().len() + kb.cadences().len(), 51);
    }

    #[test]
    fn lookups_resolve() {
        let kb = KnowledgeBase::embedded();
        assert!(kb
            .rule("harmony.dominant_seventh_resolves_down_fifth")
            .is_some());
        assert!(kb.rule("harmony.does_not_exist").is_none());
        assert_eq!(kb.scale("major").map(|s| s.semitones.len()), Some(7));
        assert!(kb.chord_quality("major_triad").is_some());
        assert!(kb.source("open-music-theory").is_some());
        assert!(kb.profile("jazz_standard").is_some());
        assert!(kb.instrument_profile("melody_lead").is_some());
        assert!(kb.voicing_template("close_triad_root").is_some());
    }

    #[test]
    fn rules_in_domain_partitions_the_rule_base() {
        let kb = KnowledgeBase::embedded();
        let total: usize = RuleDomain::all()
            .iter()
            .map(|d| kb.rules_in_domain(*d).len())
            .sum();
        assert_eq!(total, kb.rules().len());
        assert_eq!(kb.rules_in_domain(RuleDomain::Harmony).len(), 37);
    }

    #[test]
    fn rules_for_event_respects_profile_scope_and_disables() {
        let kb = KnowledgeBase::embedded();
        let jazz = kb.resolve_profile("jazz_standard").unwrap();
        let all: Vec<_> = kb
            .rules()
            .iter()
            .filter(|r| r.trigger.event == RuleEvent::VoicePairMotion)
            .collect();
        let active = kb.rules_for_event(RuleEvent::VoicePairMotion, &jazz);
        assert!(!active.is_empty());
        assert!(active.len() <= all.len());
        assert!(active
            .iter()
            .all(|r| jazz.is_rule_enabled(&r.id) && jazz.applies_to(r)));
        // jazz_standard disables this one explicitly.
        assert!(!active
            .iter()
            .any(|r| r.id == "voice_leading.direct_perfect_in_outer_voices"));
    }

    #[test]
    fn every_profile_resolves() {
        let kb = KnowledgeBase::embedded();
        for p in kb.profiles() {
            let r = kb.resolve_profile(&p.id).unwrap();
            assert_eq!(r.id, p.id);
            assert!(
                (r.weights.values().sum::<f64>() - 1.0).abs() < 1e-9,
                "{}",
                p.id
            );
            assert_eq!(r.weights.len(), 13);
            assert_eq!(r.chain[0], p.id);
        }
        assert_eq!(
            kb.resolve_profile("neo_soul_rnb").unwrap().chain,
            vec!["neo_soul_rnb", "jazz_standard", "common_practice"]
        );
        assert_eq!(kb.resolve_profile("blues").unwrap().chain, vec!["blues"]);
        assert_eq!(
            kb.resolve_profile("drum_and_bass").unwrap().chain,
            vec!["drum_and_bass", "electronic_loop", "pop_rock"]
        );
    }

    #[test]
    fn unknown_profile_is_not_found() {
        let kb = KnowledgeBase::embedded();
        assert_eq!(
            kb.resolve_profile("dubstep").unwrap_err().code,
            "KB_NOT_FOUND"
        );
    }

    #[test]
    fn catalog_json_is_complete_and_deterministic() {
        let kb = KnowledgeBase::embedded();
        let a = kb.catalog_json();
        let b = kb.catalog_json();
        assert_eq!(a.to_string(), b.to_string());
        assert_eq!(
            a.get("knowledge_version").and_then(Json::as_str),
            Some("1.0.0")
        );
        assert_eq!(
            a.get("counts")
                .and_then(|c| c.get("rules"))
                .and_then(Json::as_i64),
            Some(147)
        );
        let preds = a.get("predicates").and_then(Json::as_arr).unwrap();
        assert_eq!(
            preds.len(),
            crate::rules::RuleEngine::known_predicates().len()
        );
        let colls = a.get("collections").and_then(Json::as_obj).unwrap();
        assert_eq!(colls.len(), 11);
    }

    #[test]
    fn test_ids_and_predicates_are_sorted_and_unique() {
        let kb = KnowledgeBase::embedded();
        let ids = kb.test_ids();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(ids.len(), 237);
        let preds = kb.predicates_used();
        assert!(preds.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn content_hash_ignores_formatting_but_not_content() {
        let mut a = BTreeMap::new();
        a.insert(
            "x.json".to_string(),
            Json::parse(r#"{"b":1,"a":2}"#).unwrap(),
        );
        let mut b = BTreeMap::new();
        b.insert(
            "x.json".to_string(),
            Json::parse("{\n  \"a\" : 2,\n  \"b\": 1\n}").unwrap(),
        );
        assert_eq!(content_hash(&a), content_hash(&b));
        let mut c = BTreeMap::new();
        c.insert(
            "x.json".to_string(),
            Json::parse(r#"{"a":3,"b":1}"#).unwrap(),
        );
        assert_ne!(content_hash(&a), content_hash(&c));
    }

    #[test]
    fn content_hash_excludes_the_manifest_and_non_json_files() {
        let mut a = BTreeMap::new();
        a.insert("x.json".to_string(), Json::parse("{}").unwrap());
        let base = content_hash(&a);
        a.insert(
            MANIFEST_FILE.to_string(),
            Json::parse(r#"{"z":1}"#).unwrap(),
        );
        assert_eq!(content_hash(&a), base);
    }

    #[test]
    fn embedded_hash_matches_the_stamped_manifest() {
        let kb = KnowledgeBase::embedded();
        assert_eq!(
            kb.manifest().content_sha256,
            kb.content_hash(),
            "run `cargo run -p xtask -- stamp-manifest`"
        );
    }
}
