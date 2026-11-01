//! The handful of Win32 calls this crate declares for itself.
//!
//! They used to be declared per module - seventeen `extern "system"` blocks, with
//! `GetModuleHandleA` in three of them and `GetTickCount64` in two - which is how a signature
//! drifts away from the callers that depend on it.

#[link(name = "kernel32")]
extern "system" {
    pub fn GetModuleHandleA(name: *const u8) -> *mut u8;
    pub fn GetModuleFileNameW(module: *mut u8, buffer: *mut u16, size: u32) -> u32;
    pub fn GetCurrentProcess() -> *mut u8;
    pub fn K32EnumProcessModules(
        process: *mut u8,
        modules: *mut *mut u8,
        size: u32,
        needed: *mut u32,
    ) -> i32;
    pub fn GetCurrentThreadId() -> u32;
    pub fn GetTickCount64() -> u64;
    pub fn OpenThread(access: u32, inherit: i32, thread_id: u32) -> *mut u8;
    /// The four `FILETIME`s are 8-byte values the API writes in place; `*mut u64` matches them.
    pub fn GetThreadTimes(
        thread: *mut u8,
        creation: *mut u64,
        exit: *mut u64,
        kernel: *mut u64,
        user: *mut u64,
    ) -> i32;
}
