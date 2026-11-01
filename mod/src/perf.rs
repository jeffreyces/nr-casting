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

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

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
/// Total time spent inside our own detour, so the *average* poll cost is known as well as the worst.
/// A worst case of 200 us every ten seconds is noise; an average of 200 us over 120 polls a second
/// is 2.4% of the frame budget and worth knowing. Only the max was ever recorded, which cannot tell
/// those two apart - and a reporter's stutter that appears with a controller attached is exactly the
/// case where the pad path's average matters.
static DETOUR_SUM_US: AtomicU64 = AtomicU64::new(0);
/// One sample per second of the worst gap in that second, kept for the diagnostics report.
///
/// A ten-second average hides the thing people report — "every couple of seconds" — because a
/// 60 ms hitch every 2 s is 3% of the time and indistinguishable from noise in a mean. The series
/// makes the periodicity visible at a glance.
const SERIES_LEN: usize = 300;
#[allow(clippy::declare_interior_mutable_const)]
const ZERO: AtomicU32 = AtomicU32::new(0);
static SERIES: [AtomicU32; SERIES_LEN] = [ZERO; SERIES_LEN];
/// The frame thread's CPU time in the same second, in microseconds. Beside the gap it answers the
/// first question a stutter report has: was that second spent working, or waiting?
static SERIES_CPU: [AtomicU32; SERIES_LEN] = [ZERO; SERIES_LEN];
/// The most CPU the frame thread used in any second of the current window.
static CPU_MAX_US: AtomicU32 = AtomicU32::new(0);
static SERIES_HEAD: AtomicU32 = AtomicU32::new(0);
static SERIES_FILLED: AtomicU32 = AtomicU32::new(0);
/// The worst gap since the current one-second sample started, and when it started.
/// Gaps over 32 ms seen in the current second, so a report can say how often rather than how bad.
static SECOND_HITCHES: AtomicU32 = AtomicU32::new(0);
static SECOND_MAX_US: AtomicU32 = AtomicU32::new(0);
/// The panel's open state at the last written line, so a line is written when it changes.
static LAST_OPEN: AtomicBool = AtomicBool::new(false);
/// Ten-second windows since the last written line, for the quiet-run heartbeat.
static QUIET_WINDOWS: AtomicU32 = AtomicU32::new(0);
/// Write the next window whatever it says: set at load and when a character loads, so a report
/// always has a baseline line and a line marking the load.
static FORCE_LINE: AtomicBool = AtomicBool::new(true);

/// When the series last advanced, so the poll path can stand in if the timer never armed.
static TICKED_AT: AtomicU64 = AtomicU64::new(0);
/// When the pad was last polled, so the keyboard path knows to stand down.
static PAD_AT: AtomicU64 = AtomicU64::new(0);
/// When the game last presented: the frame clock, when the pass-through Present hook is in place.
static PRESENT_AT: AtomicU64 = AtomicU64::new(0);

/// Advance the one-second series by one sample.
///
/// Driven by a one-second window timer, not by the pad poll: a keyboard-and-mouse player never
/// produces a pad poll, and their report must still fill (and the button must still count down).
pub fn tick() -> Second {
    let sample = Second {
        worst_us: SECOND_MAX_US.swap(0, Ordering::Relaxed),
        hitches: SECOND_HITCHES.swap(0, Ordering::Relaxed),
    };
    let head = SERIES_HEAD.fetch_add(1, Ordering::Relaxed) as usize % SERIES_LEN;
    SERIES[head].store(sample.worst_us, Ordering::Relaxed);
    // Sampled from a thread that is allowed to make a syscall, once a second.
    let cpu = crate::threadcpu::cpu_since_last().unwrap_or(0).min(u32::MAX as u64) as u32;
    SERIES_CPU[head].store(cpu, Ordering::Relaxed);
    CPU_MAX_US.fetch_max(cpu, Ordering::Relaxed);
    SERIES_FILLED.fetch_add(1, Ordering::Relaxed);
    TICKED_AT.store(ticks(), Ordering::Relaxed);
    // The same sample feeds the guided bisect, so a phase and the rolling series are the same
    // measurement and cannot disagree.
    sample
}

