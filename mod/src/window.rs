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

use crate::panel_geometry::{
    center_window, design_size, fit_window_height, initial_window_pos, initial_window_size,
    size_window, window_pos, window_size,
};
use crate::panel_shell::{open_url, reveal};

// The design size and the scale bounds live in the core crate, with the fit that uses them, so
// the formula the panel runs is the one the tests cover.
use nr_casting_core::{fit_scale, Balance, RowState, MAX_UI_SCALE, MIN_UI_SCALE};

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use imgui::{Condition, StyleColor, Ui};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1,
    D3D_FEATURE_LEVEL_11_0,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11DepthStencilView, ID3D11Device, ID3D11DeviceContext,
    ID3D11RenderTargetView, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::IDXGIAdapter;
use windows::Win32::Graphics::Gdi::{InvalidateRect, UpdateWindow};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, HOT_KEY_MODIFIERS};
use windows::Win32::UI::WindowsAndMessaging::WaitMessage;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, GetCursorPos,
    GetDesktopWindow, GetForegroundWindow, GetWindowRect, IsIconic, LoadCursorW, PeekMessageW,
    RegisterClassW, SetForegroundWindow, SetWindowPos, ShowWindow, TranslateMessage, IDC_ARROW,
    MINMAXINFO, MSG, PM_REMOVE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, SW_HIDE, SW_SHOW, WM_ACTIVATE, WM_CHAR, WM_CLOSE, WM_ENTERSIZEMOVE,
    WM_EXITSIZEMOVE, WM_GETMINMAXINFO, WM_HOTKEY, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_SIZE, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER, WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP, WS_THICKFRAME,
};
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

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
const BASE_FONT_PX: f32 = 15.0;
/// The scales offered in the panel, as multipliers. Auto is offered separately.
const UI_SCALE_CHOICES: [f32; 5] = [1.0, 1.25, 1.5, 1.75, 2.0];

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
    Slot {
        label: "L1",
        mouse: "RMB",
        weapon_slot: 0,
        right_hand: false,
    },
    Slot {
        label: "L2",
        mouse: "Shift+RMB",
        weapon_slot: 1,
        right_hand: false,
    },
    Slot {
        label: "Y+L1",
        mouse: "Alt+RMB",
        weapon_slot: 3,
        right_hand: false,
    },
    Slot {
        label: "Y+L2",
        mouse: "Ctrl+RMB",
        weapon_slot: 2,
        right_hand: false,
    },
    Slot {
        label: "R1",
        mouse: "LMB",
        weapon_slot: 0,
        right_hand: true,
    },
    Slot {
        label: "R2",
        mouse: "Shift+LMB",
        weapon_slot: 1,
        right_hand: true,
    },
    Slot {
        label: "Y+R1",
        mouse: "Alt+LMB",
        weapon_slot: 3,
        right_hand: true,
    },
    Slot {
        label: "Y+R2",
        mouse: "Ctrl+LMB",
        weapon_slot: 2,
        right_hand: true,
    },
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
    /// The class port the equipped weapons belong to, when one does: named from the port's own
    /// weapon, because the character's ids read 0 on this build.
    port_class: Option<String>,
    /// Whether assignments are limited to the character's memorised spells, and the memorised
    /// list itself. The column and every marker are conditional on `balance`.
    balance: Balance,
    attuned: Vec<i32>,
    /// Rebuilt only when the filter or the character's stats change. The panel redraws every
    /// frame, and this list is several hundred formatted strings.
    candidates: Rc<Vec<(i32, String)>>,
    /// The right hand's list: a staff and a seal offer different spells, so each hand needs its own.
    candidates_right: Rc<Vec<(i32, String)>>,
    candidates_key: String,
}

impl Model {
    fn refresh_candidates(&mut self) {
        // Effective stats: base plus active effects. Keying the cache on them means a Great
        // Rune going on (or a talisman swap) re-filters the list instead of leaving it stale.
        let stats = crate::player_effective_stats().unwrap_or((0, 0, 0));
        let (int, fai, arc) = stats;
        // The equipped catalysts decide what each list may contain, so they belong in the key: a
        // weapon swap has to re-filter both hands.
        let key = format!(
            "{}\u{1}{}\u{1}{int}\u{1}{fai}\u{1}{arc}\u{1}{}\u{1}{}",
            self.search, self.show_all, self.weapons.0, self.weapons.1
        );
        if key == self.candidates_key {
            return;
        }
        // Balanced mode offers the memorised set, and nothing else. That is the whole point of the
        // mode: the list *is* what the character can actually cast, so a spell that is not attuned
        // has no business in it. Planning is the exception the player asks for explicitly - with
        // "list everything" on, the rest appears marked and is refused at selection, so a build can
        // be sketched without the gate ever being bypassed.
        let attuned = &self.attuned;
        let limit = |list: Vec<(i32, String)>| -> Vec<(i32, String)> {
            if self.balance != Balance::Attuned {
                return list;
            }
            list.into_iter()
                .filter(|(id, _)| self.show_all || attuned.contains(id))
                .map(|(id, label)| {
                    let label = if attuned.contains(&id) {
                        label
                    } else {
                        format!("{label}   (not attuned)")
                    };
                    (id, label)
                })
                .collect()
        };
        self.candidates = Rc::new(limit(
            self.panel.candidates_for(self.weapons.0, &self.search),
        ));
        self.candidates_right = Rc::new(limit(
            self.panel.candidates_for(self.weapons.1, &self.search),
        ));
        self.candidates_key = key;
    }

    fn new(config: &crate::config::Config) -> Self {
        Model {
            panel: Panel::new(),
            weapons: (0, 0),
            search: String::new(),
            show_all: config.show_all,
            port_class: None,
            balance: config.balance,
            attuned: Vec::new(),
            who: "waiting for the player...".to_string(),
            stats: String::new(),
            boost: String::new(),
            listed: String::new(),
            candidates: Rc::new(Vec::new()),
            candidates_right: Rc::new(Vec::new()),
            candidates_key: String::new(),
        }
    }

    /// How many of the eight rows the panel shows are dormant right now.
    ///
    /// Used for the note when the toggle flips, so the line reports what changed instead of just
    /// announcing a setting: "4 rows went dormant" is the sentence that explains the greying.
    fn stale_rows(&self) -> u32 {
        nr_casting_core::counts(&self.pair_rows()).stale
    }

    /// Every row the panel can show, both hands, in one list: one assembly, two readers.
    fn pair_rows(&self) -> Vec<RowState> {
        let balanced = self.balance == Balance::Attuned;
        let mut rows = Vec::with_capacity(2 * 4);
        rows.extend_from_slice(&row_states(
            &self.panel,
            self.weapons.0,
            &self.attuned,
            balanced,
        ));
        rows.extend_from_slice(&row_states(
            &self.panel,
            self.weapons.1,
            &self.attuned,
            balanced,
        ));
        rows
    }

