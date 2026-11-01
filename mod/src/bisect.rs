//! A guided bisect: one button, six one-minute phases, one report.
//!
//! A reporter's question - "does it stutter without subsystem X" - used to need a different build and
//! a different session each time, which is four round trips through somebody else's evening. This
//! runs the comparison in one sitting instead: the player loads a character, stands still, and the
//! mod switches one subsystem off at a time, recording the same per-second frame gaps for every
//! phase.
//!
//! Nothing is uninstalled. Each hooked function checks its bit and calls the original untouched when
//! it is set, so a "disabled" phase still has the hook and its jump in the path - which is what makes
//! the comparison mean something: it separates *what our code does* from *the fact that we are
//! loaded*.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

use nr_casting_core::{gap_stats, PhaseClock, PhaseTick};

/// The subsystems a phase can switch off.
pub mod bit {
    pub const PAD: u32 = 1;
    pub const RAW: u32 = 2;
    pub const POINTER: u32 = 4;
    pub const THREADS: u32 = 8;
}

/// Seconds each phase runs.
pub const PHASE_SECONDS: u32 = 60;

/// The phases, in order: a baseline first, each subsystem alone, then everything off.
///
/// `pad` costs casting for a minute and is in the list anyway: it is the hook the game calls most
/// often, and a comparison that skipped it would be missing the prime suspect.
const PLAN: [(&str, u32); 6] = [
    ("everything on", 0),
    ("pointer detours off", bit::POINTER),
    ("raw input hooks off", bit::RAW),
    ("pad casting off (the hook still times)", bit::PAD),
    ("housekeeping thread off", bit::THREADS),
    ("all of our work off", bit::PAD | bit::RAW | bit::POINTER | bit::THREADS),
];

/// The most samples one phase can hold.
const SLOTS: usize = 90;

static DISABLED: AtomicU32 = AtomicU32::new(0);
static RUNNING: AtomicBool = AtomicBool::new(false);
static CLOCK: Mutex<PhaseClock> = Mutex::new(PhaseClock::new(PLAN.len(), PHASE_SECONDS));
/// Which hook measured the frames for this run. Every phase has to use the same one, or the phases
/// are not comparable - the first run proved it by switching mid-phase and moving its own p50.
static CLOCK_KIND: Mutex<&'static str> = Mutex::new("none");
/// Ticks recorded during each phase, so a phase measured at a different tick rate is visible as
/// such instead of being read as a difference in smoothness.
static TICKS: [AtomicU32; PLAN.len()] = [const { AtomicU32::new(0) }; PLAN.len()];
/// The recorder's tick count when the current phase began.
static PHASE_POLLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static FILLED: [AtomicU32; PLAN.len()] = [const { AtomicU32::new(0) }; PLAN.len()];
static SERIES: [AtomicU32; PLAN.len() * SLOTS] =
    [const { AtomicU32::new(0) }; PLAN.len() * SLOTS];
/// Individual gaps over 32 ms in each second, for the "how often" half of the picture.
static HITCHES: [AtomicU32; PLAN.len() * SLOTS] =
    [const { AtomicU32::new(0) }; PLAN.len() * SLOTS];

/// Total wall time a run needs, in seconds (six minutes).
pub fn total_seconds() -> u32 {
    PHASE_SECONDS * PLAN.len() as u32
}

pub fn phases() -> usize {
    PLAN.len()
}

/// Whether a subsystem is switched off right now.
///
/// One relaxed load; called on the input hot path, so it does not lock or allocate.
#[inline]
pub fn disabled(bits: u32) -> bool {
    DISABLED.load(Ordering::Relaxed) & bits != 0
}

pub fn is_running() -> bool {
    RUNNING.load(Ordering::Relaxed)
}

