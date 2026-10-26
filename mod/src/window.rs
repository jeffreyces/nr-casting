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
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM};
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
    CreateWindowExW, DefWindowProcW, LoadCursorW, IDC_ARROW, WS_EX_TOPMOST, WS_POPUP, DispatchMessageW, GetClientRect, IsIconic, PeekMessageW,
    GetForegroundWindow, RegisterClassW, SetForegroundWindow, ShowWindow, TranslateMessage, MSG,
    PM_REMOVE, SW_HIDE, SW_SHOW,
    WNDCLASSW, WM_CHAR, WM_CLOSE, WM_GETMINMAXINFO, WM_KEYDOWN, WM_KEYUP, WM_NCHITTEST,
    WS_THICKFRAME, MINMAXINFO,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN,
    WM_RBUTTONUP, WM_SIZE,
};

use crate::overlay::Panel;

/// Height of the draggable header band, in physical pixels.
const TITLE_BAND: i16 = 28;
/// `HTCAPTION` as `DefWindowProcW` returns it; the `windows` crate only exposes the constant on
/// some target feature sets, and the value is part of the Win32 ABI.
const HTCAPTION: isize = 2;

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
    Slot { label: "Y+L2", mouse: "Ctrl+RMB", weapon_slot: 2, right_hand: false },
    Slot { label: "Y+L1", mouse: "Alt+RMB", weapon_slot: 3, right_hand: false },
    Slot { label: "R1", mouse: "LMB", weapon_slot: 0, right_hand: true },
    Slot { label: "R2", mouse: "Shift+LMB", weapon_slot: 1, right_hand: true },
    Slot { label: "Y+R2", mouse: "Ctrl+LMB", weapon_slot: 2, right_hand: true },
    Slot { label: "Y+R1", mouse: "Alt+LMB", weapon_slot: 3, right_hand: true },
];

/// Everything the panel shows, refreshed when it opens.
struct Model {
    panel: Panel,
    weapons: (i32, i32),
    search: String,
    show_all: bool,
    status: String,
    /// Rebuilt only when the filter or the character's stats change. The panel redraws every
    /// frame, and this list is several hundred formatted strings.
    candidates: Rc<Vec<(i32, String)>>,
    candidates_key: String,
}

impl Model {
    fn refresh_candidates(&mut self) {
        let stats = crate::player_stats().unwrap_or((0, 0, 0));
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
            show_all: false,
            status: "waiting for the player...".to_string(),
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
        let stats = crate::player_stats()
            .map(|(int, fai, arc)| format!("INT {int}   FAI {fai}   ARC {arc}"))
            .unwrap_or_else(|| "stats unavailable".to_string());
        self.status = format!(
            "{stats}       {listed}/{} spells listed",
            self.panel.spell_count()
        );
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
    imgui: imgui::Context,
    renderer: imgui_dx11_renderer::Renderer,
    last_frame: Instant,
    presented: bool,
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
    hotkey: i32,
    hotkey_down: bool,
    /// A physical press must produce exactly one toggle: the poll runs every 16 ms and the
    /// window can also see the key, so ignore anything closer than this.
    last_toggle: Option<Instant>,
}

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
            hotkey: crate::config::load().hotkey,
            hotkey_down: false,
            last_toggle: None,
            previous_foreground: None,
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

        let mut imgui = imgui::Context::create();
        imgui.set_ini_filename(None);
        imgui.style_mut().window_rounding = 4.0;
        imgui.style_mut().frame_rounding = 3.0;
        imgui.fonts().add_font(&[imgui::FontSource::DefaultFontData {
            config: Some(imgui::FontConfig { size_pixels: 15.0, ..Default::default() }),
        }]);

        let renderer = match unsafe { imgui_dx11_renderer::Renderer::new(&mut imgui, &device) } {
            Ok(renderer) => renderer,
            Err(e) => {
                crate::log(&format!("panel: ImGui D3D11 renderer failed: {e}"));
                return false;
            }
        };

