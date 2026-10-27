//! Counters for the input paths, so the log can answer "is the hook alive" and "which path is
//! carrying this cast" without guessing.
//!
//! These are diagnostics only: nothing here changes behaviour, and every counter is read by the
//! 30-second heartbeat behind `debug_log`. Keeping them in one place is what lets `input` and
//! `rawinput` stay about behaviour.

use std::sync::atomic::{AtomicU32, Ordering};

/// Pad polls that succeeded.
pub(crate) static PAD_POLLS: AtomicU32 = AtomicU32::new(0);
/// Full passes, for the heartbeat.
static PASSES: AtomicU32 = AtomicU32::new(0);
/// Pad polls the game made with no controller attached.
pub(crate) static PAD_UNPLUGGED: AtomicU32 = AtomicU32::new(0);
/// `GetRawInputData` calls that returned a record.
static RAW_CALLS: AtomicU32 = AtomicU32::new(0);
static RAW_BUFFER_CALLS: AtomicU32 = AtomicU32::new(0);
static MOUSE_RECORDS: AtomicU32 = AtomicU32::new(0);
static BUTTON_RECORDS: AtomicU32 = AtomicU32::new(0);
/// Casts steered from a raw mouse click, and the last slot announced.
static STEERED: AtomicU32 = AtomicU32::new(0);
static STEERED_SLOT: AtomicU32 = AtomicU32::new(u32::MAX);
/// Alt chords handed to the class port as its own key presses.
static HANDOFF: AtomicU32 = AtomicU32::new(0);
/// Binds made from a Super + click.
static BOUND: AtomicU32 = AtomicU32::new(0);
/// Shift presses rewritten as releases.
static SHIFT_HIDDEN: AtomicU32 = AtomicU32::new(0);
/// Key-state queries from the game, and how many were for the mouse buttons (the question the
/// raw-input path exists to answer).
static KEY_POLLS: AtomicU32 = AtomicU32::new(0);
static KEY_LMB: AtomicU32 = AtomicU32::new(0);
static KEY_RMB: AtomicU32 = AtomicU32::new(0);

/// Count one successful pad poll and return its running number.
pub(crate) fn pad_ok() -> u32 {
    PAD_POLLS.fetch_add(1, Ordering::Relaxed) + 1
}

/// Pad polls since the mod loaded.
pub(crate) fn pad_polls() -> u32 {
    PAD_POLLS.load(Ordering::Relaxed)
}

/// Polls the game made with no controller attached.
pub(crate) fn pad_unplugged_count() -> u32 {
    PAD_UNPLUGGED.load(Ordering::Relaxed)
}

/// One full pass over the report (not a skipped repeat).
pub(crate) fn pass() {
    PASSES.fetch_add(1, Ordering::Relaxed);
}

/// Full passes since the mod loaded.
pub(crate) fn passes() -> u32 {
    PASSES.load(Ordering::Relaxed)
}

pub(crate) fn pad_unplugged() {
    PAD_UNPLUGGED.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn raw_call() {
    RAW_CALLS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn raw_buffer_call() {
    RAW_BUFFER_CALLS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn mouse_record() {
    MOUSE_RECORDS.fetch_add(1, Ordering::Relaxed);
}

/// Record a mouse record whose button field says something.
pub(crate) fn mouse_buttons(buttons: u32) {
    if buttons & 0x0F != 0 {
        BUTTON_RECORDS.fetch_add(1, Ordering::Relaxed);
    }
}

/// A click was steered onto a slot. Announced once per slot change, not per click.
pub(crate) fn steered(which: &str, slot: usize) {
    STEERED.fetch_add(1, Ordering::Relaxed);
    let key = slot as u32;
    if STEERED_SLOT.swap(key, Ordering::Relaxed) != key {
        crate::log(&format!("mouse: {which} -> slot {slot}"));
    }
}

pub(crate) fn handoff() {
    HANDOFF.fetch_add(1, Ordering::Relaxed);
}

/// A Super + click bound the game's selected spell to a slot.
pub(crate) fn bound(which: &str) {
    let count = BOUND.fetch_add(1, Ordering::Relaxed) + 1;
    if count <= 8 || count.is_multiple_of(32) {
        crate::log(&format!("mouse: super + click bound {which} ({count} so far)"));
    }
}

pub(crate) fn shift_hidden() {
    SHIFT_HIDDEN.fetch_add(1, Ordering::Relaxed);
}

/// A key-state query from the game.
pub(crate) fn key_poll(vk: i32) {
    KEY_POLLS.fetch_add(1, Ordering::Relaxed);
    match vk {
        0x01 => KEY_LMB.fetch_add(1, Ordering::Relaxed),
        0x02 => KEY_RMB.fetch_add(1, Ordering::Relaxed),
        _ => 0,
    };
}

/// One heartbeat line for the keyboard/mouse path, drained so each report covers one window.
pub(crate) fn raw_report() -> String {
    format!(
        "{} key polls (LMB {}, RMB {}); raw input: {} calls, {} buffer, {} mouse records, \
         {} with buttons; {} casts steered, {} handed to Recluse, {} bound by super + click, \
         {} shift presses hidden",
        KEY_POLLS.swap(0, Ordering::Relaxed),
        KEY_LMB.swap(0, Ordering::Relaxed),
        KEY_RMB.swap(0, Ordering::Relaxed),
        RAW_CALLS.swap(0, Ordering::Relaxed),
        RAW_BUFFER_CALLS.swap(0, Ordering::Relaxed),
        MOUSE_RECORDS.swap(0, Ordering::Relaxed),
        BUTTON_RECORDS.swap(0, Ordering::Relaxed),
        STEERED.swap(0, Ordering::Relaxed),
        HANDOFF.swap(0, Ordering::Relaxed),
        BOUND.swap(0, Ordering::Relaxed),
        SHIFT_HIDDEN.swap(0, Ordering::Relaxed),
    )
}
