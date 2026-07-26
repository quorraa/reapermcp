//! Small deterministic helpers shared by the analysis stages.
//!
//! Two of these carry product weight rather than convenience:
//!
//! * [`round6`] is applied to **every** `f64` that reaches JSON. Analysis
//!   scores are sums of many small products, and the last bits of an `f64` are
//!   not something a golden file should be pinned to; six decimals is far more
//!   precision than any musical decision uses.
//! * [`cmp_f64`] gives a total order over scores without `partial_cmp`
//!   unwrapping, so a `NaN` produced by degenerate input can never panic a sort.

use music_domain::prelude::*;
use qjson::Json;
use std::cmp::Ordering;

/// Rounds to six decimal places, mapping `-0.0` to `0.0` and any non-finite
/// value to `0.0` so serialisation is always well-defined.
pub(crate) fn round6(v: f64) -> f64 {
    if !v.is_finite() {
        return 0.0;
    }
    let r = (v * 1_000_000.0).round() / 1_000_000.0;
    if r == 0.0 {
        0.0
    } else {
        r
    }
}

/// A rounded `f64` as JSON.
pub(crate) fn num(v: f64) -> Json {
    Json::Float(round6(v))
}

/// A total order over `f64` that treats `NaN` as the smallest value.
pub(crate) fn cmp_f64(a: f64, b: f64) -> Ordering {
    match (a.is_nan(), b.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
    }
}

/// Clamps to `0.0..=1.0`, mapping non-finite input to `0.0`.
pub(crate) fn unit(v: f64) -> f64 {
    if !v.is_finite() {
        return 0.0;
    }
    v.clamp(0.0, 1.0)
}

/// A JSON array of note ids.
pub(crate) fn id_array(ids: &[NoteId]) -> Json {
    Json::Arr(ids.iter().map(|i| Json::Int(*i as i64)).collect())
}

/// A JSON array of strings.
pub(crate) fn str_array<S: AsRef<str>>(items: &[S]) -> Json {
    Json::Arr(
        items
            .iter()
            .map(|s| Json::Str(s.as_ref().to_string()))
            .collect(),
    )
}

/// A JSON array of warnings.
pub(crate) fn warning_array(items: &[Warning]) -> Json {
    Json::Arr(items.iter().map(Warning::to_json).collect())
}

/// The median of a set of quarter-note lengths, or `None` when empty.
///
/// The median rather than the mean, because one held final note should not
/// redefine what "a normal note length" means for the phrase around it.
pub(crate) fn median_duration(notes: &[Note]) -> Option<BeatTime> {
    if notes.is_empty() {
        return None;
    }
    let mut d: Vec<BeatTime> = notes.iter().map(|n| n.duration).collect();
    d.sort();
    Some(d[d.len() / 2])
}

/// Spelled-pitch-class text such as `"C#"`, used in evidence strings and ids.
pub(crate) fn class_text(tonic: (Letter, Accidental)) -> String {
    format!("{}{}", tonic.0.as_char(), tonic.1.ascii())
}

/// Sounding pitch class of a spelled class, `0..=11`.
pub(crate) fn class_pc(tonic: (Letter, Accidental)) -> i32 {
    (tonic.0.natural_pc() + tonic.1 .0 as i32).rem_euclid(12)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_is_stable_and_kills_negative_zero() {
        assert_eq!(round6(1.0 / 3.0), 0.333333);
        assert_eq!(round6(-0.0), 0.0);
        assert_eq!(round6(f64::NAN), 0.0);
        assert_eq!(round6(f64::INFINITY), 0.0);
        assert_eq!(round6(0.1 + 0.2), 0.3);
    }

    #[test]
    fn nan_sorts_last_without_panicking() {
        let mut v = [1.0, f64::NAN, 0.5];
        v.sort_by(|a, b| cmp_f64(*b, *a));
        assert_eq!(v[0], 1.0);
        assert_eq!(v[1], 0.5);
        assert!(v[2].is_nan());
    }

    #[test]
    fn unit_clamps() {
        assert_eq!(unit(-3.0), 0.0);
        assert_eq!(unit(3.0), 1.0);
        assert_eq!(unit(0.25), 0.25);
    }

    #[test]
    fn pitch_class_helpers() {
        assert_eq!(class_pc((Letter::C, Accidental::SHARP)), 1);
        assert_eq!(class_pc((Letter::C, Accidental::FLAT)), 11);
        assert_eq!(class_text((Letter::B, Accidental::FLAT)), "Bb");
    }

    #[test]
    fn median_of_an_empty_set_is_none() {
        assert_eq!(median_duration(&[]), None);
    }
}