/// Advance the series if a second has passed, called from the poll path.
///
/// This is the fallback for a window timer that could not be armed (`SetTimer` returns zero on a
/// thread that is out of timers, or for a window that does not exist yet). Without it the series
/// would never fill, and the report button would sit disabled for the whole session with nothing
/// said about why. With the timer working this costs two loads and a compare: `TICKED_AT` is
/// always fresh, so the branch is never taken.
pub fn tick_if_due(poll_ticks: u64) {
    let last = TICKED_AT.load(Ordering::Relaxed);
    if last == 0 || micros(poll_ticks.saturating_sub(last)) >= 1_000_000 {
        let _ = tick();
    }
}

/// How recent a tick must be for its clock to count as live.
const LIVE_WINDOW_US: u32 = 2_000_000;

/// Whether the message pump is the only clock allowed to record.
///
/// A guided run switches the pad hooks off in phase 4, which stops `pad_live()` from being true -
/// and the pump then takes over, recording punts at a different cadence (once a frame instead of
/// about twice a frame). The two clocks are not comparable, so a phase measured on one cannot be
/// set beside a phase measured on the other. Pinning the pump for the whole run means every phase
/// is the same measurement, and switching the pad hooks off cannot change the instrument.
static PUMP_ONLY: AtomicBool = AtomicBool::new(false);

/// Pin the message pump as the only frame clock, or release it. Returns the previous state.
pub fn pin_pump(on: bool) -> bool {
    PUMP_ONLY.swap(on, Ordering::Relaxed)
}

/// Whether the pump is currently the only clock.
pub fn pump_pinned() -> bool {
    PUMP_ONLY.load(Ordering::Relaxed)
}

/// A finished ten-second window, waiting to be written.
///
/// The line used to be formatted and written *inside* the hook, on the game's own thread: file I/O
/// plus a flush, in the middle of a frame, every ten seconds. Measured at up to 17 ms in a
/// reporter's log (`our detour max 17055us`), which is a stutter we were causing ourselves. The
/// hook now only stores these numbers; a thread that is allowed to block writes them.
static PENDING: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
static PENDING_OPEN: AtomicBool = AtomicBool::new(false);
static HAS_PENDING: AtomicBool = AtomicBool::new(false);

/// Start the thread that writes perf lines, once, at load.
pub fn start_writer() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if !HAS_PENDING.swap(false, Ordering::Acquire) {
            continue;
        }
        let polls = PENDING[0].load(Ordering::Relaxed);
        let sum = PENDING[1].load(Ordering::Relaxed);
        let peak = PENDING[2].load(Ordering::Relaxed);
        let over = PENDING[3].load(Ordering::Relaxed);
        let cost_peak = PENDING[4].load(Ordering::Relaxed);
        let elapsed = PENDING[5].load(Ordering::Relaxed);
        let cpu_peak = PENDING[6].load(Ordering::Relaxed);
        let cost_mean = PENDING[7].load(Ordering::Relaxed);
        let mean = if polls > 0 { sum / polls } else { 0 };
        let open = PENDING_OPEN.load(Ordering::Relaxed);
        crate::log(&format!(
            "perf: {polls} input ticks in {:.1}s, gap mean {}ms max {}ms ({} over 32ms), \
             our detour mean {}us max {}us, frame thread cpu max {:.1}ms, panel {}",
            elapsed as f64 / 1_000_000.0,
            mean as f64 / 1000.0,
            peak as f64 / 1000.0,
            over,
            cost_mean,
            cost_peak,
            cpu_peak as f64 / 1000.0,
            if open { "open" } else { "closed" }
        ));
    });
}

/// A lifetime tick count for the per-phase tick rates.
///
/// `POLLS` is the ten-second window's counter and is swapped to zero every time a line is written,
/// which is exactly wrong for measuring a phase: a phase that spans a window boundary saw its
/// count reset and reported `0 ticks, 0/s`, while the phase's own samples were perfectly good.
static TICKS_TOTAL: AtomicU64 = AtomicU64::new(0);

/// How many ticks the recorder has seen, for per-phase tick rates.
pub fn poll_count() -> u64 {
    TICKS_TOTAL.load(Ordering::Relaxed)
}

