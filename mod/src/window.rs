//! The assignment panel: Dear ImGui, rendered with D3D11 into a window of our own.
//!
//! Four constraints, each learned from a failure:
//!
//! 1. **No hook on the game's swap chain.** A second Present hook made a co-resident class
//!    port's HUD stop drawing (`ERNativeUI.dll`), and that survived every state-restoration fix
//!    inside the hook — the hook's mere presence was enough, even with an empty draw list.
//! 2. **No winit.** Hosting a winit event loop in this process killed the game the moment the
//!    loop started. A plain Win32 window with our own message pump survives.
//! 3. **No OpenGL and no Vulkan.** A WGL context kills the game, and so does a wgpu/Vulkan device
//!    on first use. D3D11 (DXVK) is the only GPU API that survived in-process here.
//! 4. **No panic may escape the window procedure.** Unwinding across an `extern "system"`
//!    boundary aborts the process regardless of `panic = "unwind"`, so the procedure catches.
//!
//! The device is created on the first open, never during play: it is a second graphics device in
//! a process that already runs one, and creating it mid-game hitches.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use imgui::{Condition, Ui};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1,
    D3D_FEATURE_LEVEL_11_0,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDeviceAndSwapChain, ID3D11DepthStencilView, ID3D11Device, ID3D11DeviceContext,
    ID3D11RenderTargetView, ID3D11Texture2D, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_MODE_DESC, DXGI_RATIONAL,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIAdapter, IDXGISwapChain, DXGI_SWAP_CHAIN_DESC, DXGI_SWAP_EFFECT_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, LoadCursorW, IDC_ARROW, WS_EX_TOPMOST, WS_EX_TOOLWINDOW, WS_POPUP, DispatchMessageW, GetClientRect, IsIconic, PeekMessageW,
    GetForegroundWindow, RegisterClassW, SetForegroundWindow, ShowWindow, TranslateMessage, MSG,
    PM_REMOVE, SW_HIDE, SW_SHOW,
    WNDCLASSW, WM_CHAR, WM_CLOSE, WM_GETMINMAXINFO, WM_KEYDOWN, WM_KEYUP,
    WM_ACTIVATE, WM_SYSKEYDOWN, WM_SYSKEYUP,
    WS_THICKFRAME, MINMAXINFO,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN,
    WM_RBUTTONUP, WM_SIZE, GetCursorPos, GetWindowRect, SetWindowPos, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WM_EXITSIZEMOVE, GetSystemMetrics, SM_CXSCREEN,
    SM_CYSCREEN,
};

use crate::overlay::Panel;

/// Height of the draggable header band, in physical pixels.
const TITLE_BAND: i16 = 28;
/// Virtual key of the left mouse button, for the drag's own button-state poll.
const VK_LBUTTON: i32 = 0x01;

/// The size the panel was laid out at, and the font height it was laid out with.
///
/// Every UI scale is relative to these: the window opens at `DESIGN * monitor scale`, the font is
/// baked at `BASE_FONT_PX * scale`, and ImGui's paddings are scaled by the same factor. On a 4K
/// screen that is a 1800x1240 window with 30 px text, instead of a 900x620 window whose widgets
/// stay 15 px however far you drag the corner.
const DESIGN_WIDTH: f32 = 900.0;
const DESIGN_HEIGHT: f32 = 620.0;
const BASE_FONT_PX: f32 = 15.0;
/// 1.0 is the size the panel was designed at; below it nothing is legible and above 3x the three
/// columns no longer fit a 4K screen.
const MIN_UI_SCALE: f32 = 1.0;
const MAX_UI_SCALE: f32 = 3.0;
/// The scales offered in the panel, as multipliers. Auto is offered separately.
const UI_SCALE_CHOICES: [f32; 5] = [1.0, 1.25, 1.5, 1.75, 2.0];

/// The scale to use when nothing has been chosen: the screen's height over 1080.
///
/// The panel is its own window, so this is the desktop's own resolution — under Proton that is the
/// virtual desktop, which is what the drawn window is measured against anyway.
fn monitor_scale() -> f32 {
    let height = unsafe { GetSystemMetrics(SM_CYSCREEN) } as f32;
    if height <= 0.0 {
        return 1.0;
    }
    (height / 1080.0).clamp(MIN_UI_SCALE, MAX_UI_SCALE)
}

extern "system" {
    fn GetAsyncKeyState(vk: i32) -> i16;
}

/// One assignable slot: which hand's weapon, and which of its two spells.
#[derive(Clone, Copy)]
struct Slot {
    label: &'static str,
    /// Same slot, same rules — the mouse binding is shown beside the pad one so the panel says
    /// both at the point of use rather than only in the docs.
    mouse: &'static str,
    weapon_slot: usize,
    right_hand: bool,
}

const SLOTS: [Slot; 8] = [
    Slot { label: "L1", mouse: "RMB", weapon_slot: 0, right_hand: false },
    Slot { label: "L2", mouse: "Shift+RMB", weapon_slot: 1, right_hand: false },
    Slot { label: "Y+L1", mouse: "Alt+RMB", weapon_slot: 3, right_hand: false },
    Slot { label: "Y+L2", mouse: "Ctrl+RMB", weapon_slot: 2, right_hand: false },
    Slot { label: "R1", mouse: "LMB", weapon_slot: 0, right_hand: true },
    Slot { label: "R2", mouse: "Shift+LMB", weapon_slot: 1, right_hand: true },
    Slot { label: "Y+R1", mouse: "Alt+LMB", weapon_slot: 3, right_hand: true },
    Slot { label: "Y+R2", mouse: "Ctrl+LMB", weapon_slot: 2, right_hand: true },
];

/// Everything the panel shows, refreshed when it opens.
struct Model {
    panel: Panel,
    weapons: (i32, i32),
    search: String,
    show_all: bool,
    /// The loaded character's name — the key the assignments are filed under.
    who: String,
    /// INT/FAI/ARC as the game applies them.
    stats: String,
    /// The note that a rune or talisman is doing the work, empty when the base stats are what is
    /// being used.
    boost: String,
    /// How many spells the pickers are offering, against the whole table.
    listed: String,
    /// Rebuilt only when the filter or the character's stats change. The panel redraws every
    /// frame, and this list is several hundred formatted strings.
    candidates: Rc<Vec<(i32, String)>>,
    candidates_key: String,
}

