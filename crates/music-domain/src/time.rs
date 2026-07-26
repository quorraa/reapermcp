//! Canonical musical time: exact rational quarter-note positions, time
//! signatures, and the tempo/meter map.
//!
//! Floating-point PPQ values are kept only as provenance. Everything the
//! analysis and generation stages compare — loop boundaries, chord spans, grid
//! alignment — uses [`BeatTime`], a normalized rational number of quarter notes
//! with exact `Eq`, `Ord` and `Hash`.

use crate::error::DomainError;
use qjson::{json_obj, Json};
use std::cmp::Ordering;
use std::fmt;
use std::ops::{Add, Mul, Neg, Sub};
use std::str::FromStr;

/// Reference pulses-per-quarter-note used by REAPER. Retained for provenance
/// only; no arithmetic in this crate is done in PPQ.
pub const PPQ: i64 = 960;

/// Default snap grid denominator: `1920 = 2^7 * 3 * 5`, which represents
/// 128th notes, triplets and quintuplets exactly.
pub const GRID_DEN: i64 = 1920;

/// Largest magnitude, in whole quarter notes, that [`BeatTime`] arithmetic will
/// produce before saturating.
pub const MAX_QUARTERS: i64 = i64::MAX / GRID_DEN;

/// Greatest common divisor of two non-negative 128-bit integers.
fn gcd_i128(mut a: i128, mut b: i128) -> i128 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a.abs()
}

/// Floor division for 128-bit integers; returns 0 when `b == 0`.
fn floor_div_i128(a: i128, b: i128) -> i128 {
    if b == 0 {
        return 0;
    }
    let q = a / b;
    let r = a % b;
    if r != 0 && ((r < 0) != (b < 0)) {
        q - 1
    } else {
        q
    }
}

/// An exact musical position or duration measured in quarter notes.
///
/// Invariants: `den > 0` and `gcd(|num|, den) == 1`. Values are therefore
/// canonical, which is what makes `Eq`/`Hash` agree — `2/4` and `1/2` are the
/// same value *and* hash identically.
///
/// All arithmetic is performed in `i128` and re-normalized. A result that
/// cannot be represented exactly in `i64/i64` is approximated on the
/// `1/GRID_DEN` grid, and magnitudes beyond [`MAX_QUARTERS`] saturate at
/// [`BeatTime::MAX`] / [`BeatTime::MIN`] rather than wrapping.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct BeatTime {
    num: i64,
    den: i64,
}

impl BeatTime {
    /// Zero quarter notes.
    pub const ZERO: BeatTime = BeatTime { num: 0, den: 1 };
    /// One quarter note.
    pub const ONE: BeatTime = BeatTime { num: 1, den: 1 };
    /// Largest representable position.
    pub const MAX: BeatTime = BeatTime {
        num: MAX_QUARTERS,
        den: 1,
    };
    /// Smallest (most negative) representable position.
    pub const MIN: BeatTime = BeatTime {
        num: -MAX_QUARTERS,
        den: 1,
    };

    /// Builds a normalized rational.
    ///
    /// # Panics
    ///
    /// Panics when `den == 0`. Use [`BeatTime::try_new`] for input that may be
    /// malformed; nothing in this crate calls `new` with an unchecked
    /// denominator.
    pub fn new(num: i64, den: i64) -> BeatTime {
        BeatTime::try_new(num, den).expect("BeatTime denominator must be non-zero")
    }

    /// Builds a normalized rational, returning `None` when `den == 0`.
    pub fn try_new(num: i64, den: i64) -> Option<BeatTime> {
        if den == 0 {
            return None;
        }
        Some(BeatTime::from_i128(num as i128, den as i128))
    }

    /// Normalizes an arbitrary 128-bit rational into a `BeatTime`, saturating
    /// or snapping to the grid when it does not fit.
    fn from_i128(mut num: i128, mut den: i128) -> BeatTime {
        if den == 0 {
            return BeatTime::ZERO;
        }
        if den < 0 {
            num = -num;
            den = -den;
        }
        if num == 0 {
            return BeatTime::ZERO;
        }
        let g = gcd_i128(num, den);
        if g > 1 {
            num /= g;
            den /= g;
        }
        let limit = i64::MAX as i128;
        if num.abs() <= limit && den <= limit {
            return BeatTime {
                num: num as i64,
                den: den as i64,
            };
        }
        // Cannot be represented exactly: fall back to the grid, saturating.
        BeatTime::from_f64(num as f64 / den as f64)
    }

    /// A whole number of quarter notes.
    pub fn from_quarters(q: i64) -> BeatTime {
        BeatTime::from_i128(q as i128, 1)
    }

    /// Snaps a floating-point quarter-note value to the nearest `1/1920`.
    ///
    /// Non-finite input becomes [`BeatTime::ZERO`]; magnitudes beyond
    /// [`MAX_QUARTERS`] saturate.
    pub fn from_f64(q: f64) -> BeatTime {
        BeatTime::from_f64_grid(q, GRID_DEN)
    }

    /// Snaps a floating-point quarter-note value to the nearest `1/grid_den`.
    ///
    /// A `grid_den` of zero or below is treated as 1 (whole quarter notes).
    pub fn from_f64_grid(q: f64, grid_den: i64) -> BeatTime {
        if !q.is_finite() {
            return BeatTime::ZERO;
        }
        let den = if grid_den <= 0 { 1 } else { grid_den };
        let max = MAX_QUARTERS as f64;
        let clamped = q.clamp(-max, max);
        let scaled = (clamped * den as f64).round();
        // Float-to-int `as` casts saturate in Rust, so this cannot wrap.
        BeatTime::from_i128(scaled as i64 as i128, den as i128)
    }

    /// Approximate value in quarter notes.
    pub fn as_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// Numerator of the reduced fraction.
    pub fn num(self) -> i64 {
        self.num
    }

    /// Denominator of the reduced fraction; always positive.
    pub fn den(self) -> i64 {
        self.den
    }

    /// True for exactly zero.
    pub fn is_zero(self) -> bool {
        self.num == 0
    }

    /// True for values strictly greater than zero.
    pub fn is_positive(self) -> bool {
        self.num > 0
    }

    /// True for values strictly less than zero.
    pub fn is_negative(self) -> bool {
        self.num < 0
    }

    /// The smaller of two values.
    pub fn min(self, o: Self) -> Self {
        if self <= o {
            self
        } else {
            o
        }
    }