        crate::log("panel: D3D11 device + ImGui renderer created");
        let mut gfx = Gfx {
            device,
            context,
            swap_chain,
            view: None,
            size: (width, height),
            imgui,
            renderer,
            last_frame: Instant::now(),
            presented: false,
        };
        unsafe { gfx.rebuild_view() };
        self.gfx = Some(gfx);
        true
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
        let model = &mut self.model;
        let mut assign: Option<(usize, i32)> = None;

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
                ) {
                    assign = Some(choice);
                }

                ui.next_column();

                // ---- middle ----------------------------------------------------------------
                ui.text("CHARACTER");
                ui.text(&model.status);
                ui.input_text("##search", &mut model.search).build();
                let mut show = model.show_all;
                if ui.checkbox("show everything", &mut show) {
                    model.show_all = show;
                    model.panel.set_show_all(show);
                }
                if ui.button("refresh") {
                    model.reload();
                }
                let mut close = false;
                if ui.button("close") {
                    close = true;
                }
                if close {
                    // Handled after the frame: closing mid-frame would drop the state.
                    assign = assign.or(Some((usize::MAX, 0)));
                }
                ui.spacing();

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
                ) {
                    assign = Some(choice);
                }

                ui.columns(1, "footer", false);
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
    }

    fn render(&mut self) {
        // Taken out of `self` so `draw` can borrow the model mutably at the same time.
        let Some(mut gfx) = self.gfx.take() else {
            return;
        };

        {
            let io = gfx.imgui.io_mut();
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
            let ui = gfx.imgui.frame();
            self.draw(&ui);
            let draw_data = ui.render();
            unsafe {
                if let Some(view) = gfx.view.as_ref() {
                    gfx.context.OMSetRenderTargets(
                        Some(&[Some(view.clone())]),
                        None::<&ID3D11DepthStencilView>,
                    );
                    let clear = [0.07f32, 0.07, 0.09, 1.0];
                    gfx.context.ClearRenderTargetView(view, &clear);
                }
                if let Err(e) = gfx.renderer.render(draw_data) {
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
        if let Some(picked) = slot_combo(ui, &format!("##{prefix}{index}"), preview, current, candidates)
        {
            chosen = Some((index, picked));
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
                WM_KEYDOWN | WM_KEYUP => {
                    let vk = wparam.0 as u16;
                    if let Some(key) = map_key(vk) {
                        // Bounded: a stuck focus can deliver auto-repeat for as long as nobody
                        // builds a frame, and this vector is only drained by one.
                        if app.input.keys.len() < 256 {
                            app.input.keys.push((key, msg == WM_KEYDOWN));
                        }
                    }
                    if msg == WM_KEYDOWN && vk == 0x1B {
                        app.set_open(false);
                    }
                    Some(LRESULT(0))
                }
                // Borderless, so there is no caption to drag by: the header band moves the
                // window and the thick frame gives the edges their resize cursors.
                WM_NCHITTEST => {
                    let y = ((lparam.0 >> 16) & 0xFFFF) as i16;
                    if (0..TITLE_BAND).contains(&y) {
                        return Some(LRESULT(HTCAPTION));
                    }
                    // Fall through to the default handler for the resize borders.
                    None
                }
                WM_GETMINMAXINFO => {
                    let info = lparam.0 as *mut MINMAXINFO;
                    if !info.is_null() {
                        unsafe {
                            (*info).ptMinTrackSize.x = 520;
                            (*info).ptMinTrackSize.y = 320;
                        }
                    }
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
                    Some(LRESULT(0))
                }
                WM_MOUSEMOVE => {
                    let x = (lparam.0 & 0xFFFF) as i16 as f32;
                    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as f32;
                    app.input.mouse = [x, y];
                    Some(LRESULT(0))
                }
                WM_LBUTTONDOWN | WM_LBUTTONUP => {
                    app.input.down[0] = msg == WM_LBUTTONDOWN;
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

fn create_window() -> Option<HWND> {
    unsafe {
        let instance = GetModuleHandleW(PCWSTR::null()).ok()?;
        let class_name = wide("nr_casting_panel");
        let title = wide("nr-casting - spell slots");
        let class = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        RegisterClassW(&class);
        CreateWindowExW(
            WS_EX_TOPMOST,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_POPUP | WS_THICKFRAME,
            80,
            80,
            900,
            620,
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
                    let Some(app) = guard.as_mut() else {
                        return;
                    };
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
                    if app.shown && !minimized {
                        app.render();
                    }
                });
            }));
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
