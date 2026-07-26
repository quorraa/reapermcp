//! Performance and boundedness.
//!
//! The product requirement is that sixteen bars produce three candidates
//! comfortably inside a second in release mode. The bound asserted here is
//! deliberately loose in a debug build — an unoptimised build is roughly an
//! order of magnitude slower and asserting a release number there would only
//! produce a flaky test — but the release bound is the real one.

use harmony_engine::candidates::build_pools;
use harmony_engine::generate::generate_candidates;
use harmony_engine::params::{CancelFlag, GenerateParams, SearchConfig};
use harmony_engine::search::search_in;
use harmony_engine::testing;
use std::time::{Duration, Instant};

/// The sixteen-bar reference fixture.
const SIXTEEN_BARS: &str = "progressions/sixteen_bar_c_major";

/// The budget for three candidates over sixteen bars.
fn budget() -> Duration {
    if cfg!(debug_assertions) {
        Duration::from_secs(20)
    } else {
        Duration::from_millis(1000)
    }
}

#[test]
fn sixteen_bars_three_candidates_within_budget() {
    let p = GenerateParams::default()
        .with_profile("jazz_standard")
        .with_candidate_count(3);
    let h = testing::harness_with(SIXTEEN_BARS, p.clone());
    let start = Instant::now();
    let candidates = generate_candidates(h.kb, &h.analysis, &p, &CancelFlag::new(), &mut |_, _| {})
        .expect("candidates");
    let elapsed = start.elapsed();
    assert_eq!(candidates.len(), 3);
    assert!(
        elapsed < budget(),
        "sixteen bars took {elapsed:?}, budget {:?} ({} build)",
        budget(),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    println!(
        "16 bars / 3 candidates / jazz_standard: {elapsed:?} ({} build, {} slots)",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        h.analysis.grid.slots.len()
    );
}

#[test]
fn every_profile_stays_within_budget_on_sixteen_bars() {
    for profile in testing::PROFILE_IDS {
        let p = GenerateParams::default().with_profile(profile);
        let h = testing::harness_with(SIXTEEN_BARS, p.clone());
        let start = Instant::now();
        generate_candidates(h.kb, &h.analysis, &p, &CancelFlag::new(), &mut |_, _| {})
            .unwrap_or_else(|e| panic!("{profile}: {e}"));
        let elapsed = start.elapsed();
        assert!(
            elapsed < budget(),
            "{profile} took {elapsed:?}, budget {:?}",
            budget()
        );
        println!("{profile}: {elapsed:?}");
    }
}

#[test]
fn search_is_bounded_by_its_configuration() {
    let h = testing::harness(SIXTEEN_BARS, "jazz_standard");
    let ctx = h.context();
    let pools = build_pools(&ctx).expect("pools");
    for pool in &pools {
        assert!(
            pool.len() <= SearchConfig::default().max_options_per_slot,
            "a pool of {} exceeds the configured cap",
            pool.len()
        );
    }
    let cfg = SearchConfig {
        beam_width: 4,
        max_options_per_slot: 4,
        max_paths: 500,
        diversity_paths: 3,
    };
    let start = Instant::now();
    let found = search_in(&ctx, &pools, &cfg, &CancelFlag::new(), &mut |_, _| {}).expect("paths");
    let elapsed = start.elapsed();
    assert!(found.len() <= cfg.diversity_paths);
    assert!(
        elapsed < budget(),
        "a tightly bounded search took {elapsed:?}"
    );
}

#[test]
fn cancellation_returns_promptly() {
    let h = testing::harness(SIXTEEN_BARS, "jazz_standard");
    let cancel = CancelFlag::new();
    cancel.cancel();
    let start = Instant::now();
    let error = generate_candidates(
        h.kb,
        &h.analysis,
        &GenerateParams::default().with_profile("jazz_standard"),
        &cancel,
        &mut |_, _| {},
    )
    .expect_err("cancelled");
    assert!(error.is_cancelled());
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "cancellation must be observed quickly"
    );
}

#[test]
fn progress_reaches_completion_on_long_material() {
    let h = testing::harness(SIXTEEN_BARS, "cinematic");
    let mut reported: Vec<f64> = Vec::new();
    let mut labels: Vec<String> = Vec::new();
    generate_candidates(
        h.kb,
        &h.analysis,
        &GenerateParams::default().with_profile("cinematic"),
        &CancelFlag::new(),
        &mut |fraction, label| {
            reported.push(fraction);
            labels.push(label.to_string());
        },
    )
    .expect("candidates");
    assert!(reported.len() >= 4, "progress is reported through the run");
    assert_eq!(reported.last().copied(), Some(1.0));
    assert!(labels.iter().all(|l| !l.is_empty()));
}