    /// The larger of two values.
    pub fn max(self, o: Self) -> Self {
        if self >= o {
            self
        } else {
            o
        }
    }

    /// Absolute value.
    pub fn abs(self) -> Self {
        if self.num < 0 {
            BeatTime::from_i128(-(self.num as i128), self.den as i128)
        } else {
            self
        }
    }

    /// Euclidean remainder: always in `[0, |m|)`. Returns `self` when `m` is zero.
    pub fn rem_euclid(self, m: BeatTime) -> BeatTime {
        if m.is_zero() {
            return self;
        }
        let q = self.div_floor(m) as i128;
        let num = self.num as i128 * m.den as i128 - q * m.num as i128 * self.den as i128;
        let den = self.den as i128 * m.den as i128;
        BeatTime::from_i128(num, den)
    }

    /// Floor of `self / m`. Returns 0 when `m` is zero.
    pub fn div_floor(self, m: BeatTime) -> i64 {
        if m.is_zero() {
            return 0;
        }
        let a = self.num as i128 * m.den as i128;
        let b = self.den as i128 * m.num as i128;
        let q = floor_div_i128(a, b);
        q.clamp(i64::MIN as i128, i64::MAX as i128) as i64
    }

    /// True when `self` is an exact multiple of `m` (a zero `m` divides only zero).
    pub fn divides_evenly_by(self, m: BeatTime) -> bool {
        if m.is_zero() {
            return self.is_zero();
        }
        self.rem_euclid(m).is_zero()
    }

    /// Multiplies by the rational `num/den`. A zero `den` leaves the value
    /// unchanged.
    pub fn scale(self, num: i64, den: i64) -> BeatTime {
        if den == 0 {
            return self;
        }
        BeatTime::from_i128(
            self.num as i128 * num as i128,
            self.den as i128 * den as i128,
        )
    }

    /// Canonical rational text: `"3"`, `"1/2"`, `"-1/4"`, `"7/3"`.
    pub fn to_display(self) -> String {
        if self.den == 1 {
            self.num.to_string()
        } else {
            format!("{}/{}", self.num, self.den)
        }
    }

    /// Parses the canonical rational text, e.g. `"0"`, `"3"`, `"-1/4"`, `"7/3"`.
    ///
    /// Whitespace around the value and around the solidus is ignored. A bare
    /// decimal such as `"1.5"` is also accepted and snapped to the grid, so
    /// hand-written fixtures cannot silently produce a wrong position.
    pub fn parse(s: &str) -> Result<BeatTime, DomainError> {
        let text = s.trim();
        if text.is_empty() {
            return Err(DomainError::invalid_time("empty musical time"));
        }
        if let Some((n, d)) = text.split_once('/') {
            let num: i64 = n
                .trim()
                .parse()
                .map_err(|_| DomainError::invalid_time(format!("bad numerator in {text:?}")))?;
            let den: i64 = d
                .trim()
                .parse()
                .map_err(|_| DomainError::invalid_time(format!("bad denominator in {text:?}")))?;
            return BeatTime::try_new(num, den)
                .ok_or_else(|| DomainError::invalid_time(format!("zero denominator in {text:?}")));
        }
        if let Ok(n) = text.parse::<i64>() {
            return Ok(BeatTime::from_quarters(n));
        }
        if let Ok(f) = text.parse::<f64>() {
            if f.is_finite() {
                return Ok(BeatTime::from_f64(f));
            }
        }
        Err(DomainError::invalid_time(format!(
            "not a musical time: {text:?}"
        )))
    }

    /// JSON form: the canonical rational string.
    pub fn to_json(self) -> Json {
        Json::Str(self.to_display())
    }

    /// Reads the JSON form, accepting either the rational string or a number.
    pub fn from_json(v: &Json) -> Result<BeatTime, DomainError> {
        match v {
            Json::Str(s) => BeatTime::parse(s),
            Json::Int(i) => Ok(BeatTime::from_quarters(*i)),
            Json::Float(f) => Ok(BeatTime::from_f64(*f)),
            other => Err(DomainError::invalid_time(format!(
                "expected a rational string, found {}",
                other.type_name()
            ))),
        }
    }
}

impl Default for BeatTime {
    fn default() -> Self {
        BeatTime::ZERO
    }
}

impl Add for BeatTime {
    type Output = BeatTime;
    fn add(self, o: BeatTime) -> BeatTime {
        BeatTime::from_i128(
            self.num as i128 * o.den as i128 + o.num as i128 * self.den as i128,
            self.den as i128 * o.den as i128,
        )
    }
}

impl Sub for BeatTime {
    type Output = BeatTime;
    fn sub(self, o: BeatTime) -> BeatTime {
        BeatTime::from_i128(
            self.num as i128 * o.den as i128 - o.num as i128 * self.den as i128,
            self.den as i128 * o.den as i128,
        )
    }
}

impl Neg for BeatTime {
    type Output = BeatTime;
    fn neg(self) -> BeatTime {
        BeatTime::from_i128(-(self.num as i128), self.den as i128)
    }
}

impl Mul<i64> for BeatTime {
    type Output = BeatTime;
    fn mul(self, rhs: i64) -> BeatTime {
        BeatTime::from_i128(self.num as i128 * rhs as i128, self.den as i128)
    }
}

impl PartialOrd for BeatTime {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BeatTime {
    fn cmp(&self, other: &Self) -> Ordering {
        let a = self.num as i128 * other.den as i128;
        let b = other.num as i128 * self.den as i128;
        a.cmp(&b)
    }
}

impl fmt::Display for BeatTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_display())
    }
}

impl FromStr for BeatTime {
    type Err = DomainError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        BeatTime::parse(s)
    }
}

impl std::iter::Sum for BeatTime {
    fn sum<I: Iterator<Item = BeatTime>>(iter: I) -> BeatTime {
        iter.fold(BeatTime::ZERO, |a, b| a + b)
    }
}

/// A time signature such as 4/4, 6/8 or 7/8.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TimeSignature {
    /// Beats per bar.
    pub numerator: u16,
    /// Beat unit as a note-value denominator (4 = quarter, 8 = eighth).
    pub denominator: u16,
}

impl Default for TimeSignature {
    /// Common time.
    fn default() -> Self {
        TimeSignature {
            numerator: 4,
            denominator: 4,
        }
    }
}