impl Model {
    fn refresh_candidates(&mut self) {
        // Effective stats: base plus active effects. Keying the cache on them means a Great
        // Rune going on (or a talisman swap) re-filters the list instead of leaving it stale.
        let stats = crate::player_effective_stats().unwrap_or((0, 0, 0));
        let (int, fai, arc) = stats;
        let key = format!("{}\u{1}{}\u{1}{int}\u{1}{fai}\u{1}{arc}", self.search, self.show_all);
        if key == self.candidates_key {
            return;
        }
        self.candidates = Rc::new(self.panel.candidates(&self.search));
        self.candidates_key = key;
    }

    fn new() -> Self {
        Model {
            panel: Panel::new(),
            weapons: (0, 0),
            search: String::new(),
            show_all: crate::config::load().show_all,
            who: "waiting for the player...".to_string(),
            stats: String::new(),
            boost: String::new(),
            listed: String::new(),
            candidates: Rc::new(Vec::new()),
            candidates_key: String::new(),
        }
    }

    fn reload(&mut self) {
        self.panel.reload();
        self.candidates_key.clear();
        self.weapons = crate::weapon_ids().unwrap_or((0, 0));
        self.panel.set_filter(self.search.clone());
        self.panel.set_show_all(self.show_all);
        let listed = self.panel.candidates(&self.search).len();
        let (stats, boost) = match crate::player_effective_stats() {
            Some((int, fai, arc)) => {
                // Say when a rune or talisman is doing the work, so a requirement the base stats
                // do not meet does not read as a bug.
                let boosted = crate::player_stats().is_some_and(|base| base != (int, fai, arc));
                (
                    format!("INT {int}   FAI {fai}   ARC {arc}"),
                    if boosted { "incl. rune/talisman".to_string() } else { String::new() },
                )
            }
            None => ("stats unavailable".to_string(), String::new()),
        };
        // The character name is the key the assignments are filed under, so showing it says
        // which file the panel is editing.
        self.who = crate::character_name().unwrap_or_else(|| "no character".to_string());
        self.stats = stats;
        self.boost = boost;
        self.listed = format!("{listed}/{} spells listed", self.panel.spell_count());
    }

    fn weapon_of(&self, slot: Slot) -> i32 {
        if slot.right_hand {
            self.weapons.1
        } else {
            self.weapons.0
        }
    }
}

/// Input as the window procedure sees it, consumed when a frame is built.
#[derive(Default)]
struct Input {
    mouse: [f32; 2],
    down: [bool; 5],
    wheel: f32,
    chars: Vec<char>,
    keys: Vec<(usize, bool)>,
}

/// Device, swap chain and ImGui state. Created on the first open, never during play.
struct Gfx {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    swap_chain: IDXGISwapChain,
    view: Option<ID3D11RenderTargetView>,
    size: (u32, u32),
    /// ImGui context and renderer for the current scale.
    ///
    /// `None` only while a rescale is in flight (or after one failed): imgui-rs keeps a single
    /// current context per process, so the old one has to be dropped before the new one can be
    /// created — the rebuild briefly has neither.
    ui: Option<PanelUi>,
    last_frame: Instant,
    presented: bool,
    /// The multiplier the current context and font atlas were built at.
    scale: f32,
}

/// The ImGui context and its DX11 renderer, which are rebuilt together at a new scale.
struct PanelUi {
    imgui: imgui::Context,
    renderer: imgui_dx11_renderer::Renderer,
}

/// Build an ImGui context and its DX11 renderer at `scale`.
///
/// The font atlas is rasterised and uploaded inside the renderer's constructor, so a different size
/// means a new context and a new renderer — not a new font size on the old ones. The caller must
/// drop the previous context first: `Context::create` panics while another context exists.
unsafe fn build_ui(device: &ID3D11Device, scale: f32) -> Result<PanelUi, String> {
    let mut imgui = imgui::Context::create();
    imgui.set_ini_filename(None);
    // imgui-rs 0.8 leaves `io.key_map` at Dear ImGui's default of -1, "unmapped", so every key
    // shortcut was dead while typed characters arrived through `add_input_character`: Backspace,
    // the arrow keys, Enter, Home/End and Ctrl+A/C/V all read `keys_down[-1]`. The map is the
    // identity here — our `keys_down` indices are the `ImGuiKey` values themselves.
    {
        let io = imgui.io_mut();
        for key in imgui::Key::VARIANTS {
            io.key_map[key as usize] = key as u32;
        }
    }
    imgui.fonts().add_font(&[imgui::FontSource::DefaultFontData {
        config: Some(imgui::FontConfig {
            size_pixels: BASE_FONT_PX * scale,
            ..Default::default()
        }),
    }]);
    // A fresh context carries the default style, so this scales exactly once however often the
    // panel is rescaled.
    imgui.style_mut().scale_all_sizes(scale);
    imgui.style_mut().window_rounding = 4.0 * scale;
    imgui.style_mut().frame_rounding = 3.0 * scale;
    let renderer = imgui_dx11_renderer::Renderer::new(&mut imgui, device)
        .map_err(|e| format!("ImGui D3D11 renderer failed: {e:?}"))?;
    Ok(PanelUi { imgui, renderer })
}

impl Gfx {
    unsafe fn rebuild_view(&mut self) {
        let buffer: ID3D11Texture2D = match self.swap_chain.GetBuffer(0) {
            Ok(buffer) => buffer,
            Err(e) => {
                crate::log(&format!("panel: no back buffer: {e}"));
                return;
            }
        };
        // Drop the old view before replacing it: `ResizeBuffers` calls this every time, and an
        // overwritten COM pointer is a leaked render target, not a freed one.
        self.view = None;
        let mut view: Option<ID3D11RenderTargetView> = None;
        match self.device.CreateRenderTargetView(&buffer, None, Some(&mut view)) {
            Ok(()) => self.view = view,
            Err(e) => crate::log(&format!("panel: no render target: {e}")),
        }
    }
}