/// Whether presents are being seen: the preferred clock, because presenting is what a frame is.
pub fn present_live() -> bool {
    let last = PRESENT_AT.load(Ordering::Relaxed);
    last != 0 && micros(ticks().saturating_sub(last)) <= LIVE_WINDOW_US
}

/// One present, timed on entry. The pass-through hook calls this and nothing else.
pub fn report_present(started: u64) {
    let now = ticks();
    if now == 0 || started == 0 {
        return;
    }
    let cost = micros(now.saturating_sub(started));
    PRESENT_AT.store(now, Ordering::Relaxed);
    record(now, cost);
}

/// Whether the game has polled the pad within the last couple of seconds.
///
/// The whole frame measurement hangs off the pad poll: it is the only per-frame call the game makes
/// into code we are allowed to hook (no swap-chain hook, deliberately). No polls means no frame
/// clock at all - a run with no controller connected records six empty minutes, so the UI refuses to
/// start one and the report says why rather than printing zeros.
pub fn pad_live() -> bool {
    let last = PAD_AT.load(Ordering::Relaxed);
    last != 0 && micros(ticks().saturating_sub(last)) <= LIVE_WINDOW_US
}

/// Whether *any* of our per-frame hooks is being called: the pad if a controller is in use, the
/// keyboard and mouse otherwise. Either is a frame clock, which is what a run needs.
pub fn frame_clock_live() -> bool {
    let last = LAST.load(Ordering::Relaxed);
    last != 0 && micros(ticks().saturating_sub(last)) <= LIVE_WINDOW_US
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
    FORCE_LINE.store(true, Ordering::Relaxed);
    SECOND_MAX_US.store(0, Ordering::Relaxed);
    TICKED_AT.store(0, Ordering::Relaxed);
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
    let values: Vec<u32> = (0..filled)
        .map(|i| SERIES[(head + SERIES_LEN - filled + i) % SERIES_LEN].load(Ordering::Relaxed))
        .collect();
    if values.is_empty() {
        return "no samples yet".to_string();
    }
    // One implementation, in the tested crate: the rolling series and a bisect phase have to agree,
    // and quiet seconds (no pad poll at all) are counted separately rather than averaged in as 0 ms.
    nr_casting_core::gap_line(&nr_casting_core::gap_stats(&values))
}