impl TimeSignature {
    /// Builds a time signature, substituting 1 for a zero numerator and 4 for a
    /// zero denominator so that no downstream division can fail.
    pub fn new(n: u16, d: u16) -> Self {
        TimeSignature {
            numerator: if n == 0 { 1 } else { n },
            denominator: if d == 0 { 4 } else { d },
        }
    }

    /// Bar length in quarter notes: `4 * numerator / denominator`.
    pub fn bar_length_qn(self) -> BeatTime {
        BeatTime::new(4 * self.numerator as i64, self.denominator as i64)
    }

    /// Length of one notated beat in quarter notes: `4 / denominator`.
    pub fn beat_unit_qn(self) -> BeatTime {
        BeatTime::new(4, self.denominator as i64)
    }

    /// True for compound meters (6/8, 9/8, 12/8, 6/16 …), where the notated
    /// beats group in threes.
    pub fn is_compound(self) -> bool {
        self.numerator % 3 == 0 && self.numerator > 3 && self.denominator >= 8
    }

    /// Length of one *felt* beat: the dotted grouping in compound meters, the
    /// notated beat otherwise.
    pub fn pulse_qn(self) -> BeatTime {
        if self.is_compound() {
            self.beat_unit_qn() * 3
        } else {
            self.beat_unit_qn()
        }
    }

    /// Metric weight of an offset within the bar, in `0.0..=1.0`.
    ///
    /// The model is deliberately small and testable. `offset_in_bar` is reduced
    /// modulo the bar length first, then:
    ///
    /// | position | simple meter | compound meter |
    /// |---|---|---|
    /// | downbeat | 1.00 | 1.00 |
    /// | midpoint beat of an even bar | 0.80 | — |
    /// | start of a three-beat group | — | 0.85 |
    /// | other notated beat | 0.60 | 0.60 |
    /// | half-beat subdivision | 0.40 | 0.40 |
    /// | triplet subdivision | 0.30 | 0.30 |
    /// | quarter-beat subdivision | 0.25 | 0.25 |
    /// | anything finer | 0.15 | 0.15 |
    pub fn metric_weight(self, offset_in_bar: BeatTime) -> f64 {
        let bar = self.bar_length_qn();
        if bar.is_zero() {
            return 0.0;
        }
        let off = offset_in_bar.rem_euclid(bar);
        if off.is_zero() {
            return 1.0;
        }
        let beat = self.beat_unit_qn();
        if self.is_compound() {
            let group = beat * 3;
            if off.divides_evenly_by(group) {
                return 0.85;
            }
        }
        if off.divides_evenly_by(beat) {
            if !self.is_compound() && self.numerator % 2 == 0 {
                let index = off.div_floor(beat);
                if index == (self.numerator / 2) as i64 {
                    return 0.8;
                }
            }
            return 0.6;
        }
        if off.divides_evenly_by(beat.scale(1, 2)) {
            return 0.4;
        }
        if off.divides_evenly_by(beat.scale(1, 3)) {
            return 0.3;
        }
        if off.divides_evenly_by(beat.scale(1, 4)) {
            return 0.25;
        }
        0.15
    }

    /// Parses `"4/4"`, `"6/8"`, `"7/8"`.
    pub fn parse(s: &str) -> Option<TimeSignature> {
        let (n, d) = s.trim().split_once('/')?;
        let n: u16 = n.trim().parse().ok()?;
        let d: u16 = d.trim().parse().ok()?;
        if n == 0 || d == 0 {
            return None;
        }
        Some(TimeSignature::new(n, d))
    }

    /// Canonical text, e.g. `"4/4"`.
    pub fn to_display(self) -> String {
        format!("{}/{}", self.numerator, self.denominator)
    }
}

impl fmt::Display for TimeSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_display())
    }
}

/// A tempo change at a musical position.
#[derive(Clone, Debug, PartialEq)]
pub struct TempoEvent {
    /// Position in quarter notes.
    pub qn: BeatTime,
    /// Tempo in beats per minute at this position.
    pub bpm: f64,
    /// True when the tempo ramps linearly to the next event.
    pub linear: bool,
}

impl TempoEvent {
    /// Builds a tempo event.
    pub fn new(qn: BeatTime, bpm: f64, linear: bool) -> TempoEvent {
        TempoEvent { qn, bpm, linear }
    }

    /// JSON form: `{"qn": "0", "bpm": 120.0, "linear": false}`.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "qn" => self.qn.to_json(),
            "bpm" => self.bpm,
            "linear" => self.linear,
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<TempoEvent, DomainError> {
        Ok(TempoEvent {
            qn: BeatTime::from_json(v.field("qn")?)?,
            bpm: v.f64_field("bpm")?,
            linear: v.opt_bool_field("linear")?.unwrap_or(false),
        })
    }
}

/// A meter change at a musical position.
#[derive(Clone, Debug, PartialEq)]
pub struct MeterEvent {
    /// Position in quarter notes.
    pub qn: BeatTime,
    /// Time signature taking effect here.
    pub sig: TimeSignature,
    /// Zero-based bar index at this position.
    pub measure: i64,
}

impl MeterEvent {
    /// Builds a meter event.
    pub fn new(qn: BeatTime, sig: TimeSignature, measure: i64) -> MeterEvent {
        MeterEvent { qn, sig, measure }
    }

    /// JSON form: `{"qn": "0", "sig": "4/4", "measure": 0}`.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "qn" => self.qn.to_json(),
            "sig" => self.sig.to_display(),
            "measure" => self.measure,
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<MeterEvent, DomainError> {
        let sig_text = v.str_field("sig")?;
        let sig = TimeSignature::parse(sig_text)
            .ok_or_else(|| DomainError::invalid_time(format!("bad time signature {sig_text:?}")))?;
        Ok(MeterEvent {
            qn: BeatTime::from_json(v.field("qn")?)?,
            sig,
            measure: v.opt_i64_field("measure")?.unwrap_or(0),
        })
    }
}

/// The project's tempo and meter map.
///
/// Both vectors are sorted by position, de-duplicated (the first event at a
/// position wins) and guaranteed non-empty: a map built from nothing carries a
/// 120 bpm 4/4 event at position zero. Bar indices are recomputed from the map
/// on construction, so a meter change always starts a new bar even when it
/// lands mid-bar.
#[derive(Clone, Debug, PartialEq)]
pub struct TimeMap {
    /// Tempo events, ascending by position.
    pub tempos: Vec<TempoEvent>,
    /// Meter events, ascending by position.
    pub meters: Vec<MeterEvent>,
}

