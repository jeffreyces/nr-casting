//! Getting the panel's pixels onto the screen without a DXGI swap chain.
//!
//! The panel used to own a swap chain and `Present` into it, which is the ordinary way to draw a
//! window and the wrong thing to do inside a game that already has one. Measured against ERD Tools
//! (a third-party overlay that hooks the present path): with our device and no swap chain the game
//! ran, with a swap chain that is never presented it also ran, and with a single `Present` it died
//! inside ERD's own per-frame code. So the requirement is exact — the panel may not call `Present`.
//!
//! What is left is the oldest present there is: render into a texture, copy it to a staging
//! texture, copy the pixels into a DIB section, and `BitBlt` that into the window. It costs one
//! GPU-to-CPU copy per drawn frame and nothing at all when the panel is not drawn, and it puts no
//! swap chain in the process for anybody else's hook to trip over.
//!
//! Everything here is `unsafe` Win32/D3D11 and can only be exercised in the game; the part that can
//! be wrong invisibly — copying rows between two different pitches — lives in the pure crate as
//! `copy_rows`, with tests.

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET,
    D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, SRCCOPY,
};

/// A texture the UI renders into, and the DIB it travels to the window through.
pub(crate) struct Surface {
    /// Render target for ImGui. Kept so the render-target view can be rebuilt after a resize.
    texture: ID3D11Texture2D,
    /// CPU-readable copy; `CopyResource` into it is the one GPU stall in the path.
    staging: ID3D11Texture2D,
    /// The DIB section and the memory DC that has it selected, i.e. where `BitBlt` reads from.
    dib: HBITMAP,
    dc: HDC,
    /// The object the memory DC had selected before the DIB; restored on drop so neither leaks.
    previous: HGDIOBJ,
    /// The DIB's pixels. Owned by `dib`, valid until it is deleted.
    bits: *mut u8,
    size: (u32, u32),
    /// Bytes in a DIB row: 32-bit pixels are always 4-byte aligned, so this is `width * 4`.
    row_bytes: usize,
}

impl Surface {
    /// Build a surface of `width` x `height`, and the DIB that mirrors it.
    pub(crate) unsafe fn new(
        device: &ID3D11Device,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        let (width, height) = (width.max(1), height.max(1));
        let texture = create_texture(device, width, height, false)?;
        let staging = create_texture(device, width, height, true)?;

        let row_bytes = width as usize * 4;
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                // Negative height asks for a top-down DIB: the rows then run the same way as a
                // texture's, and the image is not flipped on its way to the screen.
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
            .map_err(|e| format!("CreateDIBSection: {e}"))?;
        // SAFETY: a fresh DIB always comes with a pixel buffer.
        let bits = bits as *mut u8;
        if bits.is_null() {
            let _ = DeleteObject(HGDIOBJ(dib.0));
            return Err("CreateDIBSection returned no pixels".to_string());
        }
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(dib.0));
            return Err("CreateCompatibleDC failed".to_string());
        }
        let previous = windows::Win32::Graphics::Gdi::SelectObject(dc, HGDIOBJ(dib.0));
        if previous.is_invalid() {
            let _ = DeleteDC(dc);
            let _ = DeleteObject(HGDIOBJ(dib.0));
            return Err("SelectObject(DIB) failed".to_string());
        }
        Ok(Self {
            texture,
            staging,
            dib,
            dc,
            previous,
            bits,
            size: (width, height),
            row_bytes,
        })
    }

    pub(crate) fn texture(&self) -> &ID3D11Texture2D {
        &self.texture
    }

    pub(crate) fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Copy the rendered texture to the window. One `BitBlt`, no `Present`.
    ///
    /// Returns the whole present's milliseconds - `CopyResource`, the map, the row copy and the
    /// blit - because the part that scales with panel size is the sum, and reporting only the
    /// middle would leave the two copies he pays for unmeasured.
    pub(crate) unsafe fn present(
        &self,
        context: &ID3D11DeviceContext,
        window: HWND,
    ) -> Result<u128, String> {
        let began = std::time::Instant::now();
        context.CopyResource(&self.staging, &self.texture);
        let mut mapped = Default::default();
        context
            .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(|e| format!("Map(staging): {e}"))?;
        let (width, height) = self.size;
        let row_bytes = self.row_bytes;
        let src_pitch = mapped.RowPitch as usize;
        let src_len = src_pitch * height as usize;
        // SAFETY: `Map` guarantees `RowPitch * height` readable bytes at `pData` for this texture.
        let src = std::slice::from_raw_parts(mapped.pData as *const u8, src_len);
        let dst_len = row_bytes * height as usize;
        // SAFETY: the DIB has exactly `row_bytes * height` bytes and is only written here.
        let dst = std::slice::from_raw_parts_mut(self.bits, dst_len);
        let copied =
            nr_casting_core::copy_rows(src, src_pitch, dst, row_bytes, height as usize, row_bytes);
        context.Unmap(&self.staging, 0);
        if copied.is_none() {
            return Err(format!(
                "copy_rows refused {width}x{height}: pitch {src_pitch}, row {row_bytes}"
            ));
        }

        let target = GetDC(Some(window));
        if target.is_invalid() {
            return Err("GetDC(window) failed".to_string());
        }
        let blitted = BitBlt(
            target,
            0,
            0,
            width as i32,
            height as i32,
            Some(self.dc),
            0,
            0,
            SRCCOPY,
        );
        let _ = ReleaseDC(Some(window), target);
        blitted.map_err(|e| format!("BitBlt: {e}"))?;
        Ok(began.elapsed().as_millis())
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        // Put the DC back the way it was, then delete the bitmap, then the DC. A bitmap that is
        // deleted while still selected is not freed - GDI keeps it and reports success, so the leak
        // is invisible until it matters.
        unsafe {
            windows::Win32::Graphics::Gdi::SelectObject(self.dc, self.previous);
            let _ = DeleteObject(HGDIOBJ(self.dib.0));
            let _ = DeleteDC(self.dc);
        }
    }
}

/// A texture in the format the DIB expects: BGRA8, render-target and shader-readable.
unsafe fn create_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    staging: bool,
) -> Result<ID3D11Texture2D, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: if staging {
            D3D11_USAGE_STAGING
        } else {
            D3D11_USAGE_DEFAULT
        },
        BindFlags: if staging {
            0
        } else {
            (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32
        },
        CPUAccessFlags: if staging {
            D3D11_CPU_ACCESS_READ.0 as u32
        } else {
            0
        },
        MiscFlags: 0,
    };
    let mut texture: Option<ID3D11Texture2D> = None;
    device
        .CreateTexture2D(&desc, None, Some(&mut texture))
        .map_err(|e| format!("CreateTexture2D: {e}"))?;
    texture.ok_or_else(|| "CreateTexture2D returned nothing".to_string())
}