/// The series itself, oldest first, one line per second: `t-300s ... t-1s`.
/// The frame thread's CPU time per second, beside the gap series and summarised against it.
///
/// The summary is the point: "194 seconds were slow" is a symptom, while "of those, 133 were spent
/// waiting rather than working" is a diagnosis. Waiting means the frame thread handed the work off
/// and had nothing to do - a GPU-bound frame, which is what a render-side wrapper or an expensive
/// effects chain produces. Working means the CPU itself was busy - game logic, or something
/// scanning the process.
pub fn cpu_report() -> String {
    let filled = (SERIES_FILLED.load(Ordering::Relaxed) as usize).min(SERIES_LEN);
    if filled == 0 {
        return String::new();
    }
    let head = SERIES_HEAD.load(Ordering::Relaxed) as usize % SERIES_LEN;
    let mut per_second = Vec::with_capacity(filled);
    let mut per_frame = Vec::with_capacity(filled);
    let mut ratios = Vec::with_capacity(filled);
    let mut slow = 0usize;
    let mut working = 0usize;
    let mut waiting = 0usize;
    for i in 0..filled {
        let index = (head + SERIES_LEN - filled + i) % SERIES_LEN;
        let gap = SERIES[index].load(Ordering::Relaxed) as u64;
        let cpu = SERIES_CPU[index].load(Ordering::Relaxed) as u64;
        per_second.push(cpu);
        // One tick per frame (the pump clock is the game's own loop), so a second holds
        // 1e6/gap frames and the CPU spent on each is cpu/frames. Comparing a whole second's CPU
        // with one frame's gap - the obvious mistake - makes every second look CPU-bound.
        if gap == 0 {
            continue;
        }
        let frames = 1_000_000.0 / gap as f64;
        let cpu_frame = cpu as f64 / frames;
        per_frame.push(cpu_frame);
        // The share of each frame the thread actually spent working. 0.9 means the frame time is
        // the thread; 0.2 means the thread handed the frame off and waited three quarters of it,
        // which is what a GPU-bound frame looks like.
        ratios.push((cpu_frame / gap as f64).clamp(0.0, 2.0));
        if gap > 32_000 {
            slow += 1;
            let share = cpu_frame / gap as f64;
            if share >= 0.8 {
                working += 1;
            } else if share <= 0.4 {
                waiting += 1;
            }
        }
    }
    let mut sorted = per_frame.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let at = |p: f64| -> f64 {
        if sorted.is_empty() {
            return 0.0;
        }
        let index = ((sorted.len() - 1) as f64 * p) as usize;
        sorted[index] / 1000.0
    };
    let core = per_second.iter().map(|us| *us as f64 / 10_000.0).sum::<f64>()
        / per_second.len().max(1) as f64;
    ratios.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let share_at = |p: f64| -> f64 {
        if ratios.is_empty() {
            return 0.0;
        }
        ratios[((ratios.len() - 1) as f64 * p) as usize] * 100.0
    };
    let mut out = format!(
        "frame thread CPU: {:.0}% of one core on average; per frame p50 {:.2} ms, p90 {:.2} ms, \
         max {:.2} ms; the thread used p50 {:.0}% of each frame's time (p90 {:.0}%) - near 100% \
         means the frame time is the CPU, low means it was waiting on something else. A tick is \
         assumed to be a frame; the tick rate is printed above.",
        core,
        at(0.5),
        at(0.9),
        at(1.0),
        share_at(0.5),
        share_at(0.9)
    );
    if slow == 0 {
        out.push_str("; no slow seconds to attribute");
    } else {
        out.push_str(&format!(
            "; of {slow} seconds over 32ms, {working} spent >=80% of the frame working, {waiting} \
             spent <=40% (waiting on the GPU, a wrapper or vsync), {} were between",
            slow.saturating_sub(working + waiting)
        ));
    }
    out.push('\n');
    out.push_str("frame thread CPU (percent of one core, oldest first):\n");
    for (i, cpu) in per_second.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&format!("{:.0}", *cpu as f32 / 10_000.0));
    }
    out
}

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
        if us == 0 {
            // No tick in that second: a pause, a load screen, or nothing polling. Printing 0.0
            // would read as a perfect second, which is the opposite of what it is.
            out.push_str("n/a");
        } else {
            out.push_str(&format!("{:.1}", us as f32 / 1000.0));
        }
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

/// One second's worth of frame data.
///
/// `worst_us` is the slowest gap in that second; `hitches` is how many gaps went over 32 ms (two
/// frames at 60 Hz). The two together are the difference between "it stutters" and "it stutters
/// four times a second", which is the question a stutter report actually asks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Second {
    pub worst_us: u32,
    pub hitches: u32,
}

/// Record one poll: `started` is the tick taken on entry to the detour.
pub fn report(started: u64) {
    let now = ticks();
    if now == 0 || started == 0 {
        return;
    }
    let cost = micros(now.saturating_sub(started));
    PAD_AT.store(now, Ordering::Relaxed);
    // A real per-frame clock is in place: the pad poll and the pump are proxies for it, and mixing
    // them in would make every gap a mixture of two cadences.
    if present_live() {
        return;
    }
    if pump_pinned() {
        // The pad still tells us a controller is there (so the panel does not claim otherwise), but
        // it must not feed the gaps: a run is measuring the pump.
        return;
    }
    record(now, cost);
}

/// The fallback clock: the game's own message pump, which runs once a frame whatever the input
/// device is.
///
/// `GetAsyncKeyState` looked like a frame clock and is not one - ER calls it when it wants a key's
/// state, 24 times in seven minutes on a keyboard-and-mouse session, so a "gap" was the time
/// between two keystrokes and a run recorded six empty minutes. `PeekMessageW` is called by the
/// loop itself, so it is the one per-frame signal that exists without a controller. It stands down
/// while the pad is live: the pad poll is the same measurement with the game's own frame cadence.
pub fn report_pump(started: u64) {
    let now = ticks();
    if now == 0 || started == 0 {
        return;
    }
    if present_live() || (pad_live() && !pump_pinned()) {
        return;
    }
    let cost = micros(now.saturating_sub(started));
    record(now, cost);
}

