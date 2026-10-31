//! Opt-in timing for the pad detour: what the game's own poll cadence looks like from inside our
//! hook, and what our code costs inside it.
//!
//! A stutter report is a claim about the game's frame times, which the mod cannot otherwise see.
//! The pad poll runs on the game's input thread once per frame, so the gap between one poll and the
//! next *is* the frame interval: if the game stalls, that number spikes, and if the stall is ours
//! the detour cost spikes with it. One line every ten seconds settles which.
//!
//! Off unless `perf_log = on` in the ini. The counters themselves are a handful of relaxed atomic
//! adds per poll — cheap enough to leave on, and they are what makes the line possible at all.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Performance counter frequency, read once. Zero until `ticks()` has been called.
static FREQ: AtomicU64 = AtomicU64::new(0);
/// Performance counter value at the previous poll.
static LAST: AtomicU64 = AtomicU64::new(0);
/// Start of the current reporting window.
static WINDOW: AtomicU64 = AtomicU64::new(0);
/// Polls, sum of gaps, longest gap, and how many gaps were long enough to see.
static POLLS: AtomicU32 = AtomicU32::new(0);
static GAP_SUM_US: AtomicU64 = AtomicU64::new(0);
static GAP_MAX_US: AtomicU32 = AtomicU32::new(0);
static GAP_OVER_32MS: AtomicU32 = AtomicU32::new(0);
/// Longest time spent inside our own detour for one poll.
static DETOUR_MAX_US: AtomicU32 = AtomicU32::new(0);
/// One sample per second of the worst gap in that second, kept for the diagnostics report.
///
/// A ten-second average hides the thing people report — "every couple of seconds" — because a
/// 60 ms hitch every 2 s is 3% of the time and indistinguishable from noise in a mean. The series
/// makes the periodicity visible at a glance.
const SERIES_LEN: usize = 300;
#[allow(clippy::declare_interior_mutable_const)]
const ZERO: AtomicU32 = AtomicU32::new(0);
static SERIES: [AtomicU32; SERIES_LEN] = [ZERO; SERIES_LEN];
static SERIES_HEAD: AtomicU32 = AtomicU32::new(0);
static SERIES_FILLED: AtomicU32 = AtomicU32::new(0);
/// The worst gap since the current one-second sample started, and when it started.
static SECOND_MAX_US: AtomicU32 = AtomicU32::new(0);
static SECOND_START: AtomicU64 = AtomicU64::new(0);

/// Advance the one-second series by one sample.
///
/// Driven by a one-second window timer, not by the pad poll: a keyboard-and-mouse player never
/// produces a pad poll, and their report must still fill (and the button must still count down).
pub fn tick() {
    let head = SERIES_HEAD.fetch_add(1, Ordering::Relaxed) as usize % SERIES_LEN;
    SERIES[head].store(SECOND_MAX_US.swap(0, Ordering::Relaxed), Ordering::Relaxed);
    SERIES_FILLED.fetch_add(1, Ordering::Relaxed);
}

/// How long the series still needs before it holds a full five minutes, in seconds.
///
/// The diagnostics button waits on this: a report that says "300 s sampled" should have
/// 300 seconds in it.
pub fn remaining_seconds() -> u32 {
    let filled = SERIES_FILLED.load(Ordering::Relaxed).min(SERIES_LEN as u32);
    (SERIES_LEN as u32).saturating_sub(filled)
}

/// Start the series again, as of a character loading: the samples of the last character say
/// nothing about this one.
pub fn restart_series() {
    for sample in SERIES.iter() {
        sample.store(0, Ordering::Relaxed);
    }
    SERIES_HEAD.store(0, Ordering::Relaxed);
    SERIES_FILLED.store(0, Ordering::Relaxed);
    SECOND_MAX_US.store(0, Ordering::Relaxed);
    SECOND_START.store(0, Ordering::Relaxed);
    LAST.store(0, Ordering::Relaxed);
    WINDOW.store(0, Ordering::Relaxed);
    GAP_MAX_US.store(0, Ordering::Relaxed);
    GAP_SUM_US.store(0, Ordering::Relaxed);
    POLLS.store(0, Ordering::Relaxed);
    GAP_OVER_32MS.store(0, Ordering::Relaxed);
    DETOUR_MAX_US.store(0, Ordering::Relaxed);
}

/// One line summarising the series, for the report header.
pub fn summary() -> String {
    let filled = (SERIES_FILLED.load(Ordering::Relaxed) as usize).min(SERIES_LEN);
    let head = SERIES_HEAD.load(Ordering::Relaxed) as usize % SERIES_LEN;
    let mut values: Vec<u32> = (0..filled)
        .map(|i| SERIES[(head + SERIES_LEN - filled + i) % SERIES_LEN].load(Ordering::Relaxed))
        .collect();
    if values.is_empty() {
        return "no samples yet".to_string();
    }
    values.sort_unstable();
    let at = |q: f64| values[((values.len() - 1) as f64 * q) as usize];
    format!(
        "{} s sampled: worst-gap per second p50 {}ms p90 {}ms max {}ms; {} seconds over 32ms",
        values.len(),
        at(0.5) as f32 / 1000.0,
        at(0.9) as f32 / 1000.0,
        values[values.len() - 1] as f32 / 1000.0,
        values.iter().filter(|v| **v > 32_000).count()
    )
}