/// Start the run at phase one, everything switched back on.
///
/// Returns `false` when nothing is feeding the frame clock: a run of six empty minutes is worse
/// than no run, and the guard belongs here rather than in the button that happens to call it.
pub fn start() -> bool {
    if !crate::perf::frame_clock_live() {
        crate::log("bisect: refused to start - nothing is feeding the frame clock");
        return false;
    }
    for filled in FILLED.iter() {
        filled.store(0, Ordering::Relaxed);
    }
    if let Ok(mut clock) = CLOCK.lock() {
        clock.restart();
    }
    // One clock for the whole run. The pump runs once a frame whatever the input device, so it
    // keeps measuring even in the phase that switches the pad hooks off - which is exactly the
    // phase a pad clock cannot measure. Leaving both clocks free meant phase 4 and phase 6 were
    // recorded about once a frame while phases 1, 2, 3 and 5 were recorded about twice a frame,
    // and the report claimed they were the same measurement.
    crate::perf::pin_pump(true);
    PHASE_POLLS.store(crate::perf::poll_count(), Ordering::Relaxed);
    let kind = "message pump (pinned for the run)";
    if let Ok(mut clock) = CLOCK_KIND.lock() {
        *clock = kind;
    }
    DISABLED.store(PLAN[0].1, Ordering::Relaxed);
    RUNNING.store(true, Ordering::Relaxed);
    crate::log(&format!("bisect: frame clock is the {kind}"));
    log_phase(0);
    true
}

pub fn cancel() {
    if RUNNING.swap(false, Ordering::Relaxed) {
        DISABLED.store(0, Ordering::Relaxed);
        crate::perf::pin_pump(false);
        crate::log("bisect: cancelled; every subsystem back on");
    }
}

/// One line for the panel: which phase, what it has off, and how long is left.
pub fn status() -> Option<String> {
    if !is_running() {
        return None;
    }
    let clock = CLOCK.lock().ok()?;
    let phase = clock.phase().min(PLAN.len() - 1);
    Some(format!(
        "phase {}/{}: {}, {}s left",
        phase + 1,
        clock.phases(),
        PLAN[phase].0,
        clock.left()
    ))
}

/// Record one second's worst gap, and move the run on when the phase's minute is up.
///
/// Returns `true` on the second the whole run finished, so the caller can write the report.
pub fn on_second(second: crate::perf::Second) -> bool {
    if !is_running() {
        return false;
    }
    // A poisoned lock (a panic in another thread while it held this) must not turn the run into a
    // silent no-op: the sample is still worth recording, and the phase clock is plain data.
    let mut clock = CLOCK.lock().unwrap_or_else(|e| e.into_inner());
    let phase = clock.phase().min(PLAN.len() - 1);
    let filled = FILLED[phase].fetch_add(1, Ordering::Relaxed) as usize;
    if filled < SLOTS {
        SERIES[phase * SLOTS + filled].store(second.worst_us, Ordering::Relaxed);
        HITCHES[phase * SLOTS + filled].store(second.hitches, Ordering::Relaxed);
    }
    match clock.tick() {
        PhaseTick::Second => false,
        PhaseTick::NextPhase => {
            let ticks = close_phase(phase);
            let next = clock.phase();
            DISABLED.store(PLAN[next].1, Ordering::Relaxed);
            log_phase(next);
            crate::log(&format!("bisect: phase {} recorded {ticks} ticks", phase + 1));
            false
        }
        PhaseTick::Finished => {
            let ticks = close_phase(phase);
            drop(clock);
            RUNNING.store(false, Ordering::Relaxed);
            DISABLED.store(0, Ordering::Relaxed);
            crate::perf::pin_pump(false);
            crate::log(&format!("bisect: phase {} recorded {ticks} ticks", phase + 1));
            crate::log("bisect: finished; every subsystem switched back on");
            true
        }
    }
}

/// Store the tick count of the phase that just ended and start counting for the next one.
fn close_phase(index: usize) -> u64 {
    let now = crate::perf::poll_count();
    let start = PHASE_POLLS.swap(now, Ordering::Relaxed);
    let ticks = now.saturating_sub(start);
    TICKS[index].store(ticks.min(u32::MAX as u64) as u32, Ordering::Relaxed);
    ticks
}

