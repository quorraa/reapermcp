//! FNV-1a-64 and the versioned canonical strings the bridge hashes.
//!
//! Every hash in the protocol is FNV-1a-64 over the UTF-8 bytes of a canonical
//! string, rendered as `fnv1a64:` followed by 16 lowercase zero-padded hex
//! digits. This module reimplements the bridge's canonicalisation exactly, so a
//! client can independently verify a snapshot it was handed instead of trusting
//! the hash field.
//!
//! # This is not a cryptographic hash
//!
//! FNV-1a detects accidental change — the user edited a note, moved the item,
//! changed the tempo. It is not collision resistant against a motivated
//! adversary. Treat a match as "probably unchanged", never as an authorisation.
//!
//! # Canonical formatting rules (§9.2)
//!
//! | kind | rendering |
//! |---|---|
//! | float (`f6`) | `%.6f`, negative zero normalised to `0.000000`, `±1e12` and non-finite rejected |
//! | integer (`d`) | decimal, no padding, `-` for negatives |
//! | boolean (`b`) | `1` / `0` |
//! | absent string | the four ASCII bytes `null` |
//! | line separator | `\n`, including after the last line |
//! | field separator | `\|` |
//!
//! Note the empty-string rule: the bridge's `s_or_null` maps `""` to `null`
//! just as it maps an absent value, so [`s_or_null`] does too. Emitting an
//! empty field instead would produce a different digest.

use crate::error::{codes, IpcError};

/// FNV-1a offset basis, 64-bit.
pub const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// FNV-1a prime, 64-bit.
pub const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// The prefix stamped on every hash string the protocol carries.
pub const HASH_PREFIX: &str = "fnv1a64:";

/// FNV-1a 64-bit hash of a byte string.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = FNV_OFFSET_BASIS;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

/// Hash of a canonical string in the protocol's rendered form.
///
/// ```
/// assert_eq!(reaper_ipc::hash::qlabs_hash(b"foobar"), "fnv1a64:85944171f73967e8");
/// ```
pub fn qlabs_hash(bytes: &[u8]) -> String {
    format!("{HASH_PREFIX}{:016x}", fnv1a64(bytes))
}

/// True when `s` has the shape of a protocol hash string.
///
/// Shape only — this says nothing about whether the digest is correct.
pub fn is_hash_string(s: &str) -> bool {
    match s.strip_prefix(HASH_PREFIX) {
        Some(hex) => {
            hex.len() == 16
                && hex
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        }
        None => false,
    }
}

/// Renders a float as the canonical `f6` form.
///
/// Returns `None` for non-finite values and for magnitudes above `1e12`, which
/// the bridge treats as an error rather than hashing.
pub fn f6(x: f64) -> Option<String> {
    if !x.is_finite() {
        return None;
    }
    if !(-1e12..=1e12).contains(&x) {
        return None;
    }
    // Normalises -0.0 to 0.0 so the sign never reaches the output.
    let x = if x == 0.0 { 0.0 } else { x };
    Some(format!("{x:.6}"))
}

/// Renders a number as the canonical `d` form.
///
/// Mirrors the bridge, which rounds a non-integral value with
/// `floor(x + 0.5)` rather than rejecting it. Non-finite values are rejected.
pub fn d(x: f64) -> Option<String> {
    if !x.is_finite() {
        return None;
    }
    if x.fract() == 0.0 && x.abs() < 9.007_199_254_740_992e15 {
        return Some(format!("{}", x as i64));
    }
    let r = (x + 0.5).floor();
    if !r.is_finite() || r.abs() >= 9.007_199_254_740_992e15 {
        return None;
    }
    Some(format!("{}", r as i64))
}

/// Renders an integer as the canonical `d` form.
pub fn di(x: i64) -> String {
    format!("{x}")
}

/// Renders a boolean as the canonical `b` form: `1` or `0`.
pub fn b01(v: bool) -> &'static str {
    if v {
        "1"
    } else {
        "0"
    }
}