/// A drag in progress: where the pointer was when it started, and where the window was.
///
/// The header band used to return `HTCAPTION` and let `DefWindowProcW` run its modal move loop.
/// That cannot work here: the mod swallows `SetCapture` and releases the capture every frame
/// while the panel is open (both needed to keep the pointer away from the game), so the loop
/// never tracks the mouse and the panel cannot be moved at all. Dragging it by hand touches
/// neither.
#[derive(Clone, Copy)]
struct Drag {
    /// Screen coordinates of the pointer at the start.
    from: [i32; 2],
    /// Window origin at the start.
    origin: [i32; 2],
}

struct App {
    hwnd: HWND,
    /// Whatever had focus when the panel opened. Hiding our window does not give focus back, and
    /// a hidden owner of the keyboard makes the game ignore Esc and every other key, so the
    /// game window is restored explicitly on close.
    previous_foreground: Option<HWND>,
    model: Model,
    input: Input,
    gfx: Option<Gfx>,
    shown: bool,
    /// Set while the player is dragging the panel by its header band.
    drag: Option<Drag>,
    hotkey: i32,
    hotkey_down: bool,
    /// A physical press must produce exactly one toggle: the poll runs every 16 ms and the
    /// window can also see the key, so ignore anything closer than this.
    last_toggle: Option<Instant>,
    /// The scale the player picked, `None` = follow the window's size. Mirrors `config::ui_scale`;
    /// the ini copy only takes effect on the next start.
    ui_scale: Option<f32>,
    /// A scale waiting to be applied between frames: the font atlas is rebuilt, which cannot
    /// happen while a frame is being built.
    pending_scale: Option<f32>,
    /// A window resize waiting for the frame loop, which can run it outside the `App` borrow.
    pending_resize: Option<f32>,
    /// The panel size last written to the ini, so a move/resize drag that changes nothing does not
    /// rewrite the file every time the window is nudged.
    saved_size: (u32, u32),
    /// When the UI was last rebuilt at a new scale: the rebuild is not free, so a live resize
    /// follows the window at a bounded rate instead of once per drag message.
    last_scale: Instant,
    /// When the window size last changed without being saved. The write happens once the player
    /// has stopped resizing, not on every drag.
    pending_save: Option<Instant>,
}

/// How often a live resize may rebuild the UI. A drag delivers a `WM_SIZE` per pixel; rebuilding
/// the font atlas that often would stutter, and 8 a second is already smooth to the eye.
const SCALE_REBUILD_MS: u128 = 120;
/// How long the player has to stop resizing before the size is written to the ini.
const SAVE_QUIET: std::time::Duration = std::time::Duration::from_secs(2);

