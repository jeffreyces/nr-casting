//! A frame clock that is once per frame, because presenting is once per frame.
//!
//! Every other clock in this mod is a proxy. The pad poll is about twice a frame when a controller
//! is attached and absent when it is not; `PeekMessageW` is per frame on one machine and 24 times a
//! second on another (which turned a smooth session into "194 seconds over 32 ms" the moment the
//! pad hooks were switched off, because 24 ticks a second can only ever produce 42 ms gaps).
//!
//! `IDXGISwapChain::Present` is the definition of a frame, so it is the clock. The hook is
//! measurement only: it times the call, passes it through untouched, and changes no arguments, no
//! state and no return value. It is the one place this mod touches the graphics path, and it exists
//! because the alternative was measuring a metronome and calling it stutter.

use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDeviceAndSwapChain, ID3D11Device, ID3D11DeviceContext, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_MODE_DESC, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGISwapChain, DXGI_SWAP_CHAIN_DESC, DXGI_SWAP_EFFECT_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, WS_POPUP, WINDOW_EX_STYLE, WINDOW_STYLE,
};
use windows::core::{w, Interface};

static TRAMPOLINE: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// `IDXGISwapChain::Present` is the ninth entry of the interface: three `IUnknown` slots, five
/// accessors, then it. The vtable is per-interface but the functions behind it are shared, so one
/// detour on this address covers every swap chain in the process - including the game's.
const PRESENT_VTABLE_INDEX: usize = 8;

/// Install the pass-through `Present` timer.
///
/// The address has to be read from a live swap chain, so a throwaway device and swap chain are
/// created on a hidden window, the vtable entry is read, and both are released before anything is
/// patched: nothing is left resident by this.
pub fn install() -> Result<(), String> {
    if INSTALLED.load(Ordering::Relaxed) {
        return Ok(());
    }
    let present = unsafe { present_address()? };
    unsafe {
        let raw = retour::RawDetour::new(present, present_detour as *const ())
            .map_err(|e| format!("Present detour: {e:?}"))?;
        raw.enable().map_err(|e| format!("Present enable: {e:?}"))?;
        TRAMPOLINE.store(raw.trampoline() as *const () as *mut (), Ordering::Release);
        std::mem::forget(raw);
    }
    INSTALLED.store(true, Ordering::Relaxed);
    Ok(())
}

/// Read the address of `IDXGISwapChain::Present` from a throwaway swap chain.
unsafe fn present_address() -> Result<*const (), String> {
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        w!("STATIC"),
        w!("nr_casting clock"),
        WS_POPUP as WINDOW_STYLE,
        0,
        0,
        8,
        8,
        None,
        None,
        None,
        None,
    )
    .map_err(|e| format!("clock window: {e}"))?;

    let desc = DXGI_SWAP_CHAIN_DESC {
        BufferDesc: DXGI_MODE_DESC {
            Width: 8,
            Height: 8,
            RefreshRate: DXGI_RATIONAL { Numerator: 60, Denominator: 1 },
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            ScanlineOrdering: Default::default(),
            Scaling: Default::default(),
        },
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 1,
        OutputWindow: hwnd,
        Windowed: true.into(),
        SwapEffect: DXGI_SWAP_EFFECT_DISCARD,
        Flags: 0,
    };
    let levels = [windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0];
    let mut swap_chain: Option<IDXGISwapChain> = None;
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    let mut level = levels[0];

    let created = D3D11CreateDeviceAndSwapChain(
        None,
        D3D_DRIVER_TYPE_HARDWARE,
        Default::default(),
        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        Some(&levels),
        D3D11_SDK_VERSION,
        Some(&desc),
        Some(&mut swap_chain),
        Some(&mut device),
        Some(&mut level),
        Some(&mut context),
    );

    let result = match (created, swap_chain.as_ref()) {
        (Ok(()), Some(swap_chain)) => {
            let table = *(swap_chain.as_raw() as *const *const usize);
            Ok(*table.add(PRESENT_VTABLE_INDEX) as *const ())
        }
        (Err(e), _) => Err(format!("clock swap chain: {e}")),
        _ => Err("clock swap chain: no swap chain returned".to_string()),
    };
    // Release everything this needed: the address stays valid, the objects do not stay resident.
    drop(context);
    drop(device);
    drop(swap_chain);
    let _ = DestroyWindow(hwnd);
    result
}

/// Time one present and pass it through untouched.
unsafe extern "system" fn present_detour(this: *mut u8, sync_interval: u32, flags: u32) -> i32 {
    let started = crate::perf::ticks();
    let tramp = TRAMPOLINE.load(Ordering::Acquire);
    if tramp.is_null() {
        return 0;
    }
    let original: unsafe extern "system" fn(*mut u8, u32, u32) -> i32 = std::mem::transmute(tramp);
    let out = original(this, sync_interval, flags);
    // The panel's own presents are not the game's frames.
    if CRATE_THREAD.with(|ours| !ours.get()) {
        crate::perf::report_present(started);
    }
    out
}

thread_local! {
    /// Set while our own code is presenting (the panel's frame), so its presents are not counted.
    pub static CRATE_THREAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Mark the current call as ours: used by the panel around its own `Present`.
pub fn in_our_present(ours: bool) {
    CRATE_THREAD.with(|flag| flag.set(ours));
}

// Keeps the unused-import warnings honest if the panel never presents in this build.
#[allow(dead_code)]
fn _types(_: HWND, _: WPARAM, _: LPARAM) {}