/// Renders an optional string, using the four ASCII bytes `null` for both an
/// absent value **and** an empty one, exactly as the bridge does.
pub fn s_or_null(v: Option<&str>) -> &str {
    match v {
        Some("") | None => "null",
        Some(s) => s,
    }
}

fn bad_number(field: &str, value: f64) -> IpcError {
    IpcError::with_details(
        codes::SNAPSHOT_HASH_MISMATCH,
        format!("{field} is not representable in a canonical hash string: {value}"),
        qjson::json_obj! { "field" => field, "value" => format!("{value}") },
    )
}

fn f6r(field: &str, x: f64) -> Result<String, IpcError> {
    f6(x).ok_or_else(|| bad_number(field, x))
}

/// One note as the bridge sees it in the take's own PPQ domain.
///
/// Used for [`midi_canonical`] and [`selection_canonical`], which hash **every**
/// note in the take before any scope or channel filtering.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TakeNote {
    /// Note start in take PPQ.
    pub start_ppq: f64,
    /// Note end in take PPQ.
    pub end_ppq: f64,
    /// MIDI channel, 0..=15.
    pub channel: i64,
    /// MIDI pitch, 0..=127.
    pub pitch: i64,
    /// MIDI velocity, 1..=127.
    pub velocity: i64,
    /// Muted flag.
    pub muted: bool,
    /// Selected flag.
    pub selected: bool,
}

/// One note in the project quarter-note domain, as it appears in the result's
/// filtered `notes` array. Used for [`note_list_canonical`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ListNote {
    /// Note start in project quarter notes.
    pub start_qn: f64,
    /// Note end in project quarter notes.
    pub end_qn: f64,
    /// MIDI pitch.
    pub pitch: i64,
    /// MIDI velocity.
    pub velocity: i64,
    /// MIDI channel.
    pub channel: i64,
    /// Muted flag.
    pub muted: bool,
    /// Selected flag.
    pub selected: bool,
}

/// One tempo / time-signature marker.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TempoMarker {
    /// Marker position in project seconds.
    pub time_seconds: f64,
    /// Marker position in project quarter notes.
    pub qn: f64,
    /// Tempo in BPM at this marker.
    pub bpm: f64,
    /// Time-signature numerator, or 0 when the marker carries none.
    pub timesig_num: i64,
    /// Time-signature denominator, or 0 when the marker carries none.
    pub timesig_den: i64,
    /// Linear tempo ramp flag.
    pub linear: bool,
}

/// The exact set of fields that go into [`snapshot_canonical`].
///
/// Everything else in an `inspect_selection` result is deliberately excluded —
/// see the module docs on §9.8 — so that two inspections of an unmodified
/// project produce the same `snapshot_hash`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SnapshotFields {
    /// Persistent project UUID.
    pub project_uuid: Option<String>,
    /// Normalised track GUID.
    pub track_guid: Option<String>,
    /// Normalised item GUID.
    pub item_guid: Option<String>,
    /// Normalised take GUID.
    pub take_guid: Option<String>,
    /// Item position in project seconds.
    pub item_position_seconds: f64,
    /// Item length in project seconds.
    pub item_length_seconds: f64,
    /// Item position in project quarter notes.
    pub item_position_qn: f64,
    /// Item length in project quarter notes.
    pub item_length_qn: f64,
    /// REAPER's `B_LOOPSRC` flag.
    pub is_loop_source: bool,
    /// The **requested** note scope, not the effective one.
    pub note_scope: Option<String>,
    /// The requested melody extraction mode.
    pub extraction_mode: Option<String>,
    /// The requested channel filter, or `None` for no filter (hashed as `-1`).
    pub extraction_channel: Option<i64>,
    /// Prefixed `midi_hash` string.
    pub midi_hash: Option<String>,
    /// Prefixed `tempo_map_hash` string.
    pub tempo_map_hash: Option<String>,
    /// Prefixed `timesig_map_hash` string.
    pub timesig_map_hash: Option<String>,
    /// Prefixed `note_selection_hash` string.
    pub note_selection_hash: Option<String>,
    /// Prefixed `note_list_hash` string.
    pub note_list_hash: Option<String>,
    /// Number of notes in the filtered list.
    pub note_count: i64,
}