thread_local! {
    // The window procedure and the frame loop run on this same thread.
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

impl App {
    fn new(hwnd: HWND) -> Self {
        App {
            hwnd,
            model: Model::new(),
            input: Input::default(),
            gfx: None,
            shown: false,
            drag: None,
            hotkey: crate::config::load().hotkey,
            hotkey_down: false,
            last_toggle: None,
            previous_foreground: None,
            ui_scale: crate::config::load().ui_scale,
            pending_scale: None,
            pending_resize: None,
            // The size the window actually opened at, not what the file asked for: a saved size
            // that had to be fitted to a smaller screen must not be rewritten just because the
            // player moved the panel.
            saved_size: window_size(hwnd).unwrap_or((0, 0)),
            last_scale: Instant::now(),
            pending_save: None,
        }
    }

    /// Follow the window's size while the player is still dragging.
    ///
    /// A live resize used to wait for the drag to end, so the layout stayed at the old scale the
    /// whole time and the panel looked like it refused to scale. The rebuild is throttled because
    /// each one re-rasterises the font atlas and recompiles the renderer's shaders.
    fn follow_window_scale(&mut self) {
        let Some(scale) = self.gfx.as_ref().map(|gfx| gfx.scale) else {
            return;
        };
        let target = self.target_scale();
        if (scale - target).abs() < 0.02 {
            return;
        }
        if self.last_scale.elapsed().as_millis() < SCALE_REBUILD_MS {
            return;
        }
        self.pending_scale = Some(target);
    }

    /// Note that the window size changed; the ini write waits for the player to stop.
    ///
    /// A drag calls this on every `WM_SIZE`, and a slow deliberate drag can easily span several
    /// seconds, so the timer restarts each time and one write lands at the end of the drag — not
    /// one per pixel, and not one per drag when the player resizes twice in a row.
    fn note_window_size(&mut self) {
        // A pinned scale decides the size at open, so the window's own size is not the setting.
        if self.ui_scale.is_some() {
            return;
        }
        let Some(size) = window_size(self.hwnd) else { return };
        if size == self.saved_size {
            return;
        }
        self.pending_save = Some(Instant::now());
    }

    /// Write the size once the player has stopped resizing for `SAVE_QUIET`.
    fn flush_pending_size(&mut self) {
        let Some(since) = self.pending_save else { return };
        if since.elapsed() < SAVE_QUIET {
            return;
        }
        self.pending_save = None;
        let Some(size) = window_size(self.hwnd) else { return };
        if size == self.saved_size {
            return;
        }
        self.saved_size = size;
        crate::config::save_panel_size(Some(size));
        crate::log(&format!("panel: size saved {}x{}", size.0, size.1));
    }

    /// The UI scale implied by the window's own size: the smaller of width/design and height/design.
    ///
    /// The smaller of the two because the layout is three columns side by side — growing the window
    /// in one direction only should give the columns more room, not stretch the text.
    fn auto_scale(&self) -> f32 {
        let mut rect = RECT::default();
        let _ = unsafe { GetClientRect(self.hwnd, &mut rect) };
        let width = (rect.right - rect.left).max(1) as f32;
        let height = (rect.bottom - rect.top).max(1) as f32;
        let raw = (width / DESIGN_WIDTH).min(height / DESIGN_HEIGHT);
        // Round to 5% so a framed window's client area (a few pixels inside the size it was
        // created at) still lands on the scale it was meant to: 1.974 is 2.0, not 197%.
        let rounded = (raw * 20.0).round() / 20.0;
        rounded.clamp(MIN_UI_SCALE, MAX_UI_SCALE)
    }

    /// The scale to build the UI at: the player's choice, or the one the window's size implies.
    fn target_scale(&self) -> f32 {
        match self.ui_scale {
            Some(fixed) => fixed.clamp(MIN_UI_SCALE, MAX_UI_SCALE),
            None => self.auto_scale(),
        }
    }

    /// Take a scale the player picked in the panel: remember it, persist it, and queue the rebuild.
    ///
    /// The window resize is only *queued*: `SetWindowPos` delivers `WM_SIZE` synchronously, and the
    /// window procedure cannot reach `App` while the frame that picked the scale is holding it —
    /// it would fall through to `DefWindowProcW` and the swap chain would keep its old size.
    fn set_ui_scale(&mut self, choice: Option<f32>) {
        self.ui_scale = choice;
        let scale = choice.unwrap_or_else(|| self.auto_scale());
        // A pinned scale owns the size (so it cannot be clipped); auto keeps the window it has and
        // remembers that size instead.
        let size = match choice {
            Some(scale) => {
                self.pending_resize = Some(scale);
                design_size(scale)
            }
            None => window_size(self.hwnd).unwrap_or_else(|| design_size(scale)),
        };
        self.saved_size = size;
        crate::config::save_panel_layout(choice, Some(size));
        self.pending_scale = Some(scale);
    }

    /// Rebuild the ImGui context and its font atlas at `scale`. Between frames only.
    ///
    /// The old context is dropped *before* the new one is built: imgui-rs allows one current
    /// context per process and `Context::create` panics while another exists — which is what closed
    /// the panel on every rescale before this took the old one out first.
    fn apply_scale(&mut self, scale: f32) {
        let Some(gfx) = self.gfx.as_mut() else { return };
        if (gfx.scale - scale).abs() < 0.02 {
            return;
        }
        drop(gfx.ui.take());
        match unsafe { build_ui(&gfx.device, scale) } {
            Ok(ui) => {
                gfx.ui = Some(ui);
                gfx.last_frame = Instant::now();
                gfx.scale = scale;
                self.last_scale = Instant::now();
                crate::log(&format!("panel: UI scale {:.0}%", scale * 100.0));
            }
            Err(e) => {
                // The old context is already gone, so there is nothing to draw with until the
                // panel is reopened (which rebuilds the device).
                crate::log(&format!("panel: rescale failed: {e}"));
            }
        }
    }

    /// Build the device, swap chain and ImGui renderer. First open only.
    fn init_gfx(&mut self) -> bool {
        if self.gfx.is_some() {
            return true;
        }
        let mut rect = RECT::default();
        let _ = unsafe { GetClientRect(self.hwnd, &mut rect) };
        let width = (rect.right - rect.left).max(1) as u32;
        let height = (rect.bottom - rect.top).max(1) as u32;

        let desc = DXGI_SWAP_CHAIN_DESC {
            BufferDesc: DXGI_MODE_DESC {
                Width: width,
                Height: height,
                RefreshRate: DXGI_RATIONAL { Numerator: 60, Denominator: 1 },
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                ScanlineOrdering: Default::default(),
                Scaling: Default::default(),
            },
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            OutputWindow: self.hwnd,
            Windowed: true.into(),
            SwapEffect: DXGI_SWAP_EFFECT_DISCARD,
            Flags: 0,
        };

        let levels: [D3D_FEATURE_LEVEL; 3] =
            [D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_10_0];
        let mut swap_chain: Option<IDXGISwapChain> = None;
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        let mut level = D3D_FEATURE_LEVEL_11_0;

        let created = unsafe {
            D3D11CreateDeviceAndSwapChain(
                None::<&IDXGIAdapter>,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&desc),
                Some(&mut swap_chain),
                Some(&mut device),
                Some(&mut level),
                Some(&mut context),
            )
        };
        let (device, context, swap_chain) = match (created, device, context, swap_chain) {
            (Ok(()), Some(device), Some(context), Some(swap_chain)) => (device, context, swap_chain),
            (Err(e), ..) => {
                crate::log(&format!("panel: D3D11 device creation failed: {e}"));
                return false;
            }
            _ => {
                crate::log("panel: D3D11 returned incomplete objects");
                return false;
            }
        };

        // The scale follows the window, which the window procedure sized from the monitor when it
        // was created — so the first thing the player sees on a 4K screen is already 2x.
        let scale = self.target_scale();
        let ui = match unsafe { build_ui(&device, scale) } {
            Ok(ui) => ui,
            Err(e) => {
                crate::log(&format!("panel: {e}"));
                return false;
            }
        };

        crate::log(&format!(
            "panel: D3D11 device + ImGui renderer created at {:.0}% ({width}x{height})",
            scale * 100.0
        ));
        let mut gfx = Gfx {
            device,
            context,
            swap_chain,
            view: None,
            size: (width, height),
            ui: Some(ui),
            last_frame: Instant::now(),
            presented: false,
            scale,
        };
        self.last_scale = Instant::now();
        unsafe { gfx.rebuild_view() };
        self.gfx = Some(gfx);
        true
    }

    /// Follow the pointer while the header band is dragged.
    ///
    /// Polled from the frame loop rather than driven by `WM_MOUSEMOVE`: the window moves *with*
    /// the pointer, so the client coordinates under the cursor never change and a fast drag can
    /// leave the window altogether. `GetCursorPos` is the only reliable source, and the mouse
    /// button's own state ends the drag wherever the pointer happens to be.
    fn drag_step(&mut self) {
        let Some(drag) = self.drag else { return };
        if unsafe { GetAsyncKeyState(VK_LBUTTON) } >= 0 {
            self.drag = None;
            return;
        }
        let mut point = POINT::default();
        if unsafe { GetCursorPos(&mut point) }.is_err() {
            self.drag = None;
            return;
        }
        let x = drag.origin[0] + (point.x - drag.from[0]);
        let y = drag.origin[1] + (point.y - drag.from[1]);
        // NOZORDER keeps the panel topmost; NOACTIVATE keeps focus where it is (the panel owns
        // it already, and stealing it mid-drag would close the panel through WM_ACTIVATE).
        let _ = unsafe {
            SetWindowPos(
                self.hwnd,
                None,
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
    }

    fn set_open(&mut self, open: bool) {
        if open && !self.init_gfx() {
            crate::log("panel: no panel graphics; staying closed");
            return;
        }
        self.shown = open;
        self.model.panel.set_visible(open);
        unsafe {
            if open {
                let _ = ShowWindow(self.hwnd, SW_SHOW);
            }
            if open {
                let previous = GetForegroundWindow();
                if !previous.0.is_null() && previous != self.hwnd {
                    self.previous_foreground = Some(previous);
                }
                let _ = SetForegroundWindow(self.hwnd);
            } else if let Some(previous) = self.previous_foreground.take() {
                // Focus goes back to the game *before* our window disappears: once the popup is
                // hidden the process owns no visible foreground window, and Windows then refuses
                // SetForegroundWindow, leaving the game with no keyboard at all (Esc and the
                // keybinding screen both go dead while the pad keeps working).
                let _ = SetForegroundWindow(previous);
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            } else {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
        if open {
            self.model.reload();
        }
        crate::log(if open { "panel: open" } else { "panel: closed" });
    }

    fn draw(&mut self, ui: &Ui) {
        // Read before the model borrow: the combo shows the scale auto would pick right now.
        let auto_scale = self.auto_scale();
        let current_scale = self.ui_scale;
        let model = &mut self.model;
        let mut assign: Option<(usize, i32)> = None;
        let mut scale_choice: Option<Option<f32>> = None;

        // Recomputed each frame so the search box takes effect immediately.
        model.refresh_candidates();
        let candidates = Rc::clone(&model.candidates);

        let size = ui.io().display_size;
        imgui::Window::new("nr-casting")
            .position([0.0, 0.0], Condition::Always)
            .size(size, Condition::Always)
            .title_bar(false)
            .resizable(false)
            .movable(false)
            .collapsible(false)
            .scroll_bar(false)
            .build(ui, || {
                ui.text("nr-casting");
                ui.separator();
                ui.columns(3, "layout", false);

                // ---- left hand -------------------------------------------------------------
                if let Some(choice) = hand_column(
                    ui,
                    "LEFT HAND",
                    model.weapons.0,
                    [0, 1, 2, 3],
                    "left",
                    &model.panel,
                    &candidates,
                    crate::input::y_bumper(),
                ) {
                    assign = Some(choice);
                }

                ui.next_column();

                // ---- middle ----------------------------------------------------------------
                // The name is the file the panel edits. The two lines under it are the game's
                // opinion, greyed like the disabled rows in the hand columns so they read as
                // information rather than as settings.
                ui.text(format!("Character: {}", model.who));
                ui.text_disabled(&model.stats);
                if !model.boost.is_empty() {
                    ui.text_disabled(&model.boost);
                }
                ui.spacing();
                ui.spacing();
                ui.set_next_item_width(ui.content_region_avail()[0]);
                ui.input_text("##search", &mut model.search)
                    .hint("search spells by name")
                    .build();
                ui.text_disabled(&model.listed);
                let mut show = model.show_all;
                if ui.checkbox("list everything", &mut show) {
                    model.show_all = show;
                    model.panel.set_show_all(show);
                }

                ui.next_column();

                // ---- right hand ------------------------------------------------------------
                if let Some(choice) = hand_column(
                    ui,
                    "RIGHT HAND",
                    model.weapons.1,
                    [4, 5, 6, 7],
                    "right",
                    &model.panel,
                    &candidates,
                    crate::input::y_bumper(),
                ) {
                    assign = Some(choice);
                }

                ui.columns(1, "footer", false);
                // ---- controls --------------------------------------------------------------
                // A section like the help text below it: a separator, the row, a separator. No
                // frame of its own — the extra box only made the panel look smaller.
                ui.separator();
                if ui.button("refresh") {
                    model.reload();
                }
                ui.same_line();
                // Two characters with the same name, slot and appearance cannot be told apart, so
                // the file is shared; this clears it for the one that is loaded.
                if ui.button("reset slots") {
                    crate::store::clear_character();
                    model.reload();
                }
                ui.same_line();
                if ui.button("close") {
                    // Handled after the frame: closing mid-frame would drop the state.
                    assign = assign.or(Some((usize::MAX, 0)));
                }
                ui.same_line();
                ui.text("UI scale");
                ui.same_line();
                if let Some(choice) = ui_scale_combo(ui, current_scale, auto_scale) {
                    scale_choice = Some(choice);
                }
                ui.separator();
                ui.text_disabled(
                    "Pad:  R1/R2 = right hand,  L1/L2 = left hand,  Y + R2 / Y + L2 = third spell,  \
                     Y + R1 / Y + L1 = fourth spell (unless Recluse owns it).",
                );
                ui.text_disabled(
                    "Mouse:  LMB = R1,  RMB = L1,  Shift+LMB = R2,  Shift+RMB = L2,  \
                     Ctrl+LMB = Y+R2,  Ctrl+RMB = Y+L2,  Alt+LMB = Y+R1,  Alt+RMB = Y+L1.",
                );
                ui.text_disabled("F8 opens and closes this panel;  Esc closes it.");
                ui.text_disabled(
                    "Bind:  hold L3 and press the input you want (Super + click on mouse/keyboard).",
                );
                ui.text_disabled(
                    "Drag the title band to move the panel here; the window edges resize it.",
                );
            });

        if let Some((index, spell_id)) = assign {
            if index == usize::MAX {
                self.set_open(false);
            } else {
                let slot = SLOTS[index];
                let weapon = self.model.weapon_of(slot);
                self.model.panel.assign(weapon, slot.weapon_slot, spell_id);
            }
        }
        if let Some(choice) = scale_choice {
            self.set_ui_scale(choice);
        }
    }

    fn render(&mut self) {
        // A scale change rebuilds the ImGui context and its font atlas, which cannot happen while a
        // frame is being built — so it is queued by the drag, by the ini, or by the panel's combo
        // and applied here, at the top of the next frame.
        if let Some(scale) = self.pending_scale.take() {
            self.apply_scale(scale);
        }
        // Taken out of `self` so `draw` can borrow the model mutably at the same time.
        let Some(mut gfx) = self.gfx.take() else {
            return;
        };
        // No context: a rescale is in flight or failed. Put the device back and retry next frame.
        if gfx.ui.is_none() {
            self.gfx = Some(gfx);
            return;
        }

        {
            let built = gfx.ui.as_mut().expect("context checked above");
            let io = built.imgui.io_mut();
            let now = Instant::now();
            io.delta_time = (now - gfx.last_frame).as_secs_f32().max(0.0001);
            io.display_size = [gfx.size.0 as f32, gfx.size.1 as f32];
            io.mouse_pos = self.input.mouse;
            io.mouse_down = self.input.down;
            if self.input.wheel != 0.0 {
                io.mouse_wheel = self.input.wheel;
                self.input.wheel = 0.0;
            }
            for character in self.input.chars.drain(..) {
                io.add_input_character(character);
            }
            for (key, down) in self.input.keys.drain(..) {
                io.keys_down[key] = down;
            }
        }
        gfx.last_frame = Instant::now();

        {
            let built = gfx.ui.as_mut().expect("context checked above");
            let frame = built.imgui.frame();
            self.draw(&frame);
            let draw_data = frame.render();
            unsafe {
                if let Some(view) = gfx.view.as_ref() {
                    gfx.context.OMSetRenderTargets(
                        Some(&[Some(view.clone())]),
                        None::<&ID3D11DepthStencilView>,
                    );
                    let clear = [0.07f32, 0.07, 0.09, 1.0];
                    gfx.context.ClearRenderTargetView(view, &clear);
                }
                if let Err(e) = built.renderer.render(draw_data) {
                    crate::log(&format!("panel: render failed: {e}"));
                }
                if !gfx.presented {
                    gfx.presented = true;
                    crate::log("panel: first frame presented");
                }
                let _ = gfx.swap_chain.Present(1, Default::default());
            }
        }

        self.gfx = Some(gfx);
    }

}

/// One hand's column: the equipped catalyst, then a row per slot (label, its mouse binding, and
/// the spell picker). Returns the pick the user made, if any.
fn hand_column(
    ui: &Ui,
    heading: &str,
    weapon: i32,
    indices: [usize; 4],
    prefix: &str,
    panel: &Panel,
    candidates: &[(i32, String)],
    y_bumper: bool,
) -> Option<(usize, i32)> {
    ui.text(heading);
    ui.text(if weapon > 0 {
        panel.weapon_name(weapon)
    } else {
        "(nothing equipped)".to_string()
    });
    let mut chosen = None;
    for index in indices {
        let slot = SLOTS[index];
        let current = if weapon > 0 {
            panel.slot_value(weapon, slot.weapon_slot)
        } else {
            -1
        };
        let preview = if current > 0 {
            format!("{} [{}]", panel.spell_name(current), current)
        } else {
            "(none)".to_string()
        };
        ui.text(slot.label);
        ui.same_line();
        ui.text_disabled(slot.mouse);
        // The fourth slot is Recluse's on a Recluse character: show the row, grey it out, and say
        // why, so nobody spends time wondering why Y+R1 does nothing.
        let off_for_recluse = !y_bumper && slot.weapon_slot == 3;
        ui.disabled(off_for_recluse, || {
            if off_for_recluse {
                // Not "off": the chords still work on a Recluse, they just belong to the class
                // port, which binds them to its own 4 and 5 keys.
                let key = if slot.right_hand { "4" } else { "5" };
                ui.text_disabled(format!("(Recluse class: passthrough to its {key} key)"));
            } else if let Some(picked) =
                slot_combo(ui, &format!("##{prefix}{index}"), preview, current, candidates)
            {
                chosen = Some((index, picked));
            }
        });
    }
    chosen
}

/// The panel's UI-scale picker: auto plus a few pinned multipliers.
///
/// Returns the new choice (outer `Some`) when the player picked one, `None` when nothing changed.
/// `auto` is what auto would pick right now, so the row says what it is doing.
fn ui_scale_combo(ui: &Ui, current: Option<f32>, auto: f32) -> Option<Option<f32>> {
    // "auto" is a real choice, so it is spelled as one: the percentage after it is what the window
    // currently implies, not a selection.
    let preview = match current {
        Some(scale) => format!("{:.0}%##ui_scale_preview", scale * 100.0),
        None => format!("auto ({:.0}% now)##ui_scale_preview", auto * 100.0),
    };
    let mut chosen = None;
    if let Some(_combo) = ui.begin_combo("##ui_scale", preview) {
        if imgui::Selectable::new("auto — follow the window##ui_scale_auto")
            .selected(current.is_none())
            .build(ui)
        {
            chosen = Some(None);
        }
        for scale in UI_SCALE_CHOICES {
            let percent = (scale * 100.0).round() as i32;
            let pinned = current.is_some_and(|value| (value - scale).abs() < 0.01);
            if imgui::Selectable::new(format!("{percent}% (pinned)##ui_scale_{percent}"))
                .selected(pinned)
                .build(ui)
            {
                chosen = Some(Some(scale));
            }
        }
    }
    chosen
}

/// One slot's spell picker.
fn slot_combo(
    ui: &Ui,
    id: &str,
    preview: String,
    current: i32,
    candidates: &[(i32, String)],
) -> Option<i32> {
    let mut chosen = None;
    if let Some(_combo) = ui.begin_combo(id, preview) {
        for (spell_id, text) in candidates {
            if imgui::Selectable::new(format!("{text}##{spell_id}"))
                .selected(*spell_id == current)
                .build(ui)
            {
                chosen = Some(*spell_id);
            }
        }
    }
    chosen
}

/// Map the virtual keys the text fields need onto ImGui's key indices.
fn map_key(vk: u16) -> Option<usize> {
    use imgui::Key;
    Some(match vk {
        0x08 => Key::Backspace as usize,
        0x09 => Key::Tab as usize,
        0x0D => Key::Enter as usize,
        0x1B => Key::Escape as usize,
        0x23 => Key::End as usize,
        0x24 => Key::Home as usize,
        0x25 => Key::LeftArrow as usize,
        0x26 => Key::UpArrow as usize,
        0x27 => Key::RightArrow as usize,
        0x28 => Key::DownArrow as usize,
        0x2E => Key::Delete as usize,
        _ => return None,
    })
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // A panic must never unwind into the game: crossing this FFI boundary aborts the process.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        APP.with(|cell| {
            let mut guard = cell.try_borrow_mut().ok()?;
            let app = guard.as_mut()?;
            match msg {
                WM_CLOSE => {
                    app.set_open(false);
                    Some(LRESULT(0))
                }
                // Alt-tab away and our topmost popup ends up under the game, which then holds a
                // window the player cannot reach: the game looks locked. Losing activation
                // closes the panel instead, so focus always goes back to the game.
                WM_ACTIVATE => {
                    if (wparam.0 & 0xFFFF) == 0 {
                        app.set_open(false);
                    }
                    Some(LRESULT(0))
                }
                WM_SYSKEYDOWN | WM_SYSKEYUP | WM_KEYDOWN | WM_KEYUP => {
                    let vk = wparam.0 as u16;
                    // Alt alone closes the panel before Windows builds its alt-tab list. The
                    // panel is topmost, so otherwise the first alt-tab target is the game again
                    // and the user can never reach another application. Closing here hands focus
                    // back to the game first, so alt-tab behaves exactly as it would from the game.
                    if vk == 0x12 && (msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN) {
                        app.set_open(false);
                        return Some(LRESULT(0));
                    }
                    if let Some(key) = map_key(vk) {
                        // Bounded: a stuck focus can deliver auto-repeat for as long as nobody
                        // builds a frame, and this vector is only drained by one.
                        if app.input.keys.len() < 256 {
                            app.input.keys.push((key, msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN));
                        }
                    }
                    if (msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN) && vk == 0x1B {
                        app.set_open(false);
                    }
                    Some(LRESULT(0))
                }
                // Dragging: see `Drag`. The window is borderless, so the header band is the
                // handle, but the move is done here rather than by `DefWindowProcW`'s modal
                // loop — that loop needs the mouse capture the mod gives away on purpose.
                WM_LBUTTONDOWN => {
                    let y = ((lparam.0 >> 16) & 0xFFFF) as i16;
                    if (0..TITLE_BAND).contains(&y) {
                        let mut point = POINT::default();
                        let mut rect = RECT::default();
                        if unsafe { GetCursorPos(&mut point) }.is_ok()
                            && unsafe { GetWindowRect(hwnd, &mut rect) }.is_ok()
                        {
                            app.drag = Some(Drag {
                                from: [point.x, point.y],
                                origin: [rect.left, rect.top],
                            });
                            // Deliberately not fed to ImGui: this press moves the panel.
                            return Some(LRESULT(0));
                        }
                    }
                    app.input.down[0] = true;
                    Some(LRESULT(0))
                }
                WM_LBUTTONUP => {
                    app.drag = None;
                    app.input.down[0] = false;
                    Some(LRESULT(0))
                }
                WM_GETMINMAXINFO => {
                    let info = lparam.0 as *mut MINMAXINFO;
                    if !info.is_null() {
                        unsafe {
                            // The design size, so the panel cannot be dragged smaller than the
                            // size its layout needs. Bigger than this only ever means a bigger
                            // UI scale (see `auto_scale`), never clipped columns.
                            (*info).ptMinTrackSize.x = DESIGN_WIDTH as i32;
                            (*info).ptMinTrackSize.y = DESIGN_HEIGHT as i32;
                        }
                    }
                    Some(LRESULT(0))
                }
                // A resize drag has ended: the window is the size it will stay, so work out the
                // scale that size implies and queue the rebuild. Doing this per WM_SIZE would
                // rebuild the font atlas for every pixel the corner moved.
                WM_EXITSIZEMOVE => {
                    let scale = app.target_scale();
                    let changed = app
                        .gfx
                        .as_ref()
                        .is_some_and(|gfx| (gfx.scale - scale).abs() >= 0.02);
                    if changed {
                        app.pending_scale = Some(scale);
                    }
                    // The player finished a drag: start the quiet period for the saved size.
                    app.note_window_size();
                    Some(LRESULT(0))
                }
                WM_SIZE => {
                    if let Some(gfx) = app.gfx.as_mut() {
                        let width = (lparam.0 & 0xFFFF) as u32;
                        let height = ((lparam.0 >> 16) & 0xFFFF) as u32;
                        if width > 0 && height > 0 && (width, height) != gfx.size {
                            gfx.size = (width, height);
                            // DXGI refuses ResizeBuffers while any reference to a back buffer
                            // lives, so the render target has to go first.
                            gfx.view = None;
                            if let Err(e) = gfx.swap_chain.ResizeBuffers(
                                0,
                                width,
                                height,
                                DXGI_FORMAT_R8G8B8A8_UNORM,
                                Default::default(),
                            ) {
                                crate::log(&format!("panel: resize failed: {e}"));
                            }
                            gfx.rebuild_view();
                        }
                    }
                    // Every pixel of a drag lands here, so this only restarts the quiet timer.
                    // The UI follows the new size from the frame loop (`follow_window_scale`).
                    app.note_window_size();
                    Some(LRESULT(0))
                }
                WM_MOUSEMOVE => {
                    let x = (lparam.0 & 0xFFFF) as i16 as f32;
                    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as f32;
                    app.input.mouse = [x, y];
                    Some(LRESULT(0))
                }
                WM_RBUTTONDOWN | WM_RBUTTONUP => {
                    app.input.down[1] = msg == WM_RBUTTONDOWN;
                    Some(LRESULT(0))
                }
                WM_MOUSEWHEEL => {
                    let delta = ((wparam.0 >> 16) & 0xFFFF) as i16;
                    app.input.wheel = delta as f32 / 120.0;
                    Some(LRESULT(0))
                }
                WM_CHAR => {
                    if let Some(character) = char::from_u32(wparam.0 as u32) {
                        app.input.chars.push(character);
                    }
                    Some(LRESULT(0))
                }
                _ => None,
            }
        })
    }));

    match result {
        Ok(Some(result)) => result,
        Ok(None) => DefWindowProcW(hwnd, msg, wparam, lparam),
        Err(_) => {
            crate::log("panel: panic inside the window procedure (swallowed)");
            LRESULT(0)
        }
    }
}