/// The series itself, oldest first, one line per second: `t-300s ... t-1s`.
pub fn series() -> String {
    let filled = (SERIES_FILLED.load(Ordering::Relaxed) as usize).min(SERIES_LEN);
    let head = SERIES_HEAD.load(Ordering::Relaxed) as usize % SERIES_LEN;
    if filled == 0 {
        return String::new();
    }
    let mut out = String::with_capacity(filled * 8);
    for i in 0..filled {
        let index = (head + SERIES_LEN - filled + i) % SERIES_LEN;
        let us = SERIES[index].load(Ordering::Relaxed);
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&format!("{:.1}", us as f32 / 1000.0));
    }
    out
}

extern "system" {
    fn QueryPerformanceCounter(out: *mut i64) -> i32;
    fn QueryPerformanceFrequency(out: *mut i64) -> i32;
}

/// A monotonic tick. Cheap enough for the hot path: two instructions and a shared-page read.
pub fn ticks() -> u64 {
    let mut now = 0i64;
    if unsafe { QueryPerformanceCounter(&mut now) } == 0 {
        return 0;
    }
    if FREQ.load(Ordering::Relaxed) == 0 {
        let mut freq = 0i64;
        if unsafe { QueryPerformanceFrequency(&mut freq) } == 0 || freq <= 0 {
            return 0;
        }
        FREQ.store(freq as u64, Ordering::Relaxed);
    }
    now as u64
}

fn micros(ticks: u64) -> u32 {
    let freq = FREQ.load(Ordering::Relaxed);
    if freq == 0 {
        return 0;
    }
    ((ticks as u128 * 1_000_000) / freq as u128).min(u32::MAX as u128) as u32
}

/// Record one poll: `started` is the tick taken on entry to the detour.
pub fn report(started: u64) {
    let now = ticks();
    if now == 0 || started == 0 {
        return;
    }
    let cost = micros(now.saturating_sub(started));
    let previous = LAST.swap(now, Ordering::Relaxed);
    let window_start = WINDOW.load(Ordering::Relaxed);
    if window_start == 0 {
        WINDOW.store(now, Ordering::Relaxed);
        LAST.store(now, Ordering::Relaxed);
        return;
    }
    if previous != 0 {
        let gap = micros(now.saturating_sub(previous));
        if gap > 200_000 {
            // A gap this long is a stall or a pause (alt-tab, loading): start a fresh sample second
            // so the pause does not become the "worst gap" of the next one.
            SECOND_START.store(now, Ordering::Relaxed);
            SECOND_MAX_US.store(0, Ordering::Relaxed);
        }
        POLLS.fetch_add(1, Ordering::Relaxed);
        GAP_SUM_US.fetch_add(gap as u64, Ordering::Relaxed);
        GAP_MAX_US.fetch_max(gap, Ordering::Relaxed);
        if gap > 32_000 {
            GAP_OVER_32MS.fetch_add(1, Ordering::Relaxed);
        }
        // The worst gap in this second; `tick` files it and moves the series on.
        SECOND_MAX_US.fetch_max(gap, Ordering::Relaxed);
    }
    DETOUR_MAX_US.fetch_max(cost, Ordering::Relaxed);

    let elapsed = micros(now.saturating_sub(window_start));
    if elapsed < 10_000_000 || !crate::config::load().perf_log {
        return;
    }
    // One line per ten seconds, and only when asked for.
    let polls = POLLS.swap(0, Ordering::Relaxed);
    let sum = GAP_SUM_US.swap(0, Ordering::Relaxed);
    let peak = GAP_MAX_US.swap(0, Ordering::Relaxed);
    let over = GAP_OVER_32MS.swap(0, Ordering::Relaxed);
    let cost_peak = DETOUR_MAX_US.swap(0, Ordering::Relaxed);
    WINDOW.store(now, Ordering::Relaxed);
    let mean = if polls > 0 { sum / polls as u64 } else { 0 };
    crate::log(&format!(
        "perf: {polls} polls in {:.1}s, gap mean {}ms max {}ms ({} over 32ms), our detour max {}us, panel {}",
        elapsed as f64 / 1_000_000.0,
        mean as f64 / 1000.0,
        peak as f64 / 1000.0,
        over,
        cost_peak,
        if crate::UI_OPEN.load(Ordering::Relaxed) { "open" } else { "closed" }
    ));
}