impl Default for TimeMap {
    /// 120 bpm in 4/4.
    fn default() -> Self {
        TimeMap::constant(120.0, TimeSignature::default())
    }
}

impl TimeMap {
    /// A map with a single tempo and a single meter.
    pub fn constant(bpm: f64, sig: TimeSignature) -> TimeMap {
        TimeMap::new(
            vec![TempoEvent::new(BeatTime::ZERO, bpm, false)],
            vec![MeterEvent::new(BeatTime::ZERO, sig, 0)],
        )
    }

    /// Builds a map, sorting, de-duplicating and normalizing bar indices.
    pub fn new(tempos: Vec<TempoEvent>, meters: Vec<MeterEvent>) -> TimeMap {
        let mut tempos = tempos;
        let mut meters = meters;
        tempos.sort_by(|a, b| a.qn.cmp(&b.qn));
        tempos.dedup_by(|a, b| a.qn == b.qn);
        meters.sort_by(|a, b| a.qn.cmp(&b.qn));
        meters.dedup_by(|a, b| a.qn == b.qn);

        if tempos.is_empty() {
            tempos.push(TempoEvent::new(BeatTime::ZERO, 120.0, false));
        } else if tempos[0].qn > BeatTime::ZERO {
            let first = tempos[0].bpm;
            tempos.insert(0, TempoEvent::new(BeatTime::ZERO, first, false));
        }
        if meters.is_empty() {
            meters.push(MeterEvent::new(
                BeatTime::ZERO,
                TimeSignature::default(),
                0,
            ));
        } else if meters[0].qn > BeatTime::ZERO {
            let first = meters[0].sig;
            meters.insert(0, MeterEvent::new(BeatTime::ZERO, first, 0));
        }
        for t in &mut tempos {
            if !(t.bpm.is_finite() && t.bpm > 0.0) {
                t.bpm = 120.0;
            }
        }
        // Bar indices: the first event supplies the origin, every later event is
        // recomputed so the sequence is always consistent with the positions.
        for i in 1..meters.len() {
            let prev = &meters[i - 1];
            let span = meters[i].qn - prev.qn;
            let bar_len = prev.sig.bar_length_qn();
            let whole = span.div_floor(bar_len);
            let partial = !span.rem_euclid(bar_len).is_zero();
            let bars = whole + i64::from(partial);
            meters[i].measure = prev.measure + bars;
        }
        TimeMap { tempos, meters }
    }

    /// Index of the last meter event at or before `qn` (0 when `qn` precedes all).
    fn meter_index(&self, qn: BeatTime) -> usize {
        let mut idx = 0;
        for (i, m) in self.meters.iter().enumerate() {
            if m.qn <= qn {
                idx = i;
            } else {
                break;
            }
        }
        idx
    }

    /// The time signature in effect at `qn`.
    pub fn meter_at(&self, qn: BeatTime) -> TimeSignature {
        self.meters[self.meter_index(qn)].sig
    }

    /// The tempo in effect at `qn`, in bpm.
    pub fn tempo_at(&self, qn: BeatTime) -> f64 {
        let mut bpm = self.tempos[0].bpm;
        for t in &self.tempos {
            if t.qn <= qn {
                bpm = t.bpm;
            } else {
                break;
            }
        }
        bpm
    }

    /// Zero-based bar index containing `qn`. Negative positions yield negative
    /// bar indices.
    pub fn bar_of(&self, qn: BeatTime) -> i64 {
        let i = self.meter_index(qn);
        let m = &self.meters[i];
        let bars = (qn - m.qn).div_floor(m.sig.bar_length_qn());
        m.measure + bars
    }

    /// Position of the start of `bar`.
    pub fn bar_start(&self, bar: i64) -> BeatTime {
        let mut chosen = 0usize;
        for (i, m) in self.meters.iter().enumerate() {
            if m.measure <= bar {
                chosen = i;
            } else {
                break;
            }
        }
        let m = &self.meters[chosen];
        m.qn + m.sig.bar_length_qn() * (bar - m.measure)
    }

    /// Offset of `qn` from the start of its bar.
    pub fn position_in_bar(&self, qn: BeatTime) -> BeatTime {
        qn - self.bar_start(self.bar_of(qn))
    }

    /// Metric weight of `qn` under the meter in effect there.
    pub fn metric_weight(&self, qn: BeatTime) -> f64 {
        self.meter_at(qn).metric_weight(self.position_in_bar(qn))
    }

    /// Elapsed seconds from position zero to `qn`, integrating tempo ramps.
    pub fn qn_to_seconds(&self, qn: BeatTime) -> f64 {
        if qn.is_zero() {
            return 0.0;
        }
        if qn.is_negative() {
            return -(qn.abs().as_f64()) * 60.0 / self.tempos[0].bpm;
        }
        let mut secs = 0.0;
        for i in 0..self.tempos.len() {
            let seg_start = self.tempos[i].qn;
            if seg_start >= qn {
                break;
            }
            let next = self.tempos.get(i + 1).map(|t| t.qn);
            let seg_end = match next {
                Some(e) => e.min(qn),
                None => qn,
            };
            let len = (seg_end - seg_start).as_f64();
            if len <= 0.0 {
                continue;
            }
            let b0 = self.tempos[i].bpm;
            let ramp = self.tempos[i].linear && next.is_some();
            if ramp {
                let full = (next.unwrap_or(seg_end) - seg_start).as_f64();
                let b1 = self.tempos[i + 1].bpm;
                let b_at = if full > 0.0 {
                    b0 + (b1 - b0) * (len / full)
                } else {
                    b0
                };
                if (b_at - b0).abs() < 1e-12 {
                    secs += len * 60.0 / b0;
                } else {
                    secs += 60.0 * len * (b_at / b0).ln() / (b_at - b0);
                }
            } else {
                secs += len * 60.0 / b0;
            }
        }
        secs
    }

