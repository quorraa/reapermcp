//! Phrase-level planning.
//!
//! The innermost of the brief's three planning levels: entrances and exits,
//! answers, fills, cadential support, rest placement and repetition variation.
//! Every window here is read off the analysis — phrase boundaries, melodic gaps
//! and cadential grid slots — rather than assumed from bar counts.

use music_analysis::report::Analysis;
use music_domain::prelude::*;

/// The phrase structure an arrangement is written against.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PhraseFrame {
    /// Phrase windows, ordered and clipped to the span.
    pub phrases: Vec<(BeatTime, BeatTime)>,
    /// Cadential arrival points.
    pub cadences: Vec<BeatTime>,
    /// Windows where the lead is silent — the places an answer belongs.
    pub gaps: Vec<(BeatTime, BeatTime)>,
    /// Every onset the lead articulates.
    pub lead_onsets: Vec<BeatTime>,
    /// The whole span.
    pub span: (BeatTime, BeatTime),
}

impl PhraseFrame {
    /// Reads the frame out of an analysis, clipped to a span.
    pub fn from_analysis(an: &Analysis, span: (BeatTime, BeatTime)) -> PhraseFrame {
        let mut phrases: Vec<(BeatTime, BeatTime)> = an
            .phrases
            .phrases
            .iter()
            .map(|p| (p.start.max(span.0), p.end.min(span.1)))
            .filter(|(s, e)| e > s)
            .collect();
        phrases.sort();
        if phrases.is_empty() {
            phrases.push(span);
        }
        let mut cadences: Vec<BeatTime> = an
            .grid
            .slots
            .iter()
            .filter(|s| s.is_cadential)
            .map(|s| s.start)
            .filter(|qn| *qn >= span.0 && *qn < span.1)
            .collect();
        for p in &an.phrases.phrases {
            if p.cadence.is_some() && p.end > span.0 && p.end <= span.1 && !cadences.contains(&p.end)
            {
                cadences.push(p.end);
            }
        }
        cadences.sort();
        cadences.dedup();
        let mut gaps: Vec<(BeatTime, BeatTime)> = an
            .phrases
            .gaps
            .iter()
            .copied()
            .map(|(s, e)| (s.max(span.0), e.min(span.1)))
            .filter(|(s, e)| e > s)
            .collect();
        gaps.sort();
        gaps.dedup();
        let mut lead_onsets: Vec<BeatTime> = an
            .extraction
            .melody
            .notes
            .iter()
            .map(|n| n.onset)
            .filter(|qn| *qn >= span.0 && *qn < span.1)
            .collect();
        lead_onsets.sort();
        lead_onsets.dedup();
        PhraseFrame {
            phrases,
            cadences,
            gaps,
            lead_onsets,
            span,
        }
    }

    /// A frame with nothing but a span, for callers with no analysis to hand.
    pub fn flat(span: (BeatTime, BeatTime)) -> PhraseFrame {
        PhraseFrame {
            phrases: vec![span],
            span,
            ..PhraseFrame::default()
        }
    }

    /// Where a part enters.
    ///
    /// Entrances are staggered by priority: the foreground starts at the top,
    /// midground one phrase in, background two — which is what makes the first
    /// phrases build rather than arrive all at once. A part never enters later
    /// than a third of the way through, because a layer that only appears at
    /// the end is not an arrangement, it is an afterthought; and a form with
    /// fewer than three phrases has no room to stagger at all, so every part
    /// starts together.
    pub fn entrance(&self, stagger: usize) -> BeatTime {
        if stagger == 0 || self.phrases.is_empty() {
            return self.span.0;
        }
        let limit = self.phrases.len() / 3;
        let index = stagger.min(limit);
        self.phrases
            .get(index)
            .map(|(s, _)| *s)
            .unwrap_or(self.span.0)
    }

    /// Where a part leaves, given the loop's planned silence.
    pub fn active_windows(
        &self,
        entrance: BeatTime,
        silence: &[(BeatTime, BeatTime)],
    ) -> Vec<(BeatTime, BeatTime)> {
        let mut windows = vec![(entrance.max(self.span.0), self.span.1)];
        for (s, e) in silence {
            let mut next: Vec<(BeatTime, BeatTime)> = Vec::new();
            for (ws, we) in windows {
                if *e <= ws || *s >= we {
                    next.push((ws, we));
                    continue;
                }
                if ws < *s {
                    next.push((ws, *s));
                }
                if *e < we {
                    next.push((*e, we));
                }
            }
            windows = next;
        }
        windows.retain(|(s, e)| e > s);
        windows
    }

    /// The phrase containing a position.
    pub fn phrase_index(&self, qn: BeatTime) -> Option<usize> {
        self.phrases.iter().position(|(s, e)| qn >= *s && qn < *e)
    }

    /// How much a part varies on a repeat of the same phrase.
    ///
    /// Repetition variation, expressed as a density multiplier: the second time
    /// a phrase of the same length comes round, the accompaniment does slightly
    /// more, and the fourth time slightly more again. Never enough to change
    /// the music's identity, always enough to measure.
    pub fn variation(&self, index: usize) -> f64 {
        match index % 4 {
            0 => 1.0,
            1 => 1.12,
            2 => 0.9,
            _ => 1.2,
        }
    }

    /// Windows in which an answering part should sound: the lead's silences.
    pub fn answer_windows(&self) -> Vec<(BeatTime, BeatTime)> {
        if self.gaps.is_empty() {
            return vec![self.span];
        }
        self.gaps.clone()
    }