fn record(now: u64, cost: u32) {
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
            SECOND_MAX_US.store(0, Ordering::Relaxed);
        }
        POLLS.fetch_add(1, Ordering::Relaxed);
        TICKS_TOTAL.fetch_add(1, Ordering::Relaxed);
        GAP_SUM_US.fetch_add(gap as u64, Ordering::Relaxed);
        GAP_MAX_US.fetch_max(gap, Ordering::Relaxed);
        if gap > 32_000 {
            GAP_OVER_32MS.fetch_add(1, Ordering::Relaxed);
            SECOND_HITCHES.fetch_add(1, Ordering::Relaxed);
        }
        // The worst gap in this second; `tick` files it and moves the series on.
        SECOND_MAX_US.fetch_max(gap, Ordering::Relaxed);
    }
    DETOUR_MAX_US.fetch_max(cost, Ordering::Relaxed);
    DETOUR_SUM_US.fetch_add(cost as u64, Ordering::Relaxed);
    // Stand in for a timer that never armed; with the timer working this is two loads and a
    // compare, because `TICKED_AT` is always a second or less old.
    tick_if_due(now);

    let elapsed = micros(now.saturating_sub(window_start));
    if elapsed < 10_000_000 || !crate::config::load().perf_log {
        return;
    }
    // The window closes every ten seconds; the line is only written when the window says
    // something. Six identical "0 over 32ms" lines a minute buried the interesting ones in a
    // reporter's log, which is the opposite of what a log is for.
    let polls = POLLS.swap(0, Ordering::Relaxed);
    let sum = GAP_SUM_US.swap(0, Ordering::Relaxed);
    let peak = GAP_MAX_US.swap(0, Ordering::Relaxed);
    let over = GAP_OVER_32MS.swap(0, Ordering::Relaxed);
    let cost_peak = DETOUR_MAX_US.swap(0, Ordering::Relaxed);
    let cost_sum = DETOUR_SUM_US.swap(0, Ordering::Relaxed);
    let cost_mean = if polls > 0 { cost_sum / polls as u64 } else { 0 };
    let cpu_peak = CPU_MAX_US.swap(0, Ordering::Relaxed);
    WINDOW.store(now, Ordering::Relaxed);
    let mean = if polls > 0 { sum / polls as u64 } else { 0 };

    // Interesting: a frame the game lost, a hook that took far too long, or the panel opening or
    // closing. The last one is not in the counters, so it is compared with the previous line.
    let open = crate::UI_OPEN.load(Ordering::Relaxed);
    let panel_changed = LAST_OPEN.swap(open, Ordering::Relaxed) != open;
    let interesting = FORCE_LINE.swap(false, Ordering::Relaxed)
        || over > 0
        || peak >= 25_000
        || cost_peak >= 250_000
        || panel_changed;
    // A quiet run still gets a heartbeat, so a report can show the sampler was alive: one line
    // every five minutes instead of every ten seconds.
    let quiet = QUIET_WINDOWS.fetch_add(1, Ordering::Relaxed) + 1;
    if !interesting && quiet < 30 {
        return;
    }
    QUIET_WINDOWS.store(0, Ordering::Relaxed);
    // Nothing here may block: this runs inside the game's own hook. The numbers go to atomics and
    // the writer thread does the formatting and the file write.
    PENDING[0].store(polls as u64, Ordering::Relaxed);
    PENDING[1].store(sum, Ordering::Relaxed);
    PENDING[2].store(peak as u64, Ordering::Relaxed);
    PENDING[3].store(over as u64, Ordering::Relaxed);
    PENDING[4].store(cost_peak as u64, Ordering::Relaxed);
    PENDING[5].store(elapsed as u64, Ordering::Relaxed);
    PENDING[6].store(cpu_peak as u64, Ordering::Relaxed);
    PENDING[7].store(cost_mean, Ordering::Relaxed);
    PENDING_OPEN.store(open, Ordering::Relaxed);
    let _ = mean;
    HAS_PENDING.store(true, Ordering::Release);
}
