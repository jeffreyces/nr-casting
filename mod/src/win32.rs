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
}