    /// The onsets a background part should avoid, being the lead's.
    pub fn contested_onsets(&self) -> &[BeatTime] {
        &self.lead_onsets
    }

    /// Whether a position is a cadential arrival needing support.
    pub fn is_cadential(&self, qn: BeatTime) -> bool {
        self.cadences.contains(&qn)
    }

    /// The window a transition fill occupies before a section change.
    ///
    /// Half a bar before the boundary, which is where a fill has time to be
    /// heard without stepping on the arrival.
    pub fn fill_window(&self, boundary: BeatTime, bar: BeatTime) -> Option<(BeatTime, BeatTime)> {
        let half = bar.scale(1, 2);
        let start = boundary - half;
        if start < self.span.0 || boundary > self.span.1 {
            return None;
        }
        Some((start, boundary))
    }

    /// JSON form.
    pub fn to_json(&self) -> qjson::Json {
        qjson::json_obj! {
            "phrases" => qjson::Json::Arr(self.phrases.iter().map(|(s, e)| qjson::json_obj!{
                "start" => s.to_display(),
                "end" => e.to_display(),
            }).collect()),
            "cadences" => qjson::Json::Arr(
                self.cadences.iter().map(|c| qjson::Json::Str(c.to_display())).collect()
            ),
            "gaps" => qjson::Json::Arr(self.gaps.iter().map(|(s, e)| qjson::json_obj!{
                "start" => s.to_display(),
                "end" => e.to_display(),
            }).collect()),
            "lead_onsets" => self.lead_onsets.len() as i64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span() -> (BeatTime, BeatTime) {
        (BeatTime::ZERO, BeatTime::from_quarters(32))
    }

    fn frame() -> PhraseFrame {
        PhraseFrame {
            phrases: vec![
                (BeatTime::ZERO, BeatTime::from_quarters(8)),
                (BeatTime::from_quarters(8), BeatTime::from_quarters(16)),
                (BeatTime::from_quarters(16), BeatTime::from_quarters(24)),
                (BeatTime::from_quarters(24), BeatTime::from_quarters(32)),
            ],
            cadences: vec![BeatTime::from_quarters(16)],
            gaps: vec![(BeatTime::from_quarters(6), BeatTime::from_quarters(8))],
            lead_onsets: vec![BeatTime::ZERO, BeatTime::from_quarters(4)],
            span: span(),
        }
    }

    #[test]
    fn a_flat_frame_is_one_phrase() {
        let f = PhraseFrame::flat(span());
        assert_eq!(f.phrases, vec![span()]);
        assert_eq!(f.entrance(3), span().0);
        assert_eq!(f.answer_windows(), vec![span()]);
    }

    #[test]
    fn entrances_stagger_but_never_past_a_third() {
        let f = frame();
        assert_eq!(f.entrance(0), BeatTime::ZERO);
        assert_eq!(f.entrance(1), BeatTime::from_quarters(8));
        assert_eq!(f.entrance(9), BeatTime::from_quarters(8));
    }

    #[test]
    fn a_two_phrase_form_has_no_room_to_stagger() {
        let mut f = frame();
        f.phrases.truncate(2);
        assert_eq!(f.entrance(3), BeatTime::ZERO);
    }

    #[test]
    fn silence_splits_the_active_window() {
        let f = frame();
        let windows = f.active_windows(
            BeatTime::ZERO,
            &[(BeatTime::from_quarters(8), BeatTime::from_quarters(12))],
        );
        assert_eq!(
            windows,
            vec![
                (BeatTime::ZERO, BeatTime::from_quarters(8)),
                (BeatTime::from_quarters(12), BeatTime::from_quarters(32)),
            ]
        );
    }

    #[test]
    fn silence_outside_the_window_changes_nothing() {
        let f = frame();
        let windows = f.active_windows(
            BeatTime::from_quarters(16),
            &[(BeatTime::ZERO, BeatTime::from_quarters(4))],
        );
        assert_eq!(
            windows,
            vec![(BeatTime::from_quarters(16), BeatTime::from_quarters(32))]
        );
    }

    #[test]
    fn phrase_lookup_and_variation() {
        let f = frame();
        assert_eq!(f.phrase_index(BeatTime::from_quarters(9)), Some(1));
        assert_eq!(f.phrase_index(BeatTime::from_quarters(64)), None);
        assert_eq!(f.variation(0), 1.0);
        assert!(f.variation(1) > 1.0);
        assert!(f.variation(2) < 1.0);
    }

    #[test]
    fn cadences_and_contested_onsets_are_reported() {
        let f = frame();
        assert!(f.is_cadential(BeatTime::from_quarters(16)));
        assert!(!f.is_cadential(BeatTime::from_quarters(15)));
        assert_eq!(f.contested_onsets().len(), 2);
    }

    #[test]
    fn a_fill_window_sits_before_the_boundary() {
        let f = frame();
        let bar = BeatTime::from_quarters(4);
        assert_eq!(
            f.fill_window(BeatTime::from_quarters(16), bar),
            Some((BeatTime::from_quarters(14), BeatTime::from_quarters(16)))
        );
        assert_eq!(f.fill_window(BeatTime::ZERO, bar), None);
    }

    #[test]
    fn the_frame_serialises() {
        let json = frame().to_json();
        assert!(json.get("phrases").is_some());
        assert_eq!(json.get("lead_onsets").and_then(qjson::Json::as_i64), Some(2));
    }
}