/// The order the bridge sorts take notes into before hashing: `(start_ppq,
/// pitch, channel, end_ppq, velocity, original index)`.
///
/// Deterministic and independent of REAPER's internal event order. Callers that
/// received an already-ordered `notes` array do not need this; it exists so a
/// client building its own take view hashes the same thing.
pub fn sort_take_notes(notes: &mut [TakeNote]) {
    notes.sort_by(|a, b| {
        a.start_ppq
            .total_cmp(&b.start_ppq)
            .then(a.pitch.cmp(&b.pitch))
            .then(a.channel.cmp(&b.channel))
            .then(a.end_ppq.total_cmp(&b.end_ppq))
            .then(a.velocity.cmp(&b.velocity))
    });
}

/// Canonical string for `midi_hash` (§9.3).
///
/// Over every note in the take, in the order [`sort_take_notes`] defines. An
/// empty take yields exactly `"qlabs.midi.v1\n0\n"`.
pub fn midi_canonical(notes: &[TakeNote]) -> Result<String, IpcError> {
    let mut out = String::with_capacity(64 + notes.len() * 48);
    out.push_str("qlabs.midi.v1\n");
    out.push_str(&di(notes.len() as i64));
    out.push('\n');
    for (i, n) in notes.iter().enumerate() {
        out.push_str(&di(i as i64));
        out.push('|');
        out.push_str(&f6r("start_ppq", n.start_ppq)?);
        out.push('|');
        out.push_str(&f6r("end_ppq", n.end_ppq)?);
        out.push('|');
        out.push_str(&di(n.channel));
        out.push('|');
        out.push_str(&di(n.pitch));
        out.push('|');
        out.push_str(&di(n.velocity));
        out.push('|');
        out.push_str(b01(n.muted));
        out.push('\n');
    }
    Ok(out)
}

/// Canonical string for `note_selection_hash` (§9.4).
///
/// Same note ordering as [`midi_canonical`].
pub fn selection_canonical(notes: &[TakeNote]) -> Result<String, IpcError> {
    let mut out = String::with_capacity(32 + notes.len() * 8);
    out.push_str("qlabs.selection.v1\n");
    out.push_str(&di(notes.len() as i64));
    out.push('\n');
    for (i, n) in notes.iter().enumerate() {
        out.push_str(&di(i as i64));
        out.push('|');
        out.push_str(b01(n.selected));
        out.push('\n');
    }
    Ok(out)
}

/// Canonical string for `note_list_hash` (§9.7).
///
/// Over the **filtered** note array that appears in the result, in that exact
/// order, in the project quarter-note domain. Note the field order differs from
/// [`midi_canonical`]: `pitch|velocity|channel` here, `channel|pitch|velocity`
/// there. That asymmetry is intentional and present on both sides.
pub fn note_list_canonical(notes: &[ListNote]) -> Result<String, IpcError> {
    let mut out = String::with_capacity(32 + notes.len() * 48);
    out.push_str("qlabs.notes.v1\n");
    out.push_str(&di(notes.len() as i64));
    out.push('\n');
    for (i, n) in notes.iter().enumerate() {
        out.push_str(&di(i as i64));
        out.push('|');
        out.push_str(&f6r("start_qn", n.start_qn)?);
        out.push('|');
        out.push_str(&f6r("end_qn", n.end_qn)?);
        out.push('|');
        out.push_str(&di(n.pitch));
        out.push('|');
        out.push_str(&di(n.velocity));
        out.push('|');
        out.push_str(&di(n.channel));
        out.push('|');
        out.push_str(b01(n.muted));
        out.push('|');
        out.push_str(b01(n.selected));
        out.push('\n');
    }
    Ok(out)
}

