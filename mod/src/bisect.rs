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

use nr_casting_core::{gap_line, gap_stats, PhaseClock, PhaseTick};

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
    ("pad hooks off (casting stops)", bit::PAD),
    ("housekeeping thread off", bit::THREADS),
    ("everything off", bit::PAD | bit::RAW | bit::POINTER | bit::THREADS),
];

/// The most samples one phase can hold.
const SLOTS: usize = 90;

static DISABLED: AtomicU32 = AtomicU32::new(0);
static RUNNING: AtomicBool = AtomicBool::new(false);
static CLOCK: Mutex<PhaseClock> = Mutex::new(PhaseClock::new(PLAN.len(), PHASE_SECONDS));
static FILLED: [AtomicU32; PLAN.len()] = [const { AtomicU32::new(0) }; PLAN.len()];
static SERIES: [AtomicU32; PLAN.len() * SLOTS] =
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
pub fn start() {
    for filled in FILLED.iter() {
        filled.store(0, Ordering::Relaxed);
    }
    if let Ok(mut clock) = CLOCK.lock() {
        clock.restart();
    }
    DISABLED.store(PLAN[0].1, Ordering::Relaxed);
    RUNNING.store(true, Ordering::Relaxed);
    log_phase(0);
}

pub fn cancel() {
    if RUNNING.swap(false, Ordering::Relaxed) {
        DISABLED.store(0, Ordering::Relaxed);
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
pub fn on_second(sample_us: u32) -> bool {
    if !is_running() {
        return false;
    }
    let Ok(mut clock) = CLOCK.lock() else {
        return false;
    };
    let phase = clock.phase().min(PLAN.len() - 1);
    let filled = FILLED[phase].fetch_add(1, Ordering::Relaxed) as usize;
    if filled < SLOTS {
        SERIES[phase * SLOTS + filled].store(sample_us, Ordering::Relaxed);
    }
    match clock.tick() {
        PhaseTick::Second => false,
        PhaseTick::NextPhase => {
            let next = clock.phase();
            DISABLED.store(PLAN[next].1, Ordering::Relaxed);
            log_phase(next);
            false
        }
        PhaseTick::Finished => {
            drop(clock);
            RUNNING.store(false, Ordering::Relaxed);
            DISABLED.store(0, Ordering::Relaxed);
            crate::log("bisect: finished; every subsystem switched back on");
            true
        }
    }
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
    if (0..PLAN.len()).all(|index| samples(index).iter().all(|us| *us == 0)) {
        out.push_str(
            "# every phase is empty: the game never polled the pad or the keyboard, so there is no\n\
             # frame clock. Load a character and wait until it is polling - with a controller, ER\n\
             # only polls the pad when one is attached at launch - then run this again.\n",
        );
    }
    for (index, (label, bits)) in PLAN.iter().enumerate() {
        let values = samples(index);
        out.push_str(&format!(
            "phase {}/{} {} (off: {}): {}\n",
            index + 1,
            PLAN.len(),
            label,
            if *bits == 0 { "nothing".to_string() } else { describe(*bits) },
            gap_line(&gap_stats(&values))
        ));
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
