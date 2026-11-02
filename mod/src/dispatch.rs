//! One place that owns time-gated work.
//!
//! Three kinds of clock run through this mod, and only one of them is ours to remove:
//!
//! 1. the game's own calls into us (`XInputGetState`, `PeekMessageW`, `Present`) - those *are*
//!    events, we ride them and never generate them;
//! 2. wall-clock intervals we invented, asking "has anything changed?" on a timer;
//! 3. state inspection loops, which are (2) with a memory read behind them.
//!
//! This module is the answer to (2) and (3). A hook or a caller *marks* what happened; nothing runs
//! until the dispatcher's next pass, so an edge that fires at 60 Hz cannot become 60 Hz work. Each
//! handler also declares the shortest interval it is worth running at, which is the throttle: the
//! edge says something changed, the interval says how often that is worth acting on, and a mark that
//! arrives inside the interval is dropped rather than queued - for these handlers the newest state
//! is the only state that matters.
//!
//! What deliberately stays out: cache invalidation. Dropping the weapon pair is one relaxed store,
//! and routing it through a mark, a swap and a table lookup would be three more loads to do the same
//! thing on the same thread. The rule is not "everything goes through the dispatcher", it is "nothing
//! asks a clock a question whose answer arrives by itself".

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// The panel is about to be shown: read the things the panel displays, do not serve them from a
/// cache filled for the last cast.
pub const PANEL_OPEN: u32 = 1 << 0;

/// The game's own selection may need correcting so the HUD agrees with what R1 will cast. Marked by
/// every pad poll that is idle and D-pad-free; throttled to the rate the mirror is worth.
pub const MIRROR: u32 = 1 << 1;

/// Marks waiting to be acted on. Read-and-clear, so two threads marking the same bit cost one run.
static PENDING: AtomicU32 = AtomicU32::new(0);

/// A unit of work with its own throttle.
struct Handler {
    bit: u32,
    name: &'static str,
    /// Shortest interval between runs, in milliseconds.
    min_ms: u32,
    run: fn(),
}

const HANDLERS: &[Handler] = &[
    Handler {
        bit: PANEL_OPEN,
        name: "panel open",
        // Not throttled: it runs when the panel opens, which the player does by hand.
        min_ms: 0,
        run: on_panel_open,
    },
    Handler {
        bit: MIRROR,
        name: "hud mirror",
        // The mirrored value only changes when the player moves the wheel by hand, and a pass is a
        // weapon read plus a selection compare (three `VirtualQuery`) and possibly a write. 10 Hz is
        // already more than a number in a corner needs.
        min_ms: 100,
        run: crate::input::mirror_now,
    },
];

const N: usize = HANDLERS.len();
static LAST_RUN: [AtomicU32; N] = [const { AtomicU32::new(0) }; N];
static RUNS: [AtomicU32; N] = [const { AtomicU32::new(0) }; N];
static SUM_US: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static WORST_US: [AtomicU32; N] = [const { AtomicU32::new(0) }; N];
static THROTTLED: [AtomicU32; N] = [const { AtomicU32::new(0) }; N];

/// Say that something happened. Cheap enough for a per-frame hook: one `lock or`.
pub fn mark(bits: u32) {
    PENDING.fetch_or(bits, Ordering::Relaxed);
}

/// Run whatever has been marked and is due. Called once per pad poll and once per frame.
pub fn run(now: u32) {
    let due = PENDING.swap(0, Ordering::AcqRel);
    if due == 0 {
        return;
    }
    for (slot, handler) in HANDLERS.iter().enumerate() {
        if due & handler.bit == 0 {
            continue;
        }
        if now.wrapping_sub(LAST_RUN[slot].load(Ordering::Relaxed)) < handler.min_ms {
            // Inside its interval: the mark is dropped, not queued. For these handlers the state is
            // the message - a second mirror pass a millisecond later would read the same world.
            THROTTLED[slot].fetch_add(1, Ordering::Relaxed);
            continue;
        }
        LAST_RUN[slot].store(now, Ordering::Relaxed);
        let started = crate::perf::ticks();
        (handler.run)();
        let took = crate::perf::micros_since(started) as u32;
        RUNS[slot].fetch_add(1, Ordering::Relaxed);
        SUM_US[slot].fetch_add(took as u64, Ordering::Relaxed);
        WORST_US[slot].fetch_max(took, Ordering::Relaxed);
    }
}

/// The panel is about to open. Everything it displays is read now, on the game's thread, and the
/// value is stamped so the report can say how old it is.
fn on_panel_open() {
    crate::game::request_selection_sample();
    crate::game::invalidate_weapon_cache();
}

/// The dispatcher's own numbers, for the report.
///
/// Printed even when a handler never ran: "never ran" is information - a housekeeping pass feeding
/// from an event nobody marks is the failure mode this section exists to make visible.
pub fn report(out: &mut String) {
    out.push_str("\n==== dispatch ====\n");
    out.push_str("# how often each time-gated job ran, and what it cost\n");
    for (slot, handler) in HANDLERS.iter().enumerate() {
        let runs = RUNS[slot].load(Ordering::Relaxed);
        let sum = SUM_US[slot].load(Ordering::Relaxed);
        let worst = WORST_US[slot].load(Ordering::Relaxed);
        let throttled = THROTTLED[slot].load(Ordering::Relaxed);
        if runs == 0 {
            out.push_str(&format!("  {}: never ran\n", handler.name));
            continue;
        }
        let mean = sum as f64 / runs as f64;
        out.push_str(&format!(
            "  {}: {} runs, mean {mean:.1} us, worst {worst} us, {} marks inside its {min_ms} ms interval\n",
            handler.name,
            runs,
            throttled,
            min_ms = handler.min_ms
        ));
    }
}