    fn reload(&mut self) {
        self.panel.reload();
        self.candidates_key.clear();
        // The gate and its list are read on the panel thread with the panel open: the one place a
        // game-memory read is allowed to be slow. The pad poll only ever consults the published
        // snapshot (see `game::spell_attuned`).
        self.balance = crate::config::balance();
        // `debug_log` only: what the panel decided each hand can cast, with the weapon it decided
        // it from. "The panel says both on a dagger" is a question about this line.
        if crate::config::load().debug_log {
            for (hand, weapon) in [("left", self.weapons.0), ("right", self.weapons.1)] {
                let schools = self.panel.catalyst_schools(weapon);
                crate::log(&format!(
                    "panel: {hand} hand weapon {weapon} casts: {} (sorcery={} incantation={})",
                    schools.label(),
                    schools.sorcery,
                    schools.incantation
                ));
            }
        }
        crate::game::refresh_attuned();
        self.attuned = crate::game::attuned_snapshot();
        self.weapons = crate::weapon_ids().unwrap_or((0, 0));
        self.port_class = crate::overlay::port_class(self.weapons);
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
                    if boosted {
                        "incl. rune/talisman".to_string()
                    } else {
                        String::new()
                    },
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
/// The panel's graphics device, swap chain and current ImGui context.
///
/// **Deliberately never dropped after the first open.** Closing the panel hides the window and
/// keeps this; releasing it would mean creating a D3D11 device inside a running game every time the
/// player presses the hotkey, and that hitch is the reason the device is built on the first open
/// and not during play. The cost is a device, a swap chain and two back buffers resident for the
/// process (roughly 5-8 MB at panel size), which is the cheaper of the two.
struct Gfx {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    /// Where the pixels go: a texture, a staging copy and a DIB. No swap chain, on purpose - see
    /// `surface.rs` for the measurement that made that a requirement rather than a preference.
    surface: crate::surface::Surface,
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
    /// When the last diagnostic line was written (once a second while the panel is open).
    last_report: Instant,
    /// When the last frame actually reached the screen. A window that stops presenting looks
    /// frozen — the old picture stays on the client area, so the panel appears to lose its frame
    /// and stop responding to a drag, and only an F8 brings it back.
    last_present: Instant,
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
    imgui
        .fonts()
        .add_font(&[imgui::FontSource::DefaultFontData {
            config: Some(imgui::FontConfig {
                size_pixels: BASE_FONT_PX * scale,
                ..Default::default()
            }),
        }]);
    // A fresh context carries the default style, so this scales exactly once however often the
    // panel is rescaled.
    imgui.style_mut().scale_all_sizes(scale);
    // The panel draws its own frame on the foreground list (see `draw`). ImGui paints a window
    // border on the window's own rect, expanded half a pixel outward, so a window that does not fit
    // its buffer loses the right and bottom rules — which is what made the panel look frozen and
    // un-resizable once it was dragged small.
    imgui.style_mut().window_border_size = 0.0;
    imgui.style_mut().window_rounding = 4.0 * scale;
    imgui.style_mut().frame_rounding = 3.0 * scale;
    let renderer = imgui_dx11_renderer::Renderer::new(&mut imgui, device)
        .map_err(|e| format!("ImGui D3D11 renderer failed: {e:?}"))?;
    Ok(PanelUi { imgui, renderer })
}

impl Gfx {
    unsafe fn rebuild_view(&mut self) {
        // The render target is a texture this mod owns, so there is no back buffer to ask for and
        // no `ResizeBuffers` that can fail while a view is alive.
        //
        // Drop the old view before replacing it: an overwritten COM pointer is a leaked render
        // target, not a freed one.
        self.view = None;
        let mut view: Option<ID3D11RenderTargetView> = None;
        match self
            .device
            .CreateRenderTargetView(self.surface.texture(), None, Some(&mut view))
        {
            Ok(()) => self.view = view,
            Err(e) => crate::log(&format!("panel: no render target: {e}")),
        }
    }

    /// Rebuild the render target and the DIB at a new size.
    ///
    /// A texture cannot be resized in place, so the old surface is replaced whole. A drag sends one
    /// `WM_SIZE` per pixel of movement, so resizing allocates a fresh surface repeatedly; that is
    /// the price of not having a swap chain to call `ResizeBuffers` on, and it is bounded by the
    /// panel's own size (about 4 MB at 1623x720). If the new one
    /// cannot be built the old one stays and the panel keeps drawing at its previous size, which is
    /// a better failure than a window that stops updating.
    unsafe fn resize_surface(&mut self, width: u32, height: u32) {
        self.view = None;
        match crate::surface::Surface::new(&self.device, width, height) {
            Ok(surface) => {
                self.surface = surface;
                self.size = self.surface.size();
            }
            Err(e) => {
                crate::log(&format!("panel: surface resize failed: {e}"));
                return;
            }
        }
        self.rebuild_view();
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
    /// When the memorised list was last re-read while the panel is open.
    attuned_at: Instant,
    input: Input,
    gfx: Option<Gfx>,
    shown: bool,
    /// Set while the player is dragging the panel by its header band.
    drag: Option<Drag>,
    hotkey: i32,
    hotkey_down: bool,
    /// Whether the hotkey is a registered system hotkey. When it is, the window gets a `WM_HOTKEY`
    /// and the thread can block on the queue instead of reading the key 60 times a second.
    hotkey_registered: bool,
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
    pending_resize: Option<Resize>,
    /// Ask the frame loop to put the window back in the middle of the screen ("reset layout").
    pending_center: bool,
    /// Close the panel at the top of the next frame (the bundle run needs the panel out of the way).
    close_next_frame: bool,
    /// One-shot: nobody has sized this panel by hand, so it should open exactly as tall as its
    /// content instead of at the design height with a band of empty space under it.
    needs_fit: bool,
    /// Measure the layout again on the next frame, for a change that is meant to alter it.
    remeasure: bool,
    /// The saved corner, so a drag that moves nothing does not rewrite the file.
    saved_pos: (i32, i32),
    /// What the diagnostics buttons last did, shown under the controls.
    note: String,
    /// Whether the bundle-run prompt is on screen.
    bundle_prompt: bool,
    /// The report waiting to be shown in the file manager, once the timer fires.
    ///
    /// `ShellExecuteW` from a spawned thread worked exactly once and then silently stopped, so the
    /// reveal is a window timer on the panel's own thread instead: one event, one place, no
    /// cross-thread shell calls.
    reveal: Option<std::path::PathBuf>,
    /// Inside the window manager's modal sizing loop: the frame loop is blocked there, so frames are
    /// painted from `WM_PAINT` instead.
    sizing: bool,
    /// The panel size last written to the ini, so a move/resize drag that changes nothing does not
    /// rewrite the file every time the window is nudged.
    saved_size: (u32, u32),
    /// When the UI was last rebuilt at a new scale: the rebuild is not free, so a live resize
    /// follows the window at a bounded rate instead of once per drag message.
    last_scale: Instant,
    /// When the panel went idle with a device still resident, so it can be released.
    ///
    /// The device is built on first open and, until now, never dropped: that saved a hitch on every
    /// reopen, and cost a D3D11 device, a swap chain and two back buffers resident in a running
    /// game for the rest of the session. One of those is a suspect for stutter that appears with
    /// any input device and no version before the panel existed - so it is released once the panel
    /// has been closed for a minute, and the next open pays the one hitch it always paid the first
    /// time.
    idle_since: Option<Instant>,
    /// When the window size last changed without being saved. The write happens once the player
    /// has stopped resizing, not on every drag.
    pending_save: Option<Instant>,
    /// How tall the layout is at 100%, measured from the last frame.
    ///
    /// The auto scale divides the window height by this rather than by the design constant, so a
    /// window that was fitted to its content reads back as exactly the scale it was fitted at. Use
    /// the design box instead and the fit and the scale disagree, and the panel drifts smaller
    /// every time it opens.
    natural_height: f32,
    /// How tall the layout actually was on the last frame, in pixels.
    ///
    /// The window's minimum height comes from this rather than from the design constant: the layout
    /// is shorter than the 620 px box it was designed in, and a fixed minimum forced that much empty
    /// space at the bottom of the panel.
    content_height: f32,
}

/// What the frame loop should do to the window once the frame is done.
///
/// `SetWindowPos` delivers `WM_SIZE` synchronously and the window procedure cannot borrow `App`
/// while the frame that asked for the resize still holds it, so the request is taken out of the
/// borrow and run by the loop.
#[derive(Clone, Copy)]
enum Resize {
    /// The design box at this scale: a pinned UI scale owns the window.
    Design(f32),
    /// Exactly as tall as the layout used, no taller.
    Fit(f32),
}

/// Wall-clock milliseconds at the last close, for the frame clock to report the game's resume.
///
/// Closing this panel takes 0 ms in `set_open` and still feels like a hitch, which means the
/// visible delay is on the game's side: it was unfocused while the panel was up, and the first
/// frames after focus returns are what the player is waiting for. One number separates "our window
/// lingers" from "the game took a moment", and it is the difference between a bug here and a
/// consequence of stealing focus.
pub(crate) static CLOSED_AT: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// The id this window registers its hotkey under; `WM_HOTKEY` carries it back.
const HOTKEY_ID: i32 = 1;
/// The timer that opens the file manager once the browser has had a moment.
const REVEAL_TIMER_ID: usize = 2;
/// The one-second timer that samples the frame series, whether or not a pad is being polled.
const SAMPLER_TIMER_ID: usize = 3;
/// How long the panel may stay closed with a D3D11 device resident before it is released.
const GFX_IDLE_DROP: std::time::Duration = std::time::Duration::from_secs(60);
/// `MOD_NOREPEAT`: holding the key must not toggle the panel on every auto-repeat.
const MOD_NOREPEAT: u32 = 0x4000;

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
        // Read once. `config::load` is a `OnceCell`, so a field on `App` would be a second copy of
        // a value that can never change — the binding is the whole of the win here.
        let config = crate::config::load();
        App {
            hwnd,
            model: Model::new(config),
            attuned_at: Instant::now(),
            input: Input::default(),
            gfx: None,
            shown: false,
            drag: None,
            hotkey: config.hotkey,
            hotkey_down: false,
            hotkey_registered: false,
            last_toggle: None,
            previous_foreground: None,
            ui_scale: config.ui_scale,
            pending_scale: None,
            pending_resize: None,
            pending_center: false,
            close_next_frame: false,
            needs_fit: config.panel_size.is_none(),
            remeasure: true,
            saved_pos: window_pos(hwnd).unwrap_or((0, 0)),
            note: String::new(),
            bundle_prompt: false,
            reveal: None,
            sizing: false,
            // The size the window actually opened at, not what the file asked for: a saved size
            // that had to be fitted to a smaller screen must not be rewritten just because the
            // player moved the panel.
            saved_size: window_size(hwnd).unwrap_or((0, 0)),
            last_scale: Instant::now(),
            idle_since: None,
            pending_save: None,
            content_height: 0.0,
            natural_height: 0.0,
        }
    }

    /// Follow the window's size while the player is still dragging.
    ///
    /// Note that the window moved or changed size; the ini write waits for the player to stop.
    ///
    /// A drag calls this on every `WM_SIZE`, and a slow deliberate drag can easily span several
    /// seconds, so the timer restarts each time and one write lands at the end of the drag — not
    /// one per pixel, and not one per drag when the player resizes twice in a row.
    fn note_window_geometry(&mut self) {
        let Some(size) = window_size(self.hwnd) else {
            return;
        };
        let pos = window_pos(self.hwnd).unwrap_or(self.saved_pos);
        if size == self.saved_size && pos == self.saved_pos {
            return;
        }
        self.pending_save = Some(Instant::now());
    }

    /// Write the size and corner once the player has stopped dragging for `SAVE_QUIET`.
    fn flush_pending_geometry(&mut self) {
        let Some(since) = self.pending_save else {
            return;
        };
        if since.elapsed() < SAVE_QUIET {
            return;
        }
        self.pending_save = None;
        let Some(size) = window_size(self.hwnd) else {
            return;
        };
        let pos = window_pos(self.hwnd).unwrap_or(self.saved_pos);
        if size == self.saved_size && pos == self.saved_pos {
            return;
        }
        self.saved_size = size;
        self.saved_pos = pos;
        crate::config::save_panel_geometry(Some(size), Some(pos));
        crate::log(&format!(
            "panel: geometry saved {}x{} at {},{}",
            size.0, size.1, pos.0, pos.1
        ));
    }

    /// The scale the window's own size implies.
    ///
    /// The smaller of width/design and height/natural, where natural is the height the layout
    /// takes at 100%: a window fitted to its content therefore reads back as the scale it was
    /// fitted at, and dragging either edge moves the scale without the two fighting.
    fn auto_scale(&self) -> f32 {
        let mut rect = RECT::default();
        let _ = unsafe { GetClientRect(self.hwnd, &mut rect) };
        let width = (rect.right - rect.left).max(1) as f32;
        let height = (rect.bottom - rect.top).max(1) as f32;
        // The fit itself is `fit_scale` in the core crate: it is the part that has been wrong
        // twice (an average that overflowed both axes, then a round instead of a floor), so it is
        // the part that is tested.
        fit_scale(width, height, self.natural_height)
    }

    /// The scale to build the UI at: the player's choice, or the one the window implies.
    fn target_scale(&self) -> f32 {
        match self.ui_scale {
            Some(fixed) => fixed.clamp(MIN_UI_SCALE, MAX_UI_SCALE),
            None => self.auto_scale(),
        }
    }

    /// Follow the window while the player drags it, at a bounded rate (auto only).
    fn follow_window_scale(&mut self) {
        if self.ui_scale.is_some() {
            return;
        }
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

    /// Take a scale the player picked in the panel: remember it, persist it, and queue the rebuild.
    ///
    /// The window resize is only *queued*: `SetWindowPos` delivers `WM_SIZE` synchronously, and the
    /// window procedure cannot reach `App` while the frame that picked the scale is holding it —
    /// it would fall through to `DefWindowProcW` and the swap chain would keep its old size.
    fn set_ui_scale(&mut self, choice: Option<f32>) {
        // A new font size changes how tall the layout is, so the window is re-fitted to it below.
        self.needs_fit = true;
        self.ui_scale = choice;
        let scale = choice.unwrap_or_else(|| self.auto_scale());
        // A pinned scale owns the size (so it cannot be clipped); auto keeps the window it has and
        // remembers that size instead.
        let size = match choice {
            Some(scale) => {
                self.pending_resize = Some(Resize::Design(scale));
                design_size(scale)
            }
            None => window_size(self.hwnd).unwrap_or_else(|| design_size(scale)),
        };
        self.saved_size = size;
        crate::config::save_panel_layout(choice, Some(size));
        self.pending_scale = Some(scale);
    }

    /// Release the device, swap chain and ImGui context once the panel has been closed a while.
    ///
    /// The skimpiest possible thing in the game's graphics stack: for the first time since the
    /// panel existed, a session that opens it once and plays for an hour does not leave a second
    /// D3D11 device resident and idle.
    fn drop_gfx_if_idle(&mut self) {
        if self.shown || self.gfx.is_none() {
            self.idle_since = None;
            return;
        }
        let since = *self.idle_since.get_or_insert_with(Instant::now);
        if since.elapsed() < GFX_IDLE_DROP {
            return;
        }
        // The ImGui context goes first: imgui-rs allows one current context per process, and the
        // next open creates a new one.
        if let Some(mut gfx) = self.gfx.take() {
            drop(gfx.ui.take());
            crate::log(
                "panel: device released after being closed for a minute (rebuilt on next open)",
            );
        }
        self.idle_since = None;
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
        // The cursor hooks exist for the panel: they go in with it and come out with it.
        crate::pointer::engage_cursor(true);
        if self.gfx.is_some() {
            return true;
        }
        let mut rect = RECT::default();
        let _ = unsafe { GetClientRect(self.hwnd, &mut rect) };
        let width = (rect.right - rect.left).max(1) as u32;
        let height = (rect.bottom - rect.top).max(1) as u32;

        let built_at = Instant::now();
        // A device, and only a device. The panel used to build its swap chain here and `Present`
        // into it; that single call is what killed ERD Tools when the panel opened (see
        // `surface.rs`), so the swap chain is gone and the pixels travel through a DIB instead.
        let levels: [D3D_FEATURE_LEVEL; 3] = [
            D3D_FEATURE_LEVEL_11_0,
            D3D_FEATURE_LEVEL_10_1,
            D3D_FEATURE_LEVEL_10_0,
        ];
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        let created = unsafe {
            D3D11CreateDevice(
                None::<&IDXGIAdapter>,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        };
        let (device, context) = match (created, device, context) {
            (Ok(()), Some(device), Some(context)) => (device, context),
            (Err(e), ..) => {
                crate::log(&format!("panel: D3D11 device creation failed: {e}"));
                return false;
            }
            _ => {
                crate::log("panel: D3D11 returned incomplete objects");
                return false;
            }
        };
        let device_ms = built_at.elapsed().as_millis();
        let surface_at = Instant::now();
        let surface = match unsafe { crate::surface::Surface::new(&device, width, height) } {
            Ok(surface) => surface,
            Err(e) => {
                crate::log(&format!("panel: no surface: {e}"));
                return false;
            }
        };

        let surface_ms = surface_at.elapsed().as_millis();
        let ui_at = Instant::now();
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

        // Open latency is user-visible ("a slight lag when it opens"), so the build reports what it
        // cost and where: a device, the surface, or the font atlas. Guessing which is the slow part
        // has cost a round trip before.
        crate::log(&format!(
            "panel: D3D11 device + ImGui renderer created at {:.0}% ({width}x{height}); no swap \
             chain; built in {} ms (device {device_ms}, surface {surface_ms}, ui {})",
            scale * 100.0,
            built_at.elapsed().as_millis(),
            ui_at.elapsed().as_millis()
        ));
        let mut gfx = Gfx {
            device,
            context,
            surface,
            view: None,
            size: (width, height),
            ui: Some(ui),
            last_frame: Instant::now(),
            presented: false,
            last_report: Instant::now(),
            last_present: Instant::now(),
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
            // Where the window ended up, so a drag that silently fails to move it is visible in the
            // log instead of only in the player's report.
            if let Some((x, y)) = window_pos(self.hwnd) {
                crate::log(&format!("panel: drag ends at window {x},{y}"));
            }
            // Our own drag moves the window with `SetWindowPos`, which never produces the
            // `WM_EXITSIZEMOVE` the other path relies on: without this the panel would open at the
            // old spot next time even though the player had moved it.
            self.note_window_geometry();
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

    /// Write the report, open the issue page, then reveal the folder holding the file.
    ///
    /// One function because three callers need exactly this sequence: the button, the end of a
    /// bundle run, and anything else that decides a report is due. The order is the point - the page
    /// first, then the folder a moment later, or the file manager covers the page the player is
    /// meant to drag the file into.
    fn publish_report(&mut self, reason: &str) {
        match crate::diag::write_bundle(reason) {
            Ok((path, size)) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.note = format!(
                    "wrote {name} ({} KB). The folder opens; drag it onto the issue page.",
                    size / 1024
                );
                if !open_url(&crate::diag::issue_url()) {
                    self.note = format!(
                        "wrote {name} ({} KB), but the browser did not open. The issue page is {}",
                        size / 1024,
                        crate::diag::ISSUE_PAGE
                    );
                }
                // The page needs a moment to come up before the folder appears over it.
                self.reveal = Some(path);
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), REVEAL_TIMER_ID);
                    SetTimer(Some(self.hwnd), REVEAL_TIMER_ID, 1200, None);
                }
            }
            Err(e) => self.note = format!("{reason} failed: {e}"),
        }
    }

    fn set_open(&mut self, open: bool) {
        // Opening and closing are the two moments the player feels, so they are timed and reported
        // when they are slow enough to notice. "A slight lag on open and close" is a report about
        // these two numbers, and nothing else in the log says them.
        let began = Instant::now();
        // A guided run measures the game with the panel out of the way: it owns the input, the
        // cursor and a D3D11 device, and one report showed the panel reopening 25 times inside a
        // nine-minute run - which meant the run measured a game that had a window in front of it for
        // a large share of the phases. The panel does not open while a run is going, and it says so.
        if open && crate::bisect::is_running() {
            crate::log("panel: not opening - a bundle run is in progress (it measures the game without it)");
            return;
        }
        if open && !self.init_gfx() {
            crate::log("panel: no panel graphics; staying closed");
            return;
        }
        if open {
            // The device survives between opens, so its last present can be minutes old: without
            // this the watchdog below would rebuild a perfectly good swap chain on every open.
            if let Some(gfx) = self.gfx.as_mut() {
                gfx.last_present = Instant::now();
            }
        }
        if open {
            // What the game holds, at the moment the player starts arranging slots: if the memory
            // menu and this list ever disagree, every conclusion about "the mod replaced my spell"
            // is being drawn from the wrong array.
            crate::game::log_entries();
        }
        self.shown = open;
        self.model.panel.set_visible(open);
        unsafe {
            if open {
                // Read the foreground window *before* showing ourselves: showing a topmost popup
                // makes it the foreground window under Wine, and then this would adopt the panel
                // itself — which is why the subclass never ran in the last session.
                let previous = GetForegroundWindow();
                if !previous.0.is_null() && previous != self.hwnd && previous != GetDesktopWindow()
                {
                    // The window we are about to take focus from is the game's; give it a cursor so
                    // the pointer is visible over it while the panel is open.
                    crate::pointer::adopt_game_window(previous.0 as usize);
                    self.previous_foreground = Some(previous);
                }
                // Showing without activation was measured against the same crash and changed
                // nothing (it moved the fault to another offset), so activation stays: the panel
                // needs the keyboard, and stealing focus is not what breaks the other overlay.
                let _ = ShowWindow(self.hwnd, SW_SHOW);
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
            // Opening the panel shows which weapon is in hand, so it cannot answer from a pair cached
            // for the last cast. The invalidation is inline because it is one relaxed store and the
            // reload below reads it immediately; the *read* that follows is the dispatcher's job
            // (`PANEL_OPEN`), on the game's thread, so this thread never walks game memory for the
            // selected spell.
            crate::game::invalidate_weapon_cache();
            crate::dispatch::mark(crate::dispatch::PANEL_OPEN);
            self.model.reload();
        }
        let spent = began.elapsed().as_millis();
        if !open {
            CLOSED_AT.store(
                crate::logging::uptime_ms() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
        }
        let what = if open { "open" } else { "close" };
        crate::log(&format!("panel: {what} in {spent} ms"));
    }

    /// Re-read the memorised list once a second while the panel is open.
    ///
    /// The list changes at a grace and when a talisman changes the slot count, and both happen while
    /// this window can be up. Without this the rows would describe the set the panel opened with,
    /// which is exactly the "the panel says attuned and it is not" report. One verified struct walk
    /// per second, on the panel thread, only while the panel is open - and no redraw of the lists
    /// unless the set actually changed.
    fn poll_attuned(&mut self) {
        if self.model.balance != Balance::Attuned {
            return;
        }
        if self.attuned_at.elapsed().as_millis() < 1000 {
            return;
        }
        self.attuned_at = Instant::now();
        crate::game::refresh_attuned();
        let fresh = crate::game::attuned_snapshot();
        if fresh != self.model.attuned {
            self.model.attuned = fresh;
            // Both hands' pickers are built from this set, so they are rebuilt, not just redrawn.
            self.model.candidates_key.clear();
        }
    }

    fn draw(&mut self, ui: &Ui) {
        // The mode can change under an open panel: the toggle here, or a hand edit picked up by the
        // ini watcher. The gate always reads the live value, so a panel that kept its own copy would
        // show one mode while casting obeyed the other - the debug_log lesson, again.
        // The pair can change under an open panel (a weapon swap, or the panel opening before the
        // game has published the hands at all). `weapon_ids` is cached, so this is a compare.
        if let Some(pair) = crate::game::weapon_ids() {
            if pair != self.model.weapons {
                self.model.weapons = pair;
                self.model.port_class = crate::overlay::port_class(pair);
                self.model.candidates_key.clear();
                self.remeasure = true;
            }
        }
        let live = crate::config::balance();
        if live != self.model.balance {
            self.model.balance = live;
            self.model.candidates_key.clear();
            self.remeasure = true;
        }
        self.poll_attuned();
        // Read before the model borrow: the combo shows the scale auto picks for this window.
        let auto_scale = self.auto_scale();
        let current_scale = self.ui_scale;
        let model = &mut self.model;
        let mut hand_action: Option<HandAction> = None;
        let mut balance_toggle: Option<Balance> = None;
        let mut skip_toggle: Option<bool> = None;
        let mut close_clicked = false;
        let mut scale_choice: Option<Option<f32>> = None;
        let mut reset_layout = false;
        let mut diagnostics = false;
        let mut bundle_prompt = self.bundle_prompt;
        let mut bundle_started = false;
        // Where the layout ended up vertically, measured at the end of the frame.
        let mut used_height = 0.0f32;

        // Recomputed each frame so the search box takes effect immediately.
        model.refresh_candidates();
        let candidates = Rc::clone(&model.candidates);
        let candidates_right = Rc::clone(&model.candidates_right);

        // The window sits a border's width inside the display, so the frame it draws is always
        // inside the render target: an edge at the very last pixel is what disappeared when the
        // panel was dragged small.
        let display = ui.io().display_size;
        let inset = border_width(ui);
        let size = [
            (display[0] - 2.0 * inset).max(1.0),
            (display[1] - 2.0 * inset).max(1.0),
        ];
        imgui::Window::new("nr-casting")
            // No scrollbars: a panel that scrolls in two directions while its text rescales is
            // unreadable, and the scroll state is what the content overflow was doing to the frame.
            // Content that does not fit is clipped — the answer is to resize the panel.
            .scroll_bar(false)
            .position([inset, inset], Condition::Always)
            .size(size, Condition::Always)
            .title_bar(false)
            .resizable(false)
            .movable(false)
            .collapsible(false)
            .scroll_bar(false)
            .build(ui, || {
                ui.text("nr-casting");
                ui.separator();
                // The attuned column exists only in balanced mode: in free mode there is nothing
                // for it to act on, and the width it would take is width the two hands need. One
                // branch on the layout, not a condition sprinkled through the rows.
                let balanced = model.balance == Balance::Attuned;
                ui.columns(if balanced { 4 } else { 3 }, "layout", false);

                if balanced {
                    // ---- attuned -----------------------------------------------------------
                    // The notice is about the hand that can cast: the right hand unless it is not a
                    // catalyst at all, because a seal in the left can still use the spells listed.
                    let right = model.panel.catalyst_schools(model.weapons.1);
                    let schools = if right != nr_casting_core::CatalystSchools::NONE {
                        right
                    } else {
                        model.panel.catalyst_schools(model.weapons.0)
                    };
                    attuned_column(ui, &model.panel, &model.attuned, schools);
                    ui.next_column();
                }

                // ---- left hand -------------------------------------------------------------
                let left_states = row_states(&model.panel, model.weapons.0, &model.attuned, balanced);
                if let Some(action) = hand_column(
                    ui,
                    "LEFT HAND",
                    model.weapons.0,
                    [0, 1, 2, 3],
                    "left",
                    &model.panel,
                    &candidates,
                    crate::input::y_bumper(),
                    &left_states,
                    model.port_class.as_deref().unwrap_or("class port"),
                ) {
                    hand_action = Some(action);
                }

                ui.next_column();

                // ---- middle ----------------------------------------------------------------
                // The name is the file the panel edits. The lines under it are the game's opinion,
                // greyed like the disabled rows in the hand columns so they read as information
                // rather than as settings. Spacing between the lines only — no margins around the
                // block, which just pushed the whole column down.
                ui.text(format!("Character: {}", model.who));
                vspace(ui, 5.0);
                ui.text_disabled(&model.stats);
                if !model.boost.is_empty() {
                    vspace(ui, 5.0);
                    ui.text_disabled(&model.boost);
                }
                vspace(ui, 10.0);
                ui.set_next_item_width(ui.content_region_avail()[0]);
                ui.input_text("##search", &mut model.search)
                    .hint("search spells by name")
                    .build();
                vspace(ui, 5.0);
                ui.text_disabled(&model.listed);
                if balanced {
                    // Three figures, each naming its scope: what the memory holds, what these two
                    // catalysts assign, and how much of that will not cast.
                    vspace(ui, 5.0);
                    ui.text_disabled(balance_summary(&model.attuned, &model.pair_rows()));
                }
                let mut show = model.show_all;
                if ui.checkbox("list everything", &mut show) {
                    model.show_all = show;
                    model.panel.set_show_all(show);
                    // Doubling the list is a layout change: the window is fitted to it again.
                    self.remeasure = true;
                }

                ui.next_column();

                // ---- right hand ------------------------------------------------------------
                let right_states =
                    row_states(&model.panel, model.weapons.1, &model.attuned, balanced);
                if let Some(action) = hand_column(
                    ui,
                    "RIGHT HAND",
                    model.weapons.1,
                    [4, 5, 6, 7],
                    "right",
                    &model.panel,
                    &candidates_right,
                    crate::input::y_bumper(),
                    &right_states,
                    model.port_class.as_deref().unwrap_or("class port"),
                ) {
                    hand_action = Some(action);
                }

                ui.columns(1, "footer", false);
                // ---- controls --------------------------------------------------------------
                // A section like the help text below it: a separator, the row, a separator. No
                // frame of its own - the extra box only made the panel look smaller.
                ui.separator();
                // The panel's own settings get a row of their own, above the buttons: sharing one
                // line with four buttons made a row that ran out of width on a narrower panel and
                // read as one long sentence of controls.
                vspace(ui, SECTION_GAP);
                // Always visible: hiding the switch that gets you out of the mode would be a trap.
                // Flipping it applies immediately, in both directions.
                let mut balanced_box = model.balance == Balance::Attuned;
                if ui.checkbox("balanced", &mut balanced_box) {
                    balance_toggle = Some(if balanced_box {
                        Balance::Attuned
                    } else {
                        Balance::Free
                    });
                }
                ui.same_line();
                // Independent of balanced mode on purpose: it changes what the game's own wheel
                // does, not what the panel accepts, and a player may want one without the other.
                // Read live rather than from the model: a hand edit to the ini arrives through the
                // watcher, and a checkbox that disagreed with the file for a frame is a bug report.
                let mut skip_box = crate::config::skip_assigned();
                if ui.checkbox("skip assigned in wheel", &mut skip_box) {
                    skip_toggle = Some(skip_box);
                }
                ui.same_line();
                ui.text("UI scale");
                ui.same_line();
                if let Some(choice) = ui_scale_combo(ui, current_scale, auto_scale) {
                    scale_choice = Some(choice);
                }
                // Space above *and* below the settings row, so it reads as its own band rather than
                // as the first half of the buttons' row - the same distance as every other section.
                vspace(ui, SECTION_GAP);
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
                // Back to the size and spot a first run would use: the way to see the first-open
                // layout again (and to recover from a panel dragged off the screen) without
                // hand-editing the ini.
                if ui.button("reset layout") {
                    reset_layout = true;
                }
                ui.same_line();
                if ui.button("close") {
                    // Handled after the frame: closing mid-frame would drop the state.
                    close_clicked = true;
                }
                // The reports get their own row: they are about handing over evidence, not about
                // editing an assignment, and one of them is disabled for minutes at a time.
                let report = crate::panel_report::draw(ui, bundle_prompt);
                bundle_prompt = report.prompt;
                diagnostics = report.diagnostics;
                bundle_started = report.start_run;
                if !self.note.is_empty() {
                    vspace(ui, 4.0);
                    // Wrapped, like the help lines: an unwrapped status line runs off the panel
                    // edge exactly when it carries a long file name.
                    help_line(ui, &self.note);
                }
                vspace(ui, 10.0);
                ui.separator();
                help_line(
                    ui,
                    "Pad:  R1/R2 = right hand,  L1/L2 = left hand,  Y + R2 / Y + L2 = third spell,  \
                     Y + R1 / Y + L1 = fourth spell (unless Recluse owns it).",
                );
                help_line(
                    ui,
                    "Mouse:  LMB = R1,  RMB = L1,  Shift+LMB = R2,  Shift+RMB = L2,  \
                     Ctrl+LMB = Y+R2,  Ctrl+RMB = Y+L2,  Alt+LMB = Y+R1,  Alt+RMB = Y+L1.",
                );
                help_line(ui, "F8 opens and closes this panel;  Esc closes it.");
                help_line(
                    ui,
                    "Bind:  hold L3 and press the input you want (Super + click on mouse/keyboard).",
                );
                help_line(
                    ui,
                    "Drag the title band to move the panel here; the window edges resize it.",
                );
                help_line(
                    ui,
                    "Bug reports: the diagnostics button writes one file (log, GPU, CPU, modules, \
                     frame times) and opens the issue page. No ini setting needed.",
                );
                used_height = ui.cursor_pos()[1] + ui.clone_style().window_padding[1];
                // The same layout at 100%, so the auto scale can compare a window to the content it
                // holds rather than to a design constant the layout no longer matches.
                //
                // Latched, not live: a value that changes with the scale closes the loop — scale
                // changes the content, the content changes the scale — and the panel ends up
                // flickering between two sizes. It only moves when the content itself does, as when
                // "list everything" doubles the list.
                let scale_now = (ui.text_line_height() / BASE_FONT_PX).max(0.1);
                let measured = used_height / scale_now;
                // Re-measured when the *layout* changes on purpose - the first open, a scale the
                // player picked, "reset layout", "list everything" - and never merely because the
                // content grew. A prompt, a status line or a longer tooltip is transient: latching
                // it moved the scale, and the panel resized itself under a button the player had
                // only just clicked.
                if (self.needs_fit || self.remeasure) && measured > 0.0 {
                    self.natural_height = measured;
                    self.remeasure = false;
                }
                // The frame, on the window's own draw list at the window's own rect: both are
                // inside the render target by construction, so no size can clip an edge.
                let thickness = border_width(ui);
                let border = ui.clone_style().colors[StyleColor::Border as usize];
                let half = thickness * 0.5;
                ui.get_window_draw_list()
                    .add_rect(
                        [inset + half, inset + half],
                        [inset + size[0] - half, inset + size[1] - half],
                        border,
                    )
                    .thickness(thickness)
                    .build();
            });

        // The window's minimum height follows what the layout used, so the panel can be dragged
        // down to the content instead of being forced to keep a designed-in empty band.
        self.content_height = used_height;
        // First open: nobody has sized this panel by hand, so it opens exactly as tall as its
        // content instead of at the design height with a band of empty space under the last line.
        if self.needs_fit && used_height > 0.0 {
            self.needs_fit = false;
            self.pending_resize = Some(Resize::Fit(used_height));
        }
        // The prompt is App state: it has to survive the frame that set it.
        self.bundle_prompt = bundle_prompt;
        if bundle_started {
            if crate::bisect::start() {
                // The panel has to be closed: while it is open it owns the input and the GPU
                // device, and the run is measuring the game without it.
                self.close_next_frame = true;
            } else {
                // The prompt is still on screen, so say why instead of leaving a dead button.
                self.note =
                    "could not start: nothing is feeding the frame clock (load a character first)"
                        .to_string();
            }
        }
        if diagnostics {
            self.publish_report("panel button");
        }
        if reset_layout {
            // Nothing is moved here: `SetWindowPos` delivers `WM_SIZE` synchronously and this frame
            // still holds `App`. The frame loop does it once the borrow is released.
            self.saved_size = (0, 0);
            self.saved_pos = (0, 0);
            self.pending_save = None;
            self.needs_fit = true;
            self.pending_center = true;
            crate::config::save_panel_geometry(None, None);
            crate::log("panel: layout reset (size and position back to the first-open defaults)");
        }

        if close_clicked {
            self.set_open(false);
        }
        if let Some(action) = hand_action {
            let performed = match action {
                HandAction::Customise(weapon) => {
                    crate::store_customise(weapon).then_some("this copy now has its own layout")
                }
                HandAction::ResetCopy(weapon) => {
                    crate::store_reset(weapon).then_some("back to the default for this weapon type")
                }
                HandAction::Assign(index, spell_id) => {
                    let slot = SLOTS[index];
                    let weapon = self.model.weapon_of(slot);
                    // Clearing is always allowed, in every mode: the gate exists to stop a spell
                    // being assigned that cannot be cast, and "nothing" can always be cast (the
                    // press passes through to the game).
                    if spell_id == EMPTY_SLOT {
                        self.model.panel.assign(weapon, slot.weapon_slot, EMPTY_SLOT);
                        Some("slot cleared")
                    } else if self.model.balance == Balance::Attuned
                        && !self.model.attuned.contains(&spell_id)
                    {
                        self.note = format!(
                            "{} is not attuned - attune it in the game's spell menu first",
                            self.model.panel.spell_name(spell_id)
                        );
                        None
                    } else {
                        self.model.panel.assign(weapon, slot.weapon_slot, spell_id);
                        Some("")
                    }
                }
            };
            if let Some(note) = performed {
                if !note.is_empty() {
                    self.note = note.to_string();
                }
                self.model.reload();
                // Deliberately *not* `remeasure`: picking a spell does not change the layout - every
                // row is still one line - but re-measuring re-latches the natural height from a
                // frame whose text was mid-update, which is what made the panel step by a couple of
                // pixels every time a slot was clicked, including when the same spell was chosen
                // again. Height is re-latched on layout events only: first open, scale, reset,
                // "list everything".
            }
        }
        if let Some(mode) = balance_toggle {
            // Writes the ini and publishes the mode as one atomic: no reload, so no leaked
            // `Config` per click, and the next press still sees it.
            crate::config::save_balance(mode);
            self.model.balance = mode;
            self.model.reload();
            self.remeasure = true;
            let stale = self.model.stale_rows();
            self.note = match mode {
                Balance::Attuned => format!(
                    "balanced mode on: only attuned spells can be assigned ({stale} row(s) went dormant)"
                ),
                Balance::Free => "balanced mode off: every assignment is live again".to_string(),
            };
            crate::log(&format!("panel: {}", self.note));
        }
        if let Some(choice) = scale_choice {
            self.set_ui_scale(choice);
        }
        if let Some(on) = skip_toggle {
            crate::config::save_skip_assigned(on);
            self.note = if on {
                "the spell wheel will skip the spells this panel assigns".to_string()
            } else {
                "the spell wheel cycles every attuned spell again".to_string()
            };
            crate::log(&format!("panel: {}", self.note));
        }
    }

    fn render(&mut self) {
        // A scale change rebuilds the ImGui context and its font atlas, which cannot happen while a
        // frame is being built — so it is queued by the drag, by the ini, or by the panel's combo
        // and applied here, at the top of the next frame.
        if self.close_next_frame {
            self.close_next_frame = false;
            self.set_open(false);
            return;
        }
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

        // The client rect read here, at frame time, is the only size that is reliably correct: the
        // lparam of `WM_SIZE` and even `GetClientRect` can lag a modal sizing loop (imgui#8731), and
        // a frame drawn at one size into a buffer of another is exactly a panel whose border walks
        // off the edge and whose drag stops showing. Whatever the messages said, this fixes it.
        let mut client = RECT::default();
        if unsafe { GetClientRect(self.hwnd, &mut client) }.is_ok() {
            let size = (
                (client.right - client.left).max(1) as u32,
                (client.bottom - client.top).max(1) as u32,
            );
            if size != gfx.size {
                unsafe { gfx.resize_surface(size.0, size.1) };
            }
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
                // Copy to a DIB and `BitBlt` it into the window. Deliberately *not* a swap chain:
                // a single `Present` from this window killed another overlay mod's per-frame code,
                // which is what `surface.rs` documents. The returned number is the copy's own
                // cost, reported below rather than assumed.
                //
                // The present clock no longer needs telling that this is not a game frame: there
                // is no `Present` here for it to see.
                let present = gfx.surface.present(&gfx.context, self.hwnd);
                if present.is_ok() {
                    gfx.last_present = Instant::now();
                }
                // Once a second, the numbers that decide whether a frame can be correct. A panel
                // that "loses its border" is a panel whose client, buffer and display no longer
                // agree, or one that stopped presenting; this says which.
                if gfx.last_report.elapsed().as_millis() > 1000 {
                    gfx.last_report = Instant::now();
                    crate::log(&format!(
                        "panel: {cw}x{ch} client, {bw}x{bh} buffer, scale {scale:.2}, content \
                         {content:.0}px, view {view}, present {}{copy}",
                        if present.is_ok() { "ok" } else { "FAILED" },
                        // An integer millisecond count hides everything below 1 ms, and "0 ms"
                        // reads as a broken counter - so say what the resolution can support.
                        copy = match present {
                            Ok(0) => " (copy under 1 ms)".to_string(),
                            Ok(ms) => format!(" ({ms} ms copy)"),
                            Err(_) => String::new(),
                        },
                        cw = (client.right - client.left).max(0),
                        ch = (client.bottom - client.top).max(0),
                        bw = gfx.size.0,
                        bh = gfx.size.1,
                        scale = gfx.scale,
                        content = self.content_height,
                        view = if gfx.view.is_some() { "ok" } else { "none" },
                    ));
                }
                if present.is_err() {
                    // A failing Present is invisible on screen: the panel keeps its last image and
                    // the window looks frozen or half-drawn. Say it once.
                    static WARNED: std::sync::atomic::AtomicBool =
                        std::sync::atomic::AtomicBool::new(false);
                    if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                        crate::log(&format!("panel: Present failed: {present:?}"));
                    }
                }
            }
        }

        self.gfx = Some(gfx);
    }
}

/// A greyed line of help text that wraps at the window's right edge.
///
/// `text_disabled` never wraps, so the long control lines ran off the side of the panel; the colour
/// is pushed by hand so wrapping and the disabled grey can both apply.
pub(crate) fn help_line(ui: &Ui, text: &str) {
    let disabled = ui.clone_style().colors[imgui::StyleColor::TextDisabled as usize];
    let _color = ui.push_style_color(imgui::StyleColor::Text, disabled);
    ui.text_wrapped(text);
}

/// A vertical gap of exactly `px`, allowing for the item spacing ImGui adds after every item.
///
/// `ui.spacing()` is one `ItemSpacing.y` (scaled with the UI), which is not what the layout wants
/// here: the character block asks for 10 px margins and 5 px between its lines, whatever the scale.
/// The gap between the panel's sections, in pixels: one number, so a row cannot end up with a gap
/// that is visibly different from the one above it. `vspace` subtracts the item spacing ImGui adds
/// by itself, so this is the whole distance between two rows.
pub(crate) const SECTION_GAP: f32 = 10.0;

pub(crate) fn vspace(ui: &Ui, px: f32) {
    let spacing = ui.clone_style().item_spacing[1];
    ui.dummy([0.0, (px - spacing).max(0.0)]);
}

/// One hand's column: the equipped catalyst, then a row per slot (label, its mouse binding, and
/// the spell picker). Returns the pick the user made, if any.
/// One thing a hand column can ask for: an assignment, or a change to which row the hand edits.
enum HandAction {
    Assign(usize, i32),
    /// Give this copy its own layout, seeded from the type's default.
    Customise(i32),
    /// Drop this copy's layout and inherit the type's default again.
    ResetCopy(i32),
}

/// The balance state of a hand's four rows, all `Empty` in free mode.
///
/// Computed from the published memorised list, the catalogue's school and the store - the same three
/// inputs the cast path uses, through the same pure `classify`, so the panel cannot disagree with
/// what a press will do.
fn row_states(panel: &Panel, weapon: i32, attuned: &[i32], balanced: bool) -> [RowState; 4] {
    let mut states = [RowState::Empty; 4];
    if !balanced || weapon <= 0 {
        return states;
    }
    let catalyst = panel.catalyst_schools(weapon);
    for (slot, state) in states.iter_mut().enumerate() {
        let spell = panel.slot_value(weapon, slot);
        if spell > 0 {
            *state = nr_casting_core::classify(spell, panel.spell_school(spell), catalyst, attuned);
        }
    }
    states
}

/// The left column: what the game currently has memorised, in wheel order.
///
/// Information only - nothing here is clickable, because the only place a spell can be attuned is
/// the game's own memory menu at a grace.
fn attuned_column(
    ui: &Ui,
    panel: &Panel,
    attuned: &[i32],
    catalyst: nr_casting_core::CatalystSchools,
) {
    ui.text("ATTUNED");
    ui.text_disabled("from the game's spell menu");
    vspace(ui, 5.0);
    let notice = nr_casting_core::balance_notice(catalyst, &schools(panel, attuned));
    if attuned.is_empty() {
        ui.text_disabled("nothing attuned yet");
        if let Some(text) = notice {
            vspace(ui, 5.0);
            help_line(ui, text);
        }
        return;
    }
    for (index, id) in attuned.iter().enumerate() {
        ui.text_disabled(format!("{}. {}", index + 1, panel.spell_name(*id)));
    }
    if let Some(text) = notice {
        vspace(ui, 5.0);
        help_line(ui, text);
    }
}

/// The catalogue's school for each id, in the same order.
fn schools(panel: &Panel, ids: &[i32]) -> Vec<nr_casting_core::School> {
    ids.iter().map(|id| panel.spell_school(*id)).collect()
}

/// `9 attuned | 8 assigned | 1 stale` - three figures that each name what they count.
fn balance_summary(attuned: &[i32], rows: &[RowState]) -> String {
    let counts = nr_casting_core::counts(rows);
    format!(
        "{} attuned | {} assigned | {} stale",
        attuned.len(),
        counts.assigned,
        counts.stale
    )
}

fn hand_column(
    ui: &Ui,
    heading: &str,
    weapon: i32,
    indices: [usize; 4],
    prefix: &str,
    panel: &Panel,
    candidates: &[(i32, String)],
    y_bumper: bool,
    states: &[RowState; 4],
    class_name: &str,
) -> Option<HandAction> {
    ui.text(heading);
    ui.text(if weapon > 0 {
        panel.weapon_name(weapon)
    } else {
        "(nothing equipped)".to_string()
    });
    // Which spells this hand can actually cast, so a short list reads as a filter and not a bug.
    let schools = panel.catalyst_schools(weapon);
    // "not applicable" for an unarmed hand or a dagger, and the four rows are not offered at all:
    // an assignment there could never cast, so the dropdown was inviting a mistake.
    ui.text_disabled(schools.label());
    let can_cast = schools.any();
    let mut chosen = None;
    for index in indices {
        let slot = SLOTS[index];
        let current = if weapon > 0 {
            panel.slot_value(weapon, slot.weapon_slot)
        } else {
            -1
        };
        let state = states
            .get(slot.weapon_slot)
            .copied()
            .unwrap_or(RowState::Empty);
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
        // A dormant row keeps its spell *and its dropdown*: the reason is printed beside it, but
        // the control is never disabled, because the whole point of greying rather than clearing is
        // that the player can immediately pick a spell that is attuned. Only the press is ignored
        // (see `bound_spell`), and only an unattuned pick is refused at assignment time.
        let dormant = state.dormant();
        ui.disabled(off_for_recluse || !can_cast, || {
            if off_for_recluse {
                // Not "off": the chord still works on the class character, it belongs to the class
                // port (default Y+RB / Y+LB, which it hides from the game while used). The class is
                // named from the port weapon in hand, because the character's ids read 0 here.
                let [why, how] = nr_casting_core::passthrough_note(class_name, slot.right_hand);
                ui.text_disabled(format!("({why})"));
                ui.text_disabled(format!("({how})"));
            } else if let Some(picked) = slot_combo(
                ui,
                &format!("##{prefix}{index}"),
                preview,
                current,
                candidates,
            ) {
                chosen = Some(HandAction::Assign(index, picked));
            }
        });
        if dormant && !off_for_recluse {
            ui.same_line();
            // The marker, not a lock: the row is still editable, and the label is what explains why
            // the input currently does nothing.
            ui.text_disabled(state.label());
        }
    }
    // Which row this hand edits: the type's default, or a layout that belongs to this copy. Without
    // this line a player cannot tell whether an edit applies to every staff of this type or to the
    // one in hand - and the difference is the whole point of the override.
    if weapon > 0 && nr_casting_core::base_weapon_id(weapon) != weapon {
        vspace(ui, 5.0);
        if crate::has_override(weapon) {
            ui.text_disabled("customised for this copy");
            if ui.button(format!("use the type default##{prefix}_reset")) {
                chosen = Some(HandAction::ResetCopy(weapon));
            }
        } else {
            ui.text_disabled("using the default for this weapon type");
            if ui.button(format!("customise this copy##{prefix}_own")) {
                chosen = Some(HandAction::Customise(weapon));
            }
        }
    }
    chosen
}

/// The panel's UI-scale picker: auto plus a few pinned multipliers.
///
/// Returns the new choice (outer `Some`) when the player picked one, `None` when nothing changed.
/// `auto` is what auto would pick right now, so the row says what it is doing.
fn ui_scale_combo(ui: &Ui, current: Option<f32>, auto: f32) -> Option<Option<f32>> {
    // Every choice is spelled as the font size it produces: a percentage means nothing on its own,
    // and "auto" is not a percentage at all — it is whatever this screen needs. The width is capped
    // so the popup cannot be wider than the panel, which is how it used to spill past the edge.
    ui.set_next_item_width(ui.frame_height() * 8.0);
    let pixels = |scale: f32| (BASE_FONT_PX * scale).round() as i32;
    let preview = match current {
        Some(scale) => format!(
            "{:.0}% - {} px##ui_scale_preview",
            scale * 100.0,
            pixels(scale)
        ),
        None => format!("auto - {} px##ui_scale_preview", pixels(auto)),
    };
    let mut chosen = None;
    if let Some(_combo) = ui.begin_combo("##ui_scale", preview) {
        if imgui::Selectable::new(format!(
            "auto - follows the window ({} px)##ui_scale_auto",
            pixels(auto)
        ))
        .selected(current.is_none())
        .build(ui)
        {
            chosen = Some(None);
        }
        for scale in UI_SCALE_CHOICES {
            let percent = (scale * 100.0).round() as i32;
            let pinned = current.is_some_and(|value| (value - scale).abs() < 0.01);
            if imgui::Selectable::new(format!(
                "{percent}% - {} px##ui_scale_{percent}",
                pixels(scale)
            ))
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
        // "none" last, and separated: every list needs a way to say "nothing", and without it a
        // slot can only be changed, never emptied. `-1` is what an empty row already means in the
        // slot file, so this writes no new concept into the store.
        ui.separator();
        if imgui::Selectable::new("none##clear")
            .selected(current <= 0)
            .build(ui)
        {
            chosen = Some(EMPTY_SLOT);
        }
    }
    chosen
}

/// What the dropdown yields for "none": the value an empty row already holds.
const EMPTY_SLOT: i32 = -1;

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

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // A panic must never unwind into the game: crossing this FFI boundary aborts the process.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        APP.with(|cell| {
            let mut guard = cell.try_borrow_mut().ok()?;
            let app = guard.as_mut()?;
            match msg {
                WM_TIMER => {
                    if wparam.0 == SAMPLER_TIMER_ID {
                        let second = crate::perf::tick();
                        // The same second feeds the guided run; when the last phase closes, the
                        // report is written without the player touching anything.
                        if crate::bisect::on_second(second) {
                            app.publish_report("bundle run");
                        }
                        return Some(LRESULT(0));
                    }
                    if wparam.0 == REVEAL_TIMER_ID {
                        let _ = unsafe { KillTimer(Some(app.hwnd), REVEAL_TIMER_ID) };
                        if let Some(path) = app.reveal.take() {
                            if !reveal(&path) {
                                app.note = format!(
                                    "the file manager did not open; the file is {}",
                                    path.display()
                                );
                            }
                        }
                        return Some(LRESULT(0));
                    }
                    None
                }
                WM_HOTKEY => {
                    if wparam.0 as i32 == HOTKEY_ID {
                        let settled = app
                            .last_toggle
                            .map(|when| when.elapsed().as_millis() > 300)
                            .unwrap_or(true);
                        if settled {
                            app.last_toggle = Some(Instant::now());
                            let open = !app.shown;
                            // Named, because one report showed the panel flapping open and shut
                            // through a whole guided run and the log could not say who asked.
                            crate::log(&format!(
                                "panel: hotkey says {} (it is {})",
                                if open { "open" } else { "close" },
                                if app.shown { "open" } else { "closed" }
                            ));
                            app.set_open(open);
                        }
                        return Some(LRESULT(0));
                    }
                    None
                }
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
                            app.input
                                .keys
                                .push((key, msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN));
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
                            crate::log(&format!(
                                "panel: drag starts at pointer {},{} from window {},{}{}",
                                point.x,
                                point.y,
                                rect.left,
                                rect.top,
                                if rect.left == 80 && rect.top == 80 {
                                    " (still at the default spot)"
                                } else {
                                    ""
                                }
                            ));
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
                        // A small constant, and nothing else.
                        //
                        // This used to be the design size at the current text scale, and because
                        // the scale follows the window that made the minimum depend on the size it
                        // was limiting: every drag clamped the window to a minimum computed from
                        // the size before the drag, so the panel appeared to hit a new minimum each
                        // time and could never be made smaller. The panel needs no minimum — content
                        // that does not fit is clipped — so this only stops a degenerate window.
                        unsafe {
                            (*info).ptMinTrackSize.x = 400;
                            (*info).ptMinTrackSize.y = 240;
                        }
                    }
                    Some(LRESULT(0))
                }
                // A resize drag has ended: the window is the size it will stay, so work out the
                // scale that size implies and queue the rebuild. Doing this per WM_SIZE would
                // rebuild the font atlas for every pixel the corner moved.
                WM_ENTERSIZEMOVE => {
                    app.sizing = true;
                    Some(LRESULT(0))
                }
                WM_PAINT => {
                    // The window manager runs a modal loop while the player drags an edge, and our
                    // frame loop does not run inside it — so the panel would keep showing its last
                    // frame while the window resized around it. Paint from here instead.
                    if app.sizing {
                        app.render();
                    }
                    None
                }
                WM_EXITSIZEMOVE => {
                    app.sizing = false;
                    // Force the window manager to recompute the non-client area and repaint it.
                    // After a drag that went below a certain size the frame stopped being drawn
                    // and with it the resize grips, which an F8 hide/show used to cure.
                    unsafe {
                        let _ = SetWindowPos(
                            app.hwnd,
                            None,
                            0,
                            0,
                            0,
                            0,
                            SWP_NOMOVE
                                | SWP_NOSIZE
                                | SWP_NOZORDER
                                | SWP_NOACTIVATE
                                | SWP_FRAMECHANGED,
                        );
                        let _ = InvalidateRect(Some(app.hwnd), None, true);
                        let _ = UpdateWindow(app.hwnd);
                    }
                    // Only the geometry is saved: the font size no longer follows the window, so
                    // ending a drag has nothing to rescale.
                    app.note_window_geometry();
                    Some(LRESULT(0))
                }
                WM_SIZE => {
                    if let Some(gfx) = app.gfx.as_mut() {
                        let width = (lparam.0 & 0xFFFF) as u32;
                        let height = ((lparam.0 >> 16) & 0xFFFF) as u32;
                        if width > 0 && height > 0 && (width, height) != gfx.size {
                            // The client rect is what the player asked for; the surface is built to
                            // exactly that, so ImGui and the buffer cannot disagree about how many
                            // pixels exist - which is what used to make a border vanish mid-drag.
                            gfx.resize_surface(width, height);
                        }
                    }
                    // Every pixel of a drag lands here, so this only restarts the quiet timer.
                    // The UI follows the new size from the frame loop (`follow_window_scale`).
                    app.note_window_geometry();
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

/// How thick the panel's frame is, in pixels, at the current text scale.
fn border_width(ui: &Ui) -> f32 {
    (ui.text_line_height() / 15.0).clamp(1.0, 3.0)
}

fn create_window() -> Option<HWND> {
    unsafe {
        let instance = GetModuleHandleW(PCWSTR::null()).ok()?;
        let class_name = wide("nr_casting_panel");
        let title = wide("nr-casting - spell slots");
        // A pinned `ui_scale` decides the size, then the one the player saved, then the screen.
        let (width, height) = initial_window_size();
        // Where the player last parked it, kept on the desktop.
        let (x, y) = initial_window_pos();
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
            x,
            y,
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
    let config = crate::config::load();
    if !config.overlay {
        crate::log("panel: disabled in nr_casting.ini (casting only)");
        return;
    }
    std::thread::spawn(|| {
        // This thread pumps its own messages; its `PeekMessageW` calls are not the game's frames.
        crate::pump::ignore_current_thread();
        let config = crate::config::load();
        let install = (config.hooks && config.pointer_hook).then(|| crate::pointer::install());
        match install.unwrap_or_else(|| Ok(())) {
            Ok(()) => crate::log("pointer: detours installed"),
            Err(e) => crate::log(&format!("pointer: FAILED to install detours: {e}")),
        }

        let Some(hwnd) = create_window() else {
            crate::log("panel: could not create the window");
            return;
        };
        // The pointer layer must recognise the panel's own window so it never swallows the capture
        // the window manager takes for a frame drag.
        crate::pointer::set_panel_window(hwnd.0 as usize);
        APP.with(|cell| *cell.borrow_mut() = Some(App::new(hwnd)));
        // A registered hotkey posts `WM_HOTKEY` to this window, so the thread can block on its
        // message queue and wake only when something happens. Polling `GetAsyncKeyState` 60 times a
        // second to notice one key was the cheapest-looking and most expensive option.
        let hotkey = config.hotkey;
        let registered = unsafe {
            RegisterHotKey(
                Some(hwnd),
                HOTKEY_ID,
                HOT_KEY_MODIFIERS(MOD_NOREPEAT),
                hotkey as u32,
            )
        }
        .is_ok();
        APP.with(|cell| {
            if let Some(app) = cell.borrow_mut().as_mut() {
                app.hotkey_registered = registered;
            }
        });
        // One wake a second, for the frame series: it is what turns a second's worth of ticks into
        // one sample, and what the report button counts down. `SetTimer` returns zero when it cannot
        // arm one (out of timers, or a window that is not ready); the poll path then advances the
        // series instead, and this line says so rather than leaving the report button disabled for
        // a whole session with nothing in the log.
        let armed = unsafe { SetTimer(Some(hwnd), SAMPLER_TIMER_ID, 1000, None) };
        if armed == 0 {
            crate::log("panel: no sampler timer; the frame series advances from the poll instead");
        }
        crate::log(if registered {
            "panel: ready (hotkey registered; the thread idles on its message queue)"
        } else {
            "panel: ready (hotkey unavailable to register — polling it instead)"
        });

        // Developer knob, not a feature: `NR_CASTING_AUTO_PANEL_MS=8000` opens the panel once, that
        // many milliseconds after the window exists, through the same `set_open` path the hotkey
        // uses. It exists because reproducing another mod's crash needs our panel up, and there is
        // no way to press a key from outside the process. Undocumented on purpose: a player has no
        // use for it, and an ini switch nobody can explain is worse than an env var nobody sees.
        let mut auto_open_at = std::env::var("NR_CASTING_AUTO_PANEL_MS")
            .ok()
            .and_then(|ms| ms.trim().parse::<u64>().ok())
            .map(|ms| Instant::now() + std::time::Duration::from_millis(ms));

        let mut msg = MSG::default();
        loop {
            if auto_open_at.is_some_and(|at| Instant::now() >= at) {
                auto_open_at = None;
                APP.with(|cell| {
                    if let Some(app) = cell.borrow_mut().as_mut() {
                        crate::log("panel: auto-open (NR_CASTING_AUTO_PANEL_MS)");
                        app.set_open(true);
                    }
                });
            }
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
                    // Only when the hotkey could not be registered: otherwise `WM_HOTKEY` does it.
                    let down = !app.hotkey_registered && unsafe { GetAsyncKeyState(app.hotkey) } < 0;
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
                    // A panel that has not reached the screen for a second is frozen: the swap
                    // chain is stuck, and the old picture stays where it was — which reads as a
                    // panel that lost its border and refuses to be dragged. Rebuild it here rather
                    // than waiting for the player to hide and reopen the panel.
                    if app.shown && !minimized {
                        let stale = app
                            .gfx
                            .as_ref()
                            .is_some_and(|gfx| gfx.last_present.elapsed().as_millis() > 1000);
                        if stale {
                            crate::log(
                                "panel: no frame presented for a second — rebuilding the swap chain",
                            );
                            app.gfx = None;
                            app.init_gfx();
                        }
                    }
                    if app.shown && !minimized {
                        // Before drawing: a live resize should already be at the new scale.
                        // Before drawing: a live resize should already be at the new scale.
                        app.follow_window_scale();
                        app.render();
                    }
                    // Outside the `shown` check: closing the panel right after a resize must
                    // still save the geometry.
                    app.flush_pending_geometry();
                    // Taken out here and run below, outside the borrow: the resize delivers
                    // window messages, and the window procedure cannot borrow `App` twice.
                    let center = std::mem::take(&mut app.pending_center);
                    let resize = app.pending_resize.take();
                    (resize.is_some() || center).then_some((app.hwnd, resize, center))
                })
            }));
            if let Ok(Some((hwnd, resize, center))) = frame {
                if center {
                    center_window(hwnd);
                }
                match resize {
                    Some(Resize::Design(scale)) => size_window(hwnd, scale),
                    Some(Resize::Fit(height)) => fit_window_height(hwnd, height),
                    None => {}
                }
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

            // 60 Hz only while the panel is up. Closed, this thread exists to notice a hotkey:
            // waking 60 times a second to read one key state competes with the game for nothing.
            // Idle means nothing to draw and nothing owed to the ini. With the hotkey registered
            // the thread blocks on its message queue — no timer, no wakeups, and `WM_HOTKEY` wakes
            // it immediately. Without it, poll the key at 100 ms, which is still prompt to a human
            // and 6x cheaper than the 16 ms loop this used to run while closed.
            let (idle, owed, registered, resident) = APP.with(|cell| {
                cell.try_borrow()
                    .map(|guard| {
                        guard.as_ref().map_or((true, false, false, false), |app| {
                            (
                                !app.shown,
                                app.pending_save.is_some() || app.pending_resize.is_some(),
                                app.hotkey_registered,
                                // A device still resident means this thread owes the release check,
                                // so it must keep waking: `WaitMessage` would sleep until the next
                                // hotkey and never get there.
                                app.gfx.is_some(),
                            )
                        })
                    })
                    .unwrap_or((true, false, false, false))
            });
            if idle {
                // Nothing is drawn while closed; if a device is still resident from the last open,
                // release it after a minute of not being needed.
                APP.with(|cell| {
                    if let Ok(mut guard) = cell.try_borrow_mut() {
                        if let Some(app) = guard.as_mut() {
                            app.drop_gfx_if_idle();
                            // Nothing of ours needs the cursor while the panel is closed.
                            crate::pointer::engage_cursor(false);
                        }
                    }
                });
            }
            if !idle || owed {
                std::thread::sleep(std::time::Duration::from_millis(16));
            } else if registered && !resident {
                let _ = unsafe { WaitMessage() };
            } else {
                // No timer, no 16 ms loop: one wake a second is all a release check needs.
                std::thread::sleep(std::time::Duration::from_millis(1000));
            }
        }
    });
}