/// Canonical string for `tempo_map_hash` (§9.5).
///
/// Over the markers in REAPER's own index order, or over the single synthetic
/// marker the bridge reports for a project with no explicit markers.
pub fn tempo_canonical(markers: &[TempoMarker]) -> Result<String, IpcError> {
    let mut out = String::with_capacity(32 + markers.len() * 56);
    out.push_str("qlabs.tempo.v1\n");
    out.push_str(&di(markers.len() as i64));
    out.push('\n');
    for (i, m) in markers.iter().enumerate() {
        out.push_str(&di(i as i64));
        out.push('|');
        out.push_str(&f6r("time_seconds", m.time_seconds)?);
        out.push('|');
        out.push_str(&f6r("qn", m.qn)?);
        out.push('|');
        out.push_str(&f6r("bpm", m.bpm)?);
        out.push('|');
        out.push_str(&di(m.timesig_num));
        out.push('|');
        out.push_str(&di(m.timesig_den));
        out.push('|');
        out.push_str(b01(m.linear));
        out.push('\n');
    }
    Ok(out)
}

/// Canonical string for `timesig_map_hash` (§9.6).
///
/// Only markers carrying both `timesig_num > 0` and `timesig_den > 0`,
/// re-indexed from 0 in marker order. Tempo is deliberately excluded.
pub fn timesig_canonical(markers: &[TempoMarker]) -> Result<String, IpcError> {
    let rows: Vec<&TempoMarker> = markers
        .iter()
        .filter(|m| m.timesig_num > 0 && m.timesig_den > 0)
        .collect();
    let mut out = String::with_capacity(32 + rows.len() * 32);
    out.push_str("qlabs.timesig.v1\n");
    out.push_str(&di(rows.len() as i64));
    out.push('\n');
    for (i, m) in rows.iter().enumerate() {
        out.push_str(&di(i as i64));
        out.push('|');
        out.push_str(&f6r("qn", m.qn)?);
        out.push('|');
        out.push_str(&di(m.timesig_num));
        out.push('|');
        out.push_str(&di(m.timesig_den));
        out.push('\n');
    }
    Ok(out)
}

/// Canonical string for `snapshot_hash` (§9.8).
///
/// A line-oriented `key=value` block. Field order is fixed and must not be
/// sorted. The embedded hash strings are the full prefixed forms.
pub fn snapshot_canonical(s: &SnapshotFields) -> Result<String, IpcError> {
    let mut out = String::with_capacity(640);
    out.push_str("qlabs.snapshot.v1\n");
    let kv = |k: &str, v: &str, out: &mut String| {
        out.push_str(k);
        out.push('=');
        out.push_str(v);
        out.push('\n');
    };
    kv(
        "project_uuid",
        s_or_null(s.project_uuid.as_deref()),
        &mut out,
    );
    kv("track_guid", s_or_null(s.track_guid.as_deref()), &mut out);
    kv("item_guid", s_or_null(s.item_guid.as_deref()), &mut out);
    kv("take_guid", s_or_null(s.take_guid.as_deref()), &mut out);
    kv(
        "item_position_seconds",
        &f6r("item_position_seconds", s.item_position_seconds)?,
        &mut out,
    );
    kv(
        "item_length_seconds",
        &f6r("item_length_seconds", s.item_length_seconds)?,
        &mut out,
    );
    kv(
        "item_position_qn",
        &f6r("item_position_qn", s.item_position_qn)?,
        &mut out,
    );
    kv(
        "item_length_qn",
        &f6r("item_length_qn", s.item_length_qn)?,
        &mut out,
    );
    kv("is_loop_source", b01(s.is_loop_source), &mut out);
    kv("note_scope", s_or_null(s.note_scope.as_deref()), &mut out);
    kv(
        "extraction_mode",
        s_or_null(s.extraction_mode.as_deref()),
        &mut out,
    );
    kv(
        "extraction_channel",
        &di(s.extraction_channel.unwrap_or(-1)),
        &mut out,
    );
    kv("midi_hash", s_or_null(s.midi_hash.as_deref()), &mut out);
    kv(
        "tempo_map_hash",
        s_or_null(s.tempo_map_hash.as_deref()),
        &mut out,
    );
    kv(
        "timesig_map_hash",
        s_or_null(s.timesig_map_hash.as_deref()),
        &mut out,
    );
    kv(
        "note_selection_hash",
        s_or_null(s.note_selection_hash.as_deref()),
        &mut out,
    );
    kv(
        "note_list_hash",
        s_or_null(s.note_list_hash.as_deref()),
        &mut out,
    );
    kv("note_count", &di(s.note_count), &mut out);
    Ok(out)
}

