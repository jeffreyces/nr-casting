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
        POLLS.fetch_add(1, Ordering::Relaxed);
        GAP_SUM_US.fetch_add(gap as u64, Ordering::Relaxed);
        GAP_MAX_US.fetch_max(gap, Ordering::Relaxed);
        if gap > 32_000 {
            GAP_OVER_32MS.fetch_add(1, Ordering::Relaxed);
        }
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