/// Resize the panel window so the design fits at `scale`.
///
/// Called from the frame loop, never from inside the `App` borrow: `SetWindowPos` delivers
/// `WM_SIZE` synchronously and the window procedure needs that same `RefCell`.
fn size_window(hwnd: HWND, scale: f32) {
    let (width, height) = design_size(scale);
    // NOZORDER keeps the panel topmost; NOACTIVATE keeps focus where it is (the panel owns it
    // already, and stealing it would close the panel through WM_ACTIVATE).
    let _ = unsafe {
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            width as i32,
            height as i32,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
}

/// The window size that holds the design at `scale`.
fn design_size(scale: f32) -> (u32, u32) {
    (
        (DESIGN_WIDTH * scale).round() as u32,
        (DESIGN_HEIGHT * scale).round() as u32,
    )
}

/// The panel's current outer size, in pixels.
fn window_size(hwnd: HWND) -> Option<(u32, u32)> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
    let width = (rect.right - rect.left).max(1) as u32;
    let height = (rect.bottom - rect.top).max(1) as u32;
    Some((width, height))
}

/// Fit a saved size to this screen: a panel saved on a 4K TV must not open bigger than a 1080p one.
fn fit_to_screen(size: (u32, u32)) -> (u32, u32) {
    let screen_width = unsafe { GetSystemMetrics(SM_CXSCREEN) }.max(0) as u32;
    let screen_height = unsafe { GetSystemMetrics(SM_CYSCREEN) }.max(0) as u32;
    if screen_width == 0 || screen_height == 0 {
        return size;
    }
    // Leave room for the spot the panel opens at (80,80), so a full-screen-sized panel does not
    // open with its bottom edge below the screen; never below what the layout needs.
    let max_width = screen_width.saturating_sub(160).max(DESIGN_WIDTH as u32);
    let max_height = screen_height.saturating_sub(160).max(DESIGN_HEIGHT as u32);
    (size.0.min(max_width), size.1.min(max_height))
}