/// Convenience: hash a canonical string produced by one of the builders above.
pub fn hash_canonical(canonical: &str) -> String {
    qlabs_hash(canonical.as_bytes())
}

/// Reports a hash that did not reproduce locally.
///
/// This is an error, never a warning: a snapshot whose own hashes do not
/// reproduce cannot be used as a staleness token.
pub fn mismatch(field: &str, expected: &str, actual: &str, canonical: &str) -> IpcError {
    IpcError::with_details(
        codes::SNAPSHOT_HASH_MISMATCH,
        format!("{field} does not reproduce locally: bridge sent {expected}, recomputed {actual}"),
        qjson::json_obj! {
            "field" => field,
            "expected" => expected,
            "actual" => actual,
            "canonical_string_bytes" => canonical.len(),
        },
    )
}

/// Recomputes `field`'s hash from `canonical` and compares it with `expected`.
pub fn check(field: &str, expected: &str, canonical: &str) -> Result<(), IpcError> {
    let actual = hash_canonical(canonical);
    if actual == expected {
        Ok(())
    } else {
        Err(mismatch(field, expected, &actual, canonical))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qjson::Json;

    fn vectors() -> Json {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/mock-reaper/hashes/vectors.json"
        );
        let text = std::fs::read_to_string(path).expect("hash vectors fixture");
        Json::parse(&text).expect("parses")
    }

    fn vector(name: &str) -> (String, String) {
        let doc = vectors();
        for v in doc.arr_field("vectors").expect("vectors") {
            if v.str_field("name").expect("name") == name {
                return (
                    v.str_field("canonical_string").expect("cs").to_string(),
                    v.str_field("hash").expect("hash").to_string(),
                );
            }
        }
        panic!("no vector named {name}");
    }

    #[test]
    fn algorithm_known_answer_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn every_recorded_vector_reproduces() {
        let doc = vectors();
        let list = doc.arr_field("vectors").expect("vectors");
        assert!(list.len() >= 11, "expected the full vector set");
        for v in list {
            let name = v.str_field("name").expect("name");
            let cs = v.str_field("canonical_string").expect("canonical_string");
            let want = v.str_field("hash").expect("hash");
            assert_eq!(qlabs_hash(cs.as_bytes()), want, "vector {name}");
        }
    }

    #[test]
    fn rendered_form_is_prefixed_and_zero_padded() {
        assert_eq!(qlabs_hash(b""), "fnv1a64:cbf29ce484222325");
        // A digest with leading zero nibbles must keep them.
        let h = qlabs_hash(b"qlabs.timesig.v1\n1\n0|0.000000|4|4\n");
        assert_eq!(h, "fnv1a64:0fa04d651f4255a8");
        assert_eq!(h.len(), HASH_PREFIX.len() + 16);
    }

    #[test]
    fn hash_string_shape_predicate() {
        assert!(is_hash_string("fnv1a64:0fa04d651f4255a8"));
        assert!(!is_hash_string("fnv1a64:0FA04D651F4255A8"));
        assert!(!is_hash_string("fnv1a64:short"));
        assert!(!is_hash_string("sha256:0fa04d651f4255a8"));
        assert!(!is_hash_string(""));
    }

    #[test]
    fn f6_renders_six_fractional_digits() {
        assert_eq!(f6(0.0).expect("f6"), "0.000000");
        assert_eq!(f6(960.0).expect("f6"), "960.000000");
        assert_eq!(f6(-1.5).expect("f6"), "-1.500000");
        assert_eq!(f6(1.0 / 3.0).expect("f6"), "0.333333");
    }

    #[test]
    fn f6_normalises_negative_zero() {
        assert_eq!(f6(-0.0).expect("f6"), "0.000000");
        assert!(!f6(-0.0).expect("f6").starts_with('-'));
    }

    #[test]
    fn f6_rejects_non_finite_and_huge_values() {
        assert!(f6(f64::NAN).is_none());
        assert!(f6(f64::INFINITY).is_none());
        assert!(f6(f64::NEG_INFINITY).is_none());
        assert!(f6(1e13).is_none());
        assert!(f6(-1e13).is_none());
        assert!(f6(1e12).is_some());
    }

    #[test]
    fn d_renders_integers_without_padding_and_rounds_like_the_bridge() {
        assert_eq!(d(0.0).expect("d"), "0");
        assert_eq!(d(-7.0).expect("d"), "-7");
        assert_eq!(d(127.0).expect("d"), "127");
        // The bridge falls back to floor(x + 0.5).
        assert_eq!(d(2.4).expect("d"), "2");
        assert_eq!(d(2.5).expect("d"), "3");
        assert_eq!(d(-2.5).expect("d"), "-2");
        assert!(d(f64::NAN).is_none());
    }

    #[test]
    fn b01_and_s_or_null_render_the_documented_tokens() {
        assert_eq!(b01(true), "1");
        assert_eq!(b01(false), "0");
        assert_eq!(s_or_null(None), "null");
        assert_eq!(s_or_null(Some("")), "null");
        assert_eq!(s_or_null(Some("abc")), "abc");
    }

    fn scene_take_notes() -> Vec<TakeNote> {
        vec![
            TakeNote {
                start_ppq: 0.0,
                end_ppq: 960.0,
                channel: 0,
                pitch: 60,
                velocity: 100,
                muted: false,
                selected: true,
            },
            TakeNote {
                start_ppq: 960.0,
                end_ppq: 1920.0,
                channel: 0,
                pitch: 62,
                velocity: 96,
                muted: false,
                selected: true,
            },
            TakeNote {
                start_ppq: 1920.0,
                end_ppq: 2880.0,
                channel: 0,
                pitch: 64,
                velocity: 92,
                muted: false,
                selected: true,
            },
            TakeNote {
                start_ppq: 2880.0,
                end_ppq: 3840.0,
                channel: 0,
                pitch: 65,
                velocity: 88,
                muted: false,
                selected: false,
            },
        ]
    }

    fn scene_list_notes() -> Vec<ListNote> {
        vec![
            ListNote {
                start_qn: 0.0,
                end_qn: 1.0,
                pitch: 60,
                velocity: 100,
                channel: 0,
                muted: false,
                selected: true,
            },
            ListNote {
                start_qn: 1.0,
                end_qn: 2.0,
                pitch: 62,
                velocity: 96,
                channel: 0,
                muted: false,
                selected: true,
            },
            ListNote {
                start_qn: 2.0,
                end_qn: 3.0,
                pitch: 64,
                velocity: 92,
                channel: 0,
                muted: false,
                selected: true,
            },
            ListNote {
                start_qn: 3.0,
                end_qn: 4.0,
                pitch: 65,
                velocity: 88,
                channel: 0,
                muted: false,
                selected: false,
            },
        ]
    }

    fn scene_markers() -> Vec<TempoMarker> {
        vec![TempoMarker {
            time_seconds: 0.0,
            qn: 0.0,
            bpm: 120.0,
            timesig_num: 4,
            timesig_den: 4,
            linear: false,
        }]
    }

    #[test]
    fn midi_canonical_matches_the_empty_take_vector() {
        let (cs, hash) = vector("midi_hash.empty_take");
        let built = midi_canonical(&[]).expect("build");
        assert_eq!(built, cs);
        assert_eq!(built, "qlabs.midi.v1\n0\n");
        assert_eq!(hash_canonical(&built), hash);
    }

    #[test]
    fn midi_canonical_matches_the_one_note_vector() {
        let (cs, hash) = vector("midi_hash.one_note");
        let built = midi_canonical(&scene_take_notes()[..1]).expect("build");
        assert_eq!(built, cs);
        assert_eq!(hash_canonical(&built), hash);
    }

    #[test]
    fn midi_canonical_matches_the_scene_vector() {
        let (cs, hash) = vector("midi_hash.scene");
        let built = midi_canonical(&scene_take_notes()).expect("build");
        assert_eq!(built, cs);
        assert_eq!(hash_canonical(&built), hash);
    }

    #[test]
    fn selection_canonical_matches_the_scene_vector() {
        let (cs, hash) = vector("note_selection_hash.scene");
        let built = selection_canonical(&scene_take_notes()).expect("build");
        assert_eq!(built, cs);
        assert_eq!(hash_canonical(&built), hash);
    }

    #[test]
    fn note_list_canonical_matches_the_scene_vector() {
        let (cs, hash) = vector("note_list_hash.scene");
        let built = note_list_canonical(&scene_list_notes()).expect("build");
        assert_eq!(built, cs);
        assert_eq!(hash_canonical(&built), hash);
    }

    #[test]
    fn tempo_canonical_matches_the_scene_vector() {
        let (cs, hash) = vector("tempo_map_hash.scene");
        let built = tempo_canonical(&scene_markers()).expect("build");
        assert_eq!(built, cs);
        assert_eq!(hash_canonical(&built), hash);
    }

    #[test]
    fn timesig_canonical_matches_the_scene_vector() {
        let (cs, hash) = vector("timesig_map_hash.scene");
        let built = timesig_canonical(&scene_markers()).expect("build");
        assert_eq!(built, cs);
        assert_eq!(hash_canonical(&built), hash);
    }

    #[test]
    fn timesig_canonical_drops_markers_without_a_signature_and_reindexes() {
        let markers = vec![
            TempoMarker {
                time_seconds: 0.0,
                qn: 0.0,
                bpm: 120.0,
                timesig_num: 0,
                timesig_den: 0,
                linear: false,
            },
            TempoMarker {
                time_seconds: 2.0,
                qn: 4.0,
                bpm: 140.0,
                timesig_num: 3,
                timesig_den: 4,
                linear: false,
            },
        ];
        let built = timesig_canonical(&markers).expect("build");
        assert_eq!(built, "qlabs.timesig.v1\n1\n0|4.000000|3|4\n");
    }

    #[test]
    fn snapshot_canonical_matches_the_scene_vector() {
        let (cs, hash) = vector("snapshot_hash.scene");
        let fields = SnapshotFields {
            project_uuid: Some("00000000-0000-4000-8000-000000000001".into()),
            track_guid: Some("00000001-0001-4001-8001-000000000001".into()),
            item_guid: Some("00000002-0002-4002-8002-000000000002".into()),
            take_guid: Some("00000003-0003-4003-8003-000000000003".into()),
            item_position_seconds: 0.0,
            item_length_seconds: 4.0,
            item_position_qn: 0.0,
            item_length_qn: 8.0,
            is_loop_source: false,
            note_scope: Some("all".into()),
            extraction_mode: Some("auto".into()),
            extraction_channel: None,
            midi_hash: Some("fnv1a64:fac2c019eba29541".into()),
            tempo_map_hash: Some("fnv1a64:387bdc9bc9f1446a".into()),
            timesig_map_hash: Some("fnv1a64:0fa04d651f4255a8".into()),
            note_selection_hash: Some("fnv1a64:5fc9d7c416e376f8".into()),
            note_list_hash: Some("fnv1a64:8a202eb33de32f29".into()),
            note_count: 4,
        };
        let built = snapshot_canonical(&fields).expect("build");
        assert_eq!(built, cs);
        assert_eq!(hash_canonical(&built), hash);
    }

    #[test]
    fn snapshot_canonical_uses_minus_one_for_an_absent_channel() {
        let mut f = SnapshotFields {
            extraction_channel: None,
            ..SnapshotFields::default()
        };
        let a = snapshot_canonical(&f).expect("build");
        assert!(a.contains("\nextraction_channel=-1\n"));
        f.extraction_channel = Some(0);
        let b = snapshot_canonical(&f).expect("build");
        assert!(b.contains("\nextraction_channel=0\n"));
        assert_ne!(a, b, "channel 0 must not collapse into no-channel");
    }

    #[test]
    fn snapshot_canonical_field_order_is_fixed_and_unsorted() {
        let built = snapshot_canonical(&SnapshotFields::default()).expect("build");
        let keys: Vec<&str> = built
            .lines()
            .skip(1)
            .filter_map(|l| l.split('=').next())
            .collect();
        assert_eq!(
            keys,
            vec![
                "project_uuid",
                "track_guid",
                "item_guid",
                "take_guid",
                "item_position_seconds",
                "item_length_seconds",
                "item_position_qn",
                "item_length_qn",
                "is_loop_source",
                "note_scope",
                "extraction_mode",
                "extraction_channel",
                "midi_hash",
                "tempo_map_hash",
                "timesig_map_hash",
                "note_selection_hash",
                "note_list_hash",
                "note_count",
            ]
        );
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_ne!(sorted, keys, "the block must not be sorted");
    }

    #[test]
    fn every_canonical_string_ends_with_a_newline() {
        assert!(midi_canonical(&scene_take_notes())
            .expect("b")
            .ends_with('\n'));
        assert!(selection_canonical(&scene_take_notes())
            .expect("b")
            .ends_with('\n'));
        assert!(note_list_canonical(&scene_list_notes())
            .expect("b")
            .ends_with('\n'));
        assert!(tempo_canonical(&scene_markers())
            .expect("b")
            .ends_with('\n'));
        assert!(timesig_canonical(&scene_markers())
            .expect("b")
            .ends_with('\n'));
        assert!(snapshot_canonical(&SnapshotFields::default())
            .expect("b")
            .ends_with('\n'));
    }

    #[test]
    fn sort_take_notes_uses_the_documented_key_order() {
        let mut notes = vec![
            TakeNote {
                start_ppq: 0.0,
                end_ppq: 480.0,
                channel: 1,
                pitch: 60,
                velocity: 90,
                muted: false,
                selected: false,
            },
            TakeNote {
                start_ppq: 0.0,
                end_ppq: 480.0,
                channel: 0,
                pitch: 60,
                velocity: 90,
                muted: false,
                selected: false,
            },
            TakeNote {
                start_ppq: 0.0,
                end_ppq: 480.0,
                channel: 0,
                pitch: 48,
                velocity: 90,
                muted: false,
                selected: false,
            },
        ];
        sort_take_notes(&mut notes);
        assert_eq!(notes[0].pitch, 48);
        assert_eq!(notes[1].channel, 0);
        assert_eq!(notes[2].channel, 1);
    }

    #[test]
    fn a_non_representable_number_is_an_error_not_a_panic() {
        let notes = vec![TakeNote {
            start_ppq: f64::INFINITY,
            end_ppq: 1.0,
            channel: 0,
            pitch: 60,
            velocity: 90,
            muted: false,
            selected: false,
        }];
        let err = midi_canonical(&notes).expect_err("non-finite");
        assert_eq!(err.code, codes::SNAPSHOT_HASH_MISMATCH);
    }

    #[test]
    fn check_accepts_a_matching_digest_and_rejects_a_wrong_one() {
        let cs = "qlabs.midi.v1\n0\n";
        check("midi_hash", "fnv1a64:177ff0b49402e75a", cs).expect("matches");
        let err = check("midi_hash", "fnv1a64:0000000000000000", cs).expect_err("differs");
        assert_eq!(err.code, codes::SNAPSHOT_HASH_MISMATCH);
        assert_eq!(err.detail_str("field"), Some("midi_hash"));
        assert_eq!(err.detail_str("actual"), Some("fnv1a64:177ff0b49402e75a"));
    }

    #[test]
    fn a_single_flipped_bit_changes_the_digest() {
        let a = hash_canonical("qlabs.midi.v1\n1\n0|0.000000|960.000000|0|60|100|0\n");
        let b = hash_canonical("qlabs.midi.v1\n1\n0|0.000000|960.000000|0|60|101|0\n");
        assert_ne!(a, b);
    }
}
