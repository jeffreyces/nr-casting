//! The shell: the browser, the file manager, and the clipboard.
//!
//! Small and entirely separable from the panel - none of these touch `App` - but they are the part
//! of the diagnostics flow that fails quietly, so they live together where their error handling can
//! be read in one place. Every call reports whether the shell accepted it: a refused
//! `ShellExecuteW` returns a code at or below 32, and discarding it is what made a dead button look
//! like a working one.

use windows::core::HSTRING;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Put text on the clipboard.
///
/// The report's file name is long and the file manager opens at whatever size it last had, so the
/// selected name is often cut off. Pasting the full path into a file dialog's name field opens that
/// exact file whatever the list is showing, which is faster than reading a truncated name.
pub(crate) fn copy_to_clipboard(text: &str) {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    const CF_UNICODETEXT: u32 = 13;
    unsafe {
        if OpenClipboard(None).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        if let Ok(handle) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) {
            let memory = GlobalLock(handle);
            if !memory.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), memory as *mut u16, wide.len());
                let _ = GlobalUnlock(handle);
                // On success the clipboard owns the block; on failure it is left for the process
                // to reclaim at exit, which is cheaper than a `GlobalFree` binding that this
                // windows-rs version does not expose.
                let _ = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(handle.0)));
            }
        }
        let _ = CloseClipboard();
    }
}

/// Show a file in the file manager, selected, so it can be dragged straight into a chat window.
pub(crate) fn reveal(path: &std::path::Path) -> bool {
    let verb = HSTRING::from("open");
    let program = HSTRING::from("explorer.exe");
    let arguments = HSTRING::from(format!("/select,\"{}\"", path.display()));
    let result = unsafe {
        ShellExecuteW(
            None,
            &verb,
            &program,
            &arguments,
            None,
            SW_SHOWNORMAL,
        )
    };
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
        crate::log(&format!("panel: could not open the {what} (shell error {result})"));
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
