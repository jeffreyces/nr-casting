//! The shell: the browser and the file manager.
//!
//! Small and entirely separable from the panel - none of these touch `App` - but they are the part
//! of the diagnostics flow that fails quietly, so they live together where their error handling can
//! be read in one place.
//!
//! There is deliberately no clipboard here. Copying a path is one button's worth of convenience and
//! it costs `OpenClipboard`/`SetClipboardData` in the import table, which is half of the
//! stealer-signature cluster a scanner reads. The path is shown in the note instead. Every call reports whether the shell accepted it: a refused
//! `ShellExecuteW` returns a code at or below 32, and discarding it is what made a dead button look
//! like a working one.

use windows::core::HSTRING;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Open the folder that holds a file, so the player can drag it into a chat window.
///
/// This used to launch `explorer.exe /select,<path>`, which is a named system binary spawned by
/// the shell verb — the shape a "system binary proxy execution" heuristic looks for, not the shape
/// a mod that writes a report has. Opening the directory itself reaches the same file manager
/// through the shell's own association and names no executable at all. The file is no longer
/// pre-selected; the folder is.
pub(crate) fn reveal(path: &std::path::Path) -> bool {
    let verb = HSTRING::from("open");
    let folder = path.parent().unwrap_or(path);
    let target = HSTRING::from(folder.display().to_string());
    let result = unsafe { ShellExecuteW(None, &verb, &target, None, None, SW_SHOWNORMAL) };
    log_shell_failure("file manager", result.0 as isize);
    result.0 as isize > 32
}

/// A `ShellExecuteW` result above 32 is a success; anything else is an error code.
///
/// The failure used to be discarded, so a shell that refused (no file manager, a locked-down
/// desktop) looked exactly like one that opened — the same silence that made the folder stop
/// opening after the first press. Now it is a log line naming the code.
fn log_shell_failure(what: &str, result: isize) {
    if result <= 32 {
        crate::log(&format!(
            "panel: could not open the {what} (shell error {result})"
        ));
    }
}

/// Open a URL in the player's browser.
pub(crate) fn open_url(url: &str) -> bool {
    let verb = HSTRING::from("open");
    let target = HSTRING::from(url);
    let result = unsafe { ShellExecuteW(None, &verb, &target, None, None, SW_SHOWNORMAL) };
    log_shell_failure("issue page", result.0 as isize);
    result.0 as isize > 32
}