    /// JSON form of the whole map.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "tempos" => Json::Arr(self.tempos.iter().map(|t| t.to_json()).collect()),
            "meters" => Json::Arr(self.meters.iter().map(|m| m.to_json()).collect()),
        }
    }

    /// Reads the JSON form.
    pub fn from_json(v: &Json) -> Result<TimeMap, DomainError> {
        let mut tempos = Vec::new();
        for t in v.arr_field("tempos")? {
            tempos.push(TempoEvent::from_json(t)?);
        }
        let mut meters = Vec::new();
        for m in v.arr_field("meters")? {
            meters.push(MeterEvent::from_json(m)?);
        }
        Ok(TimeMap::new(tempos, meters))
    }

    /// SHA-256 of the canonical JSON form; used as a change detector.
    pub fn hash_hex(&self) -> String {
        qjson::sha256::sha256_hex(self.to_json().to_canonical_string().as_bytes())
    }

    /// Bar start positions `p` with `start <= p < end`.
    pub fn bar_grid(&self, start: BeatTime, end: BeatTime) -> Vec<BeatTime> {
        let mut out = Vec::new();
        if end <= start {
            return out;
        }
        let mut bar = self.bar_of(start);
        // Guard against a pathological map producing zero-length bars.
        for _ in 0..MAX_GRID_STEPS {
            let p = self.bar_start(bar);
            if p >= end {
                break;
            }
            if p >= start {
                out.push(p);
            }
            let next = self.bar_start(bar + 1);
            if next <= p {
                break;
            }
            bar += 1;
        }
        out
    }

    /// Grid positions spaced `beats` apart, re-anchored at every bar line so a
    /// meter change never leaves the grid drifting.
    ///
    /// Returns positions `p` with `start <= p < end`. A non-positive `beats`
    /// yields an empty vector.
    pub fn beat_grid(&self, start: BeatTime, end: BeatTime, beats: BeatTime) -> Vec<BeatTime> {
        let mut out = Vec::new();
        if end <= start || !beats.is_positive() {
            return out;
        }
        let mut bar = self.bar_of(start);
        let mut steps = 0usize;
        loop {
            let bar_start = self.bar_start(bar);
            if bar_start >= end {
                break;
            }
            let next_bar = self.bar_start(bar + 1);
            if next_bar <= bar_start {
                break;
            }
            let limit = next_bar.min(end);
            let mut p = bar_start;
            while p < limit {
                if p >= start {
                    out.push(p);
                }
                p = p + beats;
                steps += 1;
                if steps >= MAX_GRID_STEPS {
                    return out;
                }
            }
            bar += 1;
            steps += 1;
            if steps >= MAX_GRID_STEPS {
                break;
            }
        }
        out
    }
}

/// Upper bound on grid iterations, so a malformed map cannot hang a caller.
const MAX_GRID_STEPS: usize = 1_000_000;

#[cfg(test)]
mod tests {
    use super::*;

    fn bt(n: i64, d: i64) -> BeatTime {
        BeatTime::new(n, d)
    }

    #[test]
    fn normalization_reduces_and_fixes_sign() {
        assert_eq!(bt(2, 4), bt(1, 2));
        assert_eq!(bt(-1, -2), bt(1, 2));
        assert_eq!(bt(1, -2), bt(-1, 2));
        assert_eq!(bt(0, 5), BeatTime::ZERO);
        assert_eq!(bt(6, 3).num(), 2);
        assert_eq!(bt(6, 3).den(), 1);
        assert!(bt(-3, 4).den() > 0);
    }

