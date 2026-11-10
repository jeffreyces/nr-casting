//! How much CPU the game's own frame thread is using.
//!
//! A frame gap says a frame took 48 ms. It does not say whether that time was spent running the
//! game or waiting for the GPU - and that is the first question any stutter report has to answer,
//! because a render-side wrapper (ReShade, an overlay, a driver layer) and a game-logic stall look
//! identical in a frame-time graph.
//!
//! `GetThreadTimes` on the thread that runs the frame loop splits them: a second in which the thread
//! consumed 4 ms of CPU and still only produced 21 frames was waiting; a second in which it consumed
//! 45 ms was working. One syscall a second, on the thread whose timer already runs.

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// The thread whose `PeekMessageW` we saw: the game's frame loop.
static RENDER_TID: AtomicU32 = AtomicU32::new(0);
/// A handle to it, opened once and kept. `usize` so it can live in an atomic.
static HANDLE: AtomicUsize = AtomicUsize::new(0);
static HANDLE_TID: AtomicU32 = AtomicU32::new(0);
/// Cumulative kernel + user time at the previous sample, in microseconds.
static LAST_US: AtomicU64 = AtomicU64::new(0);

const THREAD_QUERY_LIMITED_INFORMATION: u32 = 0x0800;
const THREAD_QUERY_INFORMATION: u32 = 0x0040;

/// Remember which thread runs the frame loop. Called from the pump hook, which runs there.
pub fn note_frame_thread(tid: u32) {
    RENDER_TID.store(tid, Ordering::Relaxed);
}

/// Whether a frame thread has been seen at all.
pub fn watching() -> bool {
    RENDER_TID.load(Ordering::Relaxed) != 0
}

/// Microseconds of CPU the frame thread has used since the previous call.
///
/// `None` when there is no thread to watch or the handle cannot be opened - a report says "no
/// data" rather than printing a zero that would read as an idle thread.
pub fn cpu_since_last() -> Option<u64> {
    let tid = RENDER_TID.load(Ordering::Relaxed);
    if tid == 0 {
        return None;
    }
    let handle = handle_for(tid)?;
    let (kernel, user) = unsafe { thread_times(handle)? };
    let total = kernel.saturating_add(user);
    let previous = LAST_US.swap(total, Ordering::Relaxed);
    // The first sample has nothing to subtract: it is a lifetime total, not a second's worth.
    if previous == 0 || total < previous {
        return None;
    }
    Some(total - previous)
}

/// Cumulative CPU time of the thread, in microseconds.
unsafe fn thread_times(handle: usize) -> Option<(u64, u64)> {
    let mut creation = FileTime::default();
    let mut exit = FileTime::default();
    let mut kernel = FileTime::default();
    let mut user = FileTime::default();
    let ok = crate::win32::GetThreadTimes(
        handle as *mut u8,
        &mut creation.0,
        &mut exit.0,
        &mut kernel.0,
        &mut user.0,
    );
    if ok == 0 {
        return None;
    }
    // FILETIME counts 100 ns units; two u32 words per value.
    Some((kernel.0 / 10, user.0 / 10))
}

/// The thread handle, opened once per thread id.
fn handle_for(tid: u32) -> Option<usize> {
    let cached = HANDLE.load(Ordering::Acquire);
    if cached != 0 && HANDLE_TID.load(Ordering::Relaxed) == tid {
        return Some(cached);
    }
    let mut handle = unsafe {
        crate::win32::OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, tid)
    };
    if handle.is_null() {
        // Older Windows and some protected threads want the older access right.
        handle = unsafe { crate::win32::OpenThread(THREAD_QUERY_INFORMATION, 0, tid) };
    }
    if handle.is_null() {
        return None;
    }
    HANDLE.store(handle as usize, Ordering::Release);
    HANDLE_TID.store(tid, Ordering::Relaxed);
    // A new thread means the lifetime total restarts, so the first delta is meaningless.
    LAST_US.store(0, Ordering::Relaxed);
    Some(handle as usize)
}

/// One FILETIME, as the two words the API writes.
#[repr(C)]
#[derive(Default)]
struct FileTime(u64);