fn log_phase(index: usize) {
    crate::log(&format!(
        "bisect: phase {}/{} ({}) - {} s",
        index + 1,
        PLAN.len(),
        PLAN[index].0,
        PHASE_SECONDS
    ));
}

/// The report section: one line per phase, then the phases' series side by side.
pub fn section() -> String {
    let mut out = String::from(
        "# six phases of 60 s, switched by the mod, nothing for the player to change\n\
         # worst gap per second is what one second's slowest frame cost; 32 ms is two frames at 60 Hz\n",
    );
    let kind = CLOCK_KIND.lock().map(|k| *k).unwrap_or("none");
    out.push_str(&format!(
        "# measured with the {kind}. One clock for every phase on purpose: a pad clock stops\n\
         # being fed in the phase that switches the pad hooks off, and the message pump takes over\n\
         # at about half the tick rate - two instruments, and phases that cannot be compared. The\n\
         # tick count on each phase is how you check that the rate stayed put.\n"
    ));
    if (0..PLAN.len()).all(|index| samples(index).iter().all(|us| *us == 0)) {
        out.push_str(
            "# every phase is empty: nothing was feeding the frame clock - not the pad, not the\n\
             # message pump. Load a character and run this again; the state section says which\n\
             # clock is live.\n",
        );
    }
    for (index, (label, bits)) in PLAN.iter().enumerate() {
        let values = samples(index);
        let stats = gap_stats(&values);
        let hitches: u32 = (0..values.len())
            .map(|i| HITCHES[index * SLOTS + i].load(Ordering::Relaxed))
            .sum();
        let seconds_with_hitches = (0..values.len())
            .filter(|i| HITCHES[index * SLOTS + i].load(Ordering::Relaxed) > 0)
            .count();
        // The second the worst gap fell in, so a stall can be matched against the log.
        let worst_at = values
            .iter()
            .enumerate()
            .max_by_key(|(_, us)| **us)
            .map(|(i, _)| i + 1)
            .unwrap_or(0);
        // The line itself lives in the tested crate: it is a format string with six numbers in
        // it, and the last time one of those drifted it printed the wrong count for weeks.
        out.push_str(&nr_casting_core::phase_lines(&nr_casting_core::PhaseSummary {
            index: index + 1,
            phases: PLAN.len(),
            label,
            off: &if *bits == 0 { "nothing".to_string() } else { describe(*bits) },
            stats,
            hitches,
            seconds_with_hitches,
            seconds: values.len(),
            worst_at,
            ticks: TICKS[index].load(Ordering::Relaxed),
        }));
        out.push('\n');
    }
    out.push('\n');
    for (index, (label, _)) in PLAN.iter().enumerate() {
        let values = samples(index);
        if values.is_empty() {
            continue;
        }
        let line: Vec<String> = values
            .iter()
            .map(|us| format!("{:.1}", *us as f32 / 1000.0))
            .collect();
        out.push_str(&format!("phase {} {label}:\n  {}\n", index + 1, line.join(" ")));
    }
    out.trim_end().to_string()
}

fn samples(index: usize) -> Vec<u32> {
    let filled = (FILLED[index].load(Ordering::Relaxed) as usize).min(SLOTS);
    (0..filled)
        .map(|i| SERIES[index * SLOTS + i].load(Ordering::Relaxed))
        .collect()
}

fn describe(bits: u32) -> String {
    [bit::PAD, bit::RAW, bit::POINTER, bit::THREADS]
        .iter()
        .filter(|b| bits & **b != 0)
        .map(|b| match *b {
            bit::PAD => "pad hooks",
            bit::RAW => "raw input hooks",
            bit::POINTER => "pointer detours",
            _ => "housekeeping thread",
        })
        .collect::<Vec<_>>()
        .join(", ")
}
