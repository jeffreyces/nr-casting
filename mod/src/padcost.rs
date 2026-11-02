//! Where the pad path's time actually goes.
//!
//! The guided run can only say "the pad group costs something": phase 4 switches our whole pad
//! path off and a reporter's frames got 20 ms faster. That is a group, not a line of code, and the
//! next question is which part. The detour's own average cost is already reported (`our detour
//! mean`), which covers the whole call including this work, so it cannot separate "our code is
//! slow" from "the game does expensive work because of what our code did".
//!
//! Four clocks, each a relaxed add: the whole `adjust_inner`, the idle fast path, the throttled
//! selection sample, and the chord/cast path. Costs microseconds per poll and nothing when the
//! perf log is off, which is the rule everything in this mod follows.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub const TOTAL: usize = 0;
pub const IDLE: usize = 1;
pub const CHORD: usize = 2;
const N: usize = 3;

// The selection sample used to have a slot here. It is no longer read from the poll thread at all,
// so there is nothing left to time: a zero row in a report is worse than no row.
const NAMES: [&str; N] = ["adjust_inner", "idle path", "chord path"];

static CALLS: [AtomicU32; N] = [const { AtomicU32::new(0) }; N];
/// How often a part cost more than 250 us - about a fresh page fault's price, and far more than
/// any arithmetic in these functions. A count is what separates "one slow read" from "every read on
/// a machine whose pages are not resident", which are different bugs with different fixes.
static SLOW: [AtomicU32; N] = [const { AtomicU32::new(0) }; N];
/// Idle passes that had to touch game memory anyway - a borrow still in flight. The idle path is
/// supposed to read nothing at all, so this number is the proof rather than the hope.
static IDLE_READS: AtomicU32 = AtomicU32::new(0);
const SLOW_US: u64 = 250;
static SUM_US: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static MAX_US: [AtomicU32; N] = [const { AtomicU32::new(0) }; N];

/// Record one finished measurement. `us` is clamped: a single 40 s stall must not look like a
/// saturated counter, and the max is what a reader compares against a frame.
pub fn record(slot: usize, us: u64) {
    if slot >= N {
        return;
    }
    CALLS[slot].fetch_add(1, Ordering::Relaxed);
    if us > SLOW_US {
        SLOW[slot].fetch_add(1, Ordering::Relaxed);
    }
    SUM_US[slot].fetch_add(us, Ordering::Relaxed);
    MAX_US[slot].fetch_max(us.min(u32::MAX as u64) as u32, Ordering::Relaxed);
}

/// A measurement of `slot`, recorded when it drops.
///
/// Dropping rather than calling `stop()` is deliberate: most `adjust_inner` paths end in an early
/// `return`, and a timer that has to be stopped explicitly would silently miss exactly the paths a
/// report is asking about.
pub struct Timer {
    slot: usize,
    started: u64,
}

impl Timer {
    pub fn start(slot: usize) -> Timer {
        Timer {
            slot,
            started: crate::perf::ticks(),
        }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        record(self.slot, crate::perf::micros_since(self.started));
    }
}

/// Record that an idle pass touched game memory.
pub fn note_idle_read() {
    IDLE_READS.fetch_add(1, Ordering::Relaxed);
}

/// One line per clock: mean and worst, and how often it ran.
///
/// Mean is what a frame pays; worst is what a hitch can be. A part that runs 190 times a second at
/// 3 us is 0.06% of a core, and a part that runs once a second at 2 ms is four dropped frames at
/// 60 Hz — the two need different fixes, which is the whole reason for this.
pub fn report() -> String {
    let mut out = String::new();
    for slot in 0..N {
        let calls = CALLS[slot].load(Ordering::Relaxed);
        if calls == 0 {
            continue;
        }
        let sum = SUM_US[slot].load(Ordering::Relaxed);
        let max = MAX_US[slot].load(Ordering::Relaxed);
        let mean = sum as f64 / calls as f64;
        let slow = SLOW[slot].load(Ordering::Relaxed);
        out.push_str(&format!(
            "  {}: {} calls, mean {:.1} us, worst {} us, {} over {} us\n",
            NAMES[slot], calls, mean, max, slow, SLOW_US
        ));
    }
    let idle_reads = IDLE_READS.load(Ordering::Relaxed);
    let idle_calls = CALLS[IDLE].load(Ordering::Relaxed);
    out.push_str(&format!(
        "  idle passes that touched game memory: {} of {}\n",
        idle_reads, idle_calls
    ));
    out
}