    #[test]
    fn equal_values_hash_equally() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let h = |v: BeatTime| {
            let mut s = DefaultHasher::new();
            v.hash(&mut s);
            s.finish()
        };
        assert_eq!(h(bt(2, 4)), h(bt(1, 2)));
        assert_eq!(h(bt(-2, 4)), h(bt(1, -2)));
        assert_eq!(h(bt(4, 2)), h(BeatTime::from_quarters(2)));
        let mut set = std::collections::HashSet::new();
        set.insert(bt(2, 4));
        assert!(set.contains(&bt(1, 2)));
    }

    #[test]
    fn try_new_rejects_zero_denominator() {
        assert!(BeatTime::try_new(1, 0).is_none());
        assert!(BeatTime::try_new(1, 3).is_some());
    }

    #[test]
    fn arithmetic_is_exact() {
        assert_eq!(bt(1, 3) + bt(1, 6), bt(1, 2));
        assert_eq!(bt(1, 2) - bt(1, 3), bt(1, 6));
        assert_eq!(-bt(3, 4), bt(-3, 4));
        assert_eq!(bt(1, 3) * 3, BeatTime::ONE);
        assert_eq!(bt(1, 3) + bt(1, 3) + bt(1, 3), BeatTime::ONE);
        assert_eq!(bt(7, 3).scale(3, 7), BeatTime::ONE);
        assert_eq!(bt(2, 3).abs(), bt(2, 3));
        assert_eq!(bt(-2, 3).abs(), bt(2, 3));
    }

    #[test]
    fn triplets_sum_exactly_unlike_floats() {
        let third = bt(1, 3);
        let sum: BeatTime = std::iter::repeat_n(third, 3).sum();
        assert_eq!(sum, BeatTime::ONE);
        assert!(sum.is_positive());
    }

    #[test]
    fn ordering_is_exact() {
        assert!(bt(1, 3) < bt(1, 2));
        assert!(bt(-1, 3) < BeatTime::ZERO);
        assert_eq!(bt(1, 2).max(bt(1, 3)), bt(1, 2));
        assert_eq!(bt(1, 2).min(bt(1, 3)), bt(1, 3));
        let mut v = vec![bt(3, 2), bt(1, 4), bt(-1, 2)];
        v.sort();
        assert_eq!(v, vec![bt(-1, 2), bt(1, 4), bt(3, 2)]);
    }

    #[test]
    fn rem_euclid_and_div_floor() {
        assert_eq!(bt(7, 2).rem_euclid(BeatTime::ONE), bt(1, 2));
        assert_eq!(bt(-1, 2).rem_euclid(BeatTime::ONE), bt(1, 2));
        assert_eq!(bt(4, 1).rem_euclid(bt(4, 1)), BeatTime::ZERO);
        assert_eq!(bt(7, 2).div_floor(BeatTime::ONE), 3);
        assert_eq!(bt(-1, 2).div_floor(BeatTime::ONE), -1);
        assert_eq!(bt(9, 1).div_floor(bt(3, 1)), 3);
        assert_eq!(BeatTime::ONE.div_floor(BeatTime::ZERO), 0);
        assert_eq!(BeatTime::ONE.rem_euclid(BeatTime::ZERO), BeatTime::ONE);
    }

    #[test]
    fn divides_evenly() {
        assert!(bt(2, 1).divides_evenly_by(bt(1, 2)));
        assert!(!bt(1, 3).divides_evenly_by(bt(1, 2)));
        assert!(BeatTime::ZERO.divides_evenly_by(BeatTime::ZERO));
    }

    #[test]
    fn from_f64_snaps_to_the_grid() {
        assert_eq!(BeatTime::from_f64(0.5), bt(1, 2));
        assert_eq!(BeatTime::from_f64(1.0 / 3.0), bt(1, 3));
        assert_eq!(BeatTime::from_f64(0.3333333), bt(1, 3));
        assert_eq!(BeatTime::from_f64(0.25), bt(1, 4));
        assert_eq!(BeatTime::from_f64(0.2), bt(1, 5));
        assert_eq!(BeatTime::from_f64(-1.5), bt(-3, 2));
        assert_eq!(BeatTime::from_f64(f64::NAN), BeatTime::ZERO);
        assert_eq!(BeatTime::from_f64_grid(0.51, 2), bt(1, 2));
        assert_eq!(BeatTime::from_f64_grid(0.5, 0), BeatTime::ONE);
    }

    #[test]
    fn as_f64_round_trips_through_the_grid() {
        for (n, d) in [(1, 2), (1, 3), (7, 3), (-5, 4), (0, 1), (12, 1)] {
            let v = bt(n, d);
            assert_eq!(BeatTime::from_f64(v.as_f64()), v);
        }
    }

    #[test]
    fn overflow_saturates_instead_of_wrapping() {
        let huge = BeatTime::MAX;
        assert_eq!(huge + huge, BeatTime::MAX);
        assert_eq!(BeatTime::MIN + BeatTime::MIN, BeatTime::MIN);
        assert_eq!(huge * 1000, BeatTime::MAX);
        assert!((BeatTime::MAX - BeatTime::MIN) <= BeatTime::MAX);
        // A denominator explosion falls back to the grid rather than panicking.
        let odd = bt(1, 1_000_000_007) + bt(1, 999_999_937);
        assert!(odd.den() > 0);
        assert!(odd.as_f64() > 0.0);
    }

    #[test]
    fn parse_rational_strings() {
        assert_eq!(BeatTime::parse("0").unwrap(), BeatTime::ZERO);
        assert_eq!(BeatTime::parse("3").unwrap(), BeatTime::from_quarters(3));
        assert_eq!(BeatTime::parse("1/2").unwrap(), bt(1, 2));
        assert_eq!(BeatTime::parse("-1/4").unwrap(), bt(-1, 4));
        assert_eq!(BeatTime::parse("7/3").unwrap(), bt(7, 3));
        assert_eq!(BeatTime::parse(" 2 / 4 ").unwrap(), bt(1, 2));
        assert_eq!(BeatTime::parse("1.5").unwrap(), bt(3, 2));
        assert_eq!("7/3".parse::<BeatTime>().unwrap(), bt(7, 3));
    }

    #[test]
    fn parse_rejects_garbage() {
        for bad in ["", "  ", "1/0", "a/b", "1/", "/2", "x"] {
            assert!(BeatTime::parse(bad).is_err(), "{bad:?} should fail");
        }
        assert_eq!(BeatTime::parse("1/0").unwrap_err().code, "INVALID_TIME");
    }

    #[test]
    fn display_forms() {
        assert_eq!(bt(3, 1).to_display(), "3");
        assert_eq!(bt(1, 2).to_display(), "1/2");
        assert_eq!(bt(-1, 4).to_display(), "-1/4");
        assert_eq!(bt(7, 3).to_string(), "7/3");
        assert_eq!(BeatTime::ZERO.to_display(), "0");
    }

    #[test]
    fn json_round_trip() {
        for v in [bt(0, 1), bt(7, 3), bt(-1, 4), bt(12, 1)] {
            assert_eq!(BeatTime::from_json(&v.to_json()).unwrap(), v);
        }
        assert_eq!(BeatTime::from_json(&Json::Int(4)).unwrap(), bt(4, 1));
        assert_eq!(BeatTime::from_json(&Json::Float(0.5)).unwrap(), bt(1, 2));
        assert!(BeatTime::from_json(&Json::Bool(true)).is_err());
    }

    #[test]
    fn time_signature_lengths() {
        let four = TimeSignature::new(4, 4);
        assert_eq!(four.bar_length_qn(), BeatTime::from_quarters(4));
        assert_eq!(four.beat_unit_qn(), BeatTime::ONE);
        let six_eight = TimeSignature::new(6, 8);
        assert_eq!(six_eight.bar_length_qn(), BeatTime::from_quarters(3));
        assert_eq!(six_eight.beat_unit_qn(), bt(1, 2));
        assert_eq!(TimeSignature::new(7, 8).bar_length_qn(), bt(7, 2));
        assert_eq!(TimeSignature::new(3, 4).bar_length_qn(), BeatTime::from_quarters(3));
    }

    #[test]
    fn compound_detection() {
        assert!(TimeSignature::new(6, 8).is_compound());
        assert!(TimeSignature::new(9, 8).is_compound());
        assert!(TimeSignature::new(12, 8).is_compound());
        assert!(!TimeSignature::new(3, 4).is_compound());
        assert!(!TimeSignature::new(4, 4).is_compound());
        assert!(!TimeSignature::new(3, 8).is_compound());
        assert_eq!(TimeSignature::new(6, 8).pulse_qn(), bt(3, 2));
        assert_eq!(TimeSignature::new(4, 4).pulse_qn(), BeatTime::ONE);
    }

    #[test]
    fn time_signature_parse_and_display() {
        assert_eq!(TimeSignature::parse("4/4"), Some(TimeSignature::new(4, 4)));
        assert_eq!(TimeSignature::parse(" 6/8 "), Some(TimeSignature::new(6, 8)));
        assert_eq!(TimeSignature::parse("4"), None);
        assert_eq!(TimeSignature::parse("0/4"), None);
        assert_eq!(TimeSignature::parse("4/0"), None);
        assert_eq!(TimeSignature::new(4, 4).to_display(), "4/4");
        assert_eq!(TimeSignature::new(0, 0), TimeSignature::new(1, 4));
    }

    #[test]
    fn metric_weight_simple_meter() {
        let s = TimeSignature::new(4, 4);
        assert_eq!(s.metric_weight(BeatTime::ZERO), 1.0);
        assert_eq!(s.metric_weight(BeatTime::from_quarters(2)), 0.8);
        assert_eq!(s.metric_weight(BeatTime::from_quarters(1)), 0.6);
        assert_eq!(s.metric_weight(BeatTime::from_quarters(3)), 0.6);
        assert_eq!(s.metric_weight(bt(1, 2)), 0.4);
        assert_eq!(s.metric_weight(bt(1, 3)), 0.3);
        assert_eq!(s.metric_weight(bt(1, 4)), 0.25);
        assert_eq!(s.metric_weight(bt(1, 8)), 0.15);
        // Wraps modulo the bar.
        assert_eq!(s.metric_weight(BeatTime::from_quarters(4)), 1.0);
    }

    #[test]
    fn metric_weight_compound_meter() {
        let s = TimeSignature::new(6, 8);
        assert_eq!(s.metric_weight(BeatTime::ZERO), 1.0);
        assert_eq!(s.metric_weight(bt(3, 2)), 0.85);
        assert_eq!(s.metric_weight(bt(1, 2)), 0.6);
        assert_eq!(s.metric_weight(BeatTime::ONE), 0.6);
        assert_eq!(s.metric_weight(bt(1, 4)), 0.4);
        let waltz = TimeSignature::new(3, 4);
        assert_eq!(waltz.metric_weight(BeatTime::ZERO), 1.0);
        assert_eq!(waltz.metric_weight(BeatTime::ONE), 0.6);
        assert_eq!(waltz.metric_weight(BeatTime::from_quarters(2)), 0.6);
    }

    #[test]
    fn metric_weight_is_bounded() {
        for sig in [
            TimeSignature::new(4, 4),
            TimeSignature::new(3, 4),
            TimeSignature::new(6, 8),
            TimeSignature::new(7, 8),
            TimeSignature::new(5, 4),
        ] {
            for k in 0..64 {
                let w = sig.metric_weight(bt(k, 8));
                assert!((0.0..=1.0).contains(&w), "{sig} at {k}/8 => {w}");
            }
        }
    }

    #[test]
    fn constant_map_defaults() {
        let tm = TimeMap::default();
        assert_eq!(tm.tempos.len(), 1);
        assert_eq!(tm.meters.len(), 1);
        assert_eq!(tm.tempo_at(BeatTime::from_quarters(100)), 120.0);
        assert_eq!(tm.meter_at(BeatTime::ZERO), TimeSignature::new(4, 4));
        let empty = TimeMap::new(vec![], vec![]);
        assert_eq!(empty.tempos.len(), 1);
        assert_eq!(empty.meters.len(), 1);
    }

    #[test]
    fn map_sorts_and_dedups() {
        let tm = TimeMap::new(
            vec![
                TempoEvent::new(BeatTime::from_quarters(8), 90.0, false),
                TempoEvent::new(BeatTime::ZERO, 120.0, false),
                TempoEvent::new(BeatTime::ZERO, 60.0, false),
            ],
            vec![MeterEvent::new(BeatTime::ZERO, TimeSignature::new(4, 4), 0)],
        );
        assert_eq!(tm.tempos.len(), 2);
        assert_eq!(tm.tempos[0].qn, BeatTime::ZERO);
        assert_eq!(tm.tempo_at(BeatTime::from_quarters(9)), 90.0);
        assert_eq!(tm.tempo_at(BeatTime::from_quarters(7)), 120.0);
    }

    #[test]
    fn bar_indices_in_constant_meter() {
        let tm = TimeMap::constant(120.0, TimeSignature::new(4, 4));
        assert_eq!(tm.bar_of(BeatTime::ZERO), 0);
        assert_eq!(tm.bar_of(BeatTime::from_quarters(3)), 0);
        assert_eq!(tm.bar_of(BeatTime::from_quarters(4)), 1);
        assert_eq!(tm.bar_of(BeatTime::from_quarters(9)), 2);
        assert_eq!(tm.bar_of(bt(-1, 2)), -1);
        assert_eq!(tm.bar_start(2), BeatTime::from_quarters(8));
        assert_eq!(tm.position_in_bar(BeatTime::from_quarters(9)), BeatTime::ONE);
    }

    #[test]
    fn bar_indices_across_meter_changes() {
        // 2 bars of 4/4, then 3/4 from qn 8, then 6/8 from qn 17.
        let tm = TimeMap::new(
            vec![TempoEvent::new(BeatTime::ZERO, 120.0, false)],
            vec![
                MeterEvent::new(BeatTime::ZERO, TimeSignature::new(4, 4), 0),
                MeterEvent::new(BeatTime::from_quarters(8), TimeSignature::new(3, 4), 0),
                MeterEvent::new(BeatTime::from_quarters(17), TimeSignature::new(6, 8), 0),
            ],
        );
        assert_eq!(tm.meters[1].measure, 2);
        assert_eq!(tm.meters[2].measure, 5);
        assert_eq!(tm.bar_of(BeatTime::from_quarters(7)), 1);
        assert_eq!(tm.bar_of(BeatTime::from_quarters(8)), 2);
        assert_eq!(tm.bar_of(BeatTime::from_quarters(11)), 3);
        assert_eq!(tm.bar_of(BeatTime::from_quarters(16)), 4);
        assert_eq!(tm.bar_of(BeatTime::from_quarters(17)), 5);
        assert_eq!(tm.bar_of(BeatTime::from_quarters(20)), 6);
        assert_eq!(tm.bar_start(2), BeatTime::from_quarters(8));
        assert_eq!(tm.bar_start(5), BeatTime::from_quarters(17));
        assert_eq!(tm.bar_start(6), BeatTime::from_quarters(20));
        assert_eq!(tm.meter_at(BeatTime::from_quarters(18)), TimeSignature::new(6, 8));
        assert_eq!(tm.meter_at(BeatTime::from_quarters(9)), TimeSignature::new(3, 4));
    }

    #[test]
    fn meter_change_off_a_bar_line_starts_a_new_bar() {
        let tm = TimeMap::new(
            vec![],
            vec![
                MeterEvent::new(BeatTime::ZERO, TimeSignature::new(4, 4), 0),
                MeterEvent::new(BeatTime::from_quarters(6), TimeSignature::new(3, 4), 0),
            ],
        );
        // qn 4..6 is a partial bar; it still counts as bar 1, so 3/4 starts at bar 2.
        assert_eq!(tm.meters[1].measure, 2);
        assert_eq!(tm.bar_of(BeatTime::from_quarters(5)), 1);
        assert_eq!(tm.bar_of(BeatTime::from_quarters(6)), 2);
    }

    #[test]
    fn bar_start_before_the_first_meter() {
        let tm = TimeMap::constant(120.0, TimeSignature::new(4, 4));
        assert_eq!(tm.bar_start(-1), BeatTime::from_quarters(-4));
        assert_eq!(tm.bar_of(BeatTime::from_quarters(-4)), -1);
    }

    #[test]
    fn metric_weight_through_the_map() {
        let tm = TimeMap::new(
            vec![],
            vec![
                MeterEvent::new(BeatTime::ZERO, TimeSignature::new(4, 4), 0),
                MeterEvent::new(BeatTime::from_quarters(8), TimeSignature::new(3, 4), 0),
            ],
        );
        assert_eq!(tm.metric_weight(BeatTime::from_quarters(8)), 1.0);
        assert_eq!(tm.metric_weight(BeatTime::from_quarters(9)), 0.6);
        assert_eq!(tm.metric_weight(BeatTime::from_quarters(4)), 1.0);
        assert_eq!(tm.metric_weight(BeatTime::from_quarters(6)), 0.8);
    }

    #[test]
    fn seconds_at_constant_tempo() {
        let tm = TimeMap::constant(120.0, TimeSignature::new(4, 4));
        assert!((tm.qn_to_seconds(BeatTime::from_quarters(4)) - 2.0).abs() < 1e-9);
        assert!((tm.qn_to_seconds(BeatTime::ZERO)).abs() < 1e-12);
        assert!(tm.qn_to_seconds(BeatTime::from_quarters(-4)) < 0.0);
        let slow = TimeMap::constant(60.0, TimeSignature::new(4, 4));
        assert!((slow.qn_to_seconds(BeatTime::from_quarters(4)) - 4.0).abs() < 1e-9);
    }

    #[test]
    fn seconds_across_tempo_changes() {
        let tm = TimeMap::new(
            vec![
                TempoEvent::new(BeatTime::ZERO, 120.0, false),
                TempoEvent::new(BeatTime::from_quarters(4), 60.0, false),
            ],
            vec![],
        );
        // 4 QN at 120 = 2s, then 4 QN at 60 = 4s.
        assert!((tm.qn_to_seconds(BeatTime::from_quarters(8)) - 6.0).abs() < 1e-9);
        assert!((tm.qn_to_seconds(BeatTime::from_quarters(4)) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn seconds_across_a_linear_ramp() {
        let tm = TimeMap::new(
            vec![
                TempoEvent::new(BeatTime::ZERO, 60.0, true),
                TempoEvent::new(BeatTime::from_quarters(4), 120.0, false),
            ],
            vec![],
        );
        let ramped = tm.qn_to_seconds(BeatTime::from_quarters(4));
        // Between the constant-60 (4s) and constant-120 (2s) bounds.
        assert!(ramped > 2.0 && ramped < 4.0, "{ramped}");
        // The exact integral of 60/bpm over a linear ramp.
        let expected = 60.0 * 4.0 * (120.0f64 / 60.0).ln() / 60.0;
        assert!((ramped - expected).abs() < 1e-9);
    }

    #[test]
    fn grids() {
        let tm = TimeMap::constant(120.0, TimeSignature::new(4, 4));
        let bars = tm.bar_grid(BeatTime::ZERO, BeatTime::from_quarters(12));
        assert_eq!(
            bars,
            vec![
                BeatTime::ZERO,
                BeatTime::from_quarters(4),
                BeatTime::from_quarters(8)
            ]
        );
        let beats = tm.beat_grid(BeatTime::ZERO, BeatTime::from_quarters(4), BeatTime::ONE);
        assert_eq!(beats.len(), 4);
        assert_eq!(beats[3], BeatTime::from_quarters(3));
        assert!(tm
            .bar_grid(BeatTime::from_quarters(4), BeatTime::from_quarters(4))
            .is_empty());
        assert!(tm
            .beat_grid(BeatTime::ZERO, BeatTime::from_quarters(4), BeatTime::ZERO)
            .is_empty());
    }

    #[test]
    fn beat_grid_reanchors_at_meter_changes() {
        let tm = TimeMap::new(
            vec![],
            vec![
                MeterEvent::new(BeatTime::ZERO, TimeSignature::new(4, 4), 0),
                MeterEvent::new(BeatTime::from_quarters(6), TimeSignature::new(3, 4), 0),
            ],
        );
        let g = tm.beat_grid(BeatTime::ZERO, BeatTime::from_quarters(9), BeatTime::ONE);
        assert!(g.contains(&BeatTime::from_quarters(6)));
        assert!(g.contains(&BeatTime::from_quarters(5)));
        assert!(g.iter().all(|p| *p < BeatTime::from_quarters(9)));
    }

    #[test]
    fn map_json_round_trip_and_hash() {
        let tm = TimeMap::new(
            vec![
                TempoEvent::new(BeatTime::ZERO, 120.0, false),
                TempoEvent::new(BeatTime::from_quarters(8), 90.0, true),
            ],
            vec![
                MeterEvent::new(BeatTime::ZERO, TimeSignature::new(4, 4), 0),
                MeterEvent::new(BeatTime::from_quarters(8), TimeSignature::new(3, 4), 0),
            ],
        );
        let back = TimeMap::from_json(&tm.to_json()).expect("round trip");
        assert_eq!(back, tm);
        assert_eq!(back.hash_hex(), tm.hash_hex());
        assert_eq!(tm.hash_hex().len(), 64);
        assert_ne!(
            tm.hash_hex(),
            TimeMap::constant(120.0, TimeSignature::new(4, 4)).hash_hex()
        );
    }

    #[test]
    fn invalid_tempo_values_are_repaired() {
        let tm = TimeMap::new(
            vec![TempoEvent::new(BeatTime::ZERO, 0.0, false)],
            vec![],
        );
        assert_eq!(tm.tempos[0].bpm, 120.0);
        let nan = TimeMap::new(
            vec![TempoEvent::new(BeatTime::ZERO, f64::NAN, false)],
            vec![],
        );
        assert_eq!(nan.tempos[0].bpm, 120.0);
    }

    #[test]
    fn ppq_constant_is_documented_provenance() {
        assert_eq!(PPQ, 960);
        assert_eq!(GRID_DEN, 1920);
    }
}