/// The size the panel window is created at.
///
/// A pinned `ui_scale` decides it (so the layout cannot be clipped), then a size the player saved,
/// then the screen: the design scaled by the screen height over 1080, which is what makes a 4K
/// display open at 2x instead of at a 900x620 postage stamp.
fn initial_window_size() -> (i32, i32) {
    let config = crate::config::load();
    let (width, height) = match (config.ui_scale, config.panel_size) {
        (Some(fixed), _) => design_size(fixed.clamp(MIN_UI_SCALE, MAX_UI_SCALE)),
        (None, Some(saved)) => fit_to_screen(saved),
        (None, None) => design_size(monitor_scale()),
    };
    (width as i32, height as i32)
}

fn create_window() -> Option<HWND> {
    unsafe {
        let instance = GetModuleHandleW(PCWSTR::null()).ok()?;
        let class_name = wide("nr_casting_panel");
        let title = wide("nr-casting - spell slots");
        // A pinned `ui_scale` decides the size, then the one the player saved, then the screen.
        let (width, height) = initial_window_size();
        let class = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        RegisterClassW(&class);
        CreateWindowExW(
            // WS_EX_TOOLWINDOW keeps the panel out of the alt-tab list and the taskbar, so it
            // reads as part of the game rather than as a second application window.
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_POPUP | WS_THICKFRAME,
            80,
            80,
            width,
            height,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .ok()
    }
}

/// Start the panel on its own thread. Returns immediately.
pub fn start() {
    if !crate::config::load().overlay {
        crate::log("panel: disabled in nr_casting.ini (casting only)");
        return;
    }
    std::thread::spawn(|| {
        match crate::pointer::install() {
            Ok(()) => crate::log("pointer: detours installed"),
            Err(e) => crate::log(&format!("pointer: FAILED to install detours: {e}")),
        }

        let Some(hwnd) = create_window() else {
            crate::log("panel: could not create the window");
            return;
        };
        APP.with(|cell| *cell.borrow_mut() = Some(App::new(hwnd)));
        crate::log("panel: ready (F8 opens it; no GPU device until then)");

        let mut msg = MSG::default();
        loop {
            // PeekMessageW, not GetMessageW: the latter blocks, so the hotkey poll would never
            // run while the window sits idle.
            while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.0 != 0 {
                if msg.message == WM_QUIT {
                    return;
                }
                let _ = unsafe { TranslateMessage(&msg) };
                unsafe { DispatchMessageW(&msg) };
            }

            // No-op while the panel is closed; releases the pointer's capture while it is open.
            crate::pointer::frame();

            // One bad frame must not take this thread down with it: the hotkey poll and the
            // per-frame capture upkeep live here, so losing the thread leaves the panel
            // unreachable and the pointer half-owned. The window procedure has its own
            // boundary; this is the other edge.
            let frame = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                APP.with(|cell| {
                    let mut guard = cell.borrow_mut();
                    let app = guard.as_mut()?;
                    let minimized = unsafe { IsIconic(app.hwnd) }.0 != 0;
                    let down = unsafe { GetAsyncKeyState(app.hotkey) } < 0;
                    if down && !app.hotkey_down {
                        let settled = app
                            .last_toggle
                            .map(|when| when.elapsed().as_millis() > 300)
                            .unwrap_or(true);
                        if settled {
                            app.last_toggle = Some(Instant::now());
                            let open = !app.shown;
                            app.set_open(open);
                        }
                    }
                    app.hotkey_down = down;
                    // Before drawing: a drag has to move the window under the cursor, and the
                    // ImGui frame would otherwise be a frame behind the move.
                    app.drag_step();
                    if app.shown && !minimized {
                        // Before drawing: a live resize should already be at the new scale.
                        app.follow_window_scale();
                        app.render();
                    }
                    // Outside the `shown` check: closing the panel right after a resize must
                    // still save the size.
                    app.flush_pending_size();
                    // Taken out here and run below, outside the borrow: the resize delivers
                    // window messages, and the window procedure cannot borrow `App` twice.
                    app.pending_resize.take().map(|scale| (app.hwnd, scale))
                })
            }));
            if let Ok(Some((hwnd, scale))) = frame {
                size_window(hwnd, scale);
            }
            if frame.is_err() {
                crate::log(
                    "panel: panic in the frame loop (swallowed) — closing; the next open rebuilds the device",
                );
                APP.with(|cell| {
                    if let Ok(mut guard) = cell.try_borrow_mut() {
                        if let Some(app) = guard.as_mut() {
                            app.shown = false;
                            app.gfx = None;
                            app.model.panel.set_visible(false);
                        }
                    }
                });
            }

            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    });
}
