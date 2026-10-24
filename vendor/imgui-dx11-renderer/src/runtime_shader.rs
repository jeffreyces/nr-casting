//! Compile the renderer's HLSL at runtime instead of at build time.
//!
//! Upstream compiles both shaders in a `build.rs` that calls into `fxc`, which makes the crate
//! unbuildable anywhere the Windows SDK shader compiler is absent (a cross-build on Linux, for
//! one). The official `imgui_impl_dx11.cpp` takes the other approach: it calls `D3DCompile` from
//! `d3dcompiler_47.dll`, which ships with Windows and with Wine/Proton. This module does the
//! same, and resolves the entry point from the DLL so nothing is linked against it.

use core::ffi::c_void;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use windows::core::{PCSTR, PCWSTR};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

type D3DCompileFn = unsafe extern "system" fn(
    src_data: *const c_void,
    src_data_size: usize,
    source_name: PCSTR,
    defines: *const c_void,
    include: *const c_void,
    entry_point: PCSTR,
    target: PCSTR,
    flags1: u32,
    flags2: u32,
    code: *mut Option<ID3DBlob>,
    error_msgs: *mut Option<ID3DBlob>,
) -> i32;

const DLL: &str = "d3dcompiler_47.dll\0";

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(core::iter::once(0)).collect()
}

/// Compile `source` for `target` (e.g. `vs_4_0`). Returns the compiled bytecode blob.
pub unsafe fn compile(source: &str, target: &str) -> Result<ID3DBlob, String> {
    let dll_name = wide(DLL.trim_end_matches('\0'));
    let module: HMODULE = LoadLibraryW(PCWSTR(dll_name.as_ptr()))
        .map_err(|e| format!("d3dcompiler_47.dll not available: {e}"))?;
    let procedure = GetProcAddress(module, PCSTR(b"D3DCompile\0".as_ptr()))
        .ok_or_else(|| "d3dcompiler_47.dll has no D3DCompile".to_string())?;

    let compile: D3DCompileFn = core::mem::transmute(procedure);

    let entry = PCSTR(b"main\0".as_ptr());
    let target = PCSTR(
        [target.as_bytes(), b"\0"]
            .concat()
            .leak()
            .as_ptr(),
    );

    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    let result = compile(
        source.as_ptr() as *const c_void,
        source.len(),
        PCSTR(core::ptr::null()),
        core::ptr::null(),
        core::ptr::null(),
        entry,
        target,
        0,
        0,
        &mut code,
        &mut errors,
    );

    match (result, code) {
        (0, Some(code)) => Ok(code),
        (_, _) => {
            let message = errors
                .as_ref()
                .map(|blob| {
                    let bytes = core::slice::from_raw_parts(
                        blob.GetBufferPointer() as *const u8,
                        blob.GetBufferSize(),
                    );
                    String::from_utf8_lossy(bytes).into_owned()
                })
                .unwrap_or_else(|| format!("D3DCompile failed with {result:#x}"));
            Err(message)
        }
    }
}

/// Bytecode as a byte slice, valid while `blob` is alive.
pub unsafe fn bytecode(blob: &ID3DBlob) -> &[u8] {
    core::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize())
}
