use pelite::pe64::PeView;
use std::sync::LazyLock;
use windows::Win32::System::LibraryLoader::GetModuleHandleA;
use windows::core::PCSTR;

mod bundle;
mod rva_jp;
mod rva_ww;

pub use bundle::RvaBundle;

use fromsoftware_shared::game_version::{GameVersion, LANG_ID_EN, LANG_ID_JP};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ERGameVersion {
    Ww2720,
    Jp2721,
}

impl GameVersion for ERGameVersion {
    const NAME: &'static str = "elden ring";

    fn from_lang_version(lang_id: u16, version: &str) -> Option<Self> {
        match (lang_id, version) {
            (LANG_ID_EN, "2.7.2.0") => Some(Self::Ww2720),
            (LANG_ID_JP, "2.7.2.1") => Some(Self::Jp2721),
            _ => None,
        }
    }
}

impl ERGameVersion {
    const fn rvas(self) -> RvaBundle {
        match self {
            Self::Ww2720 => rva_ww::RVAS,
            Self::Jp2721 => rva_jp::RVAS,
        }
    }
}

/// Version/table information for the running executable, recorded when the RVA table is
/// first resolved. Lets callers log what was detected and whether the version was known.
#[derive(Clone, Debug)]
pub struct ResolvedInfo {
    pub table: &'static str,
    pub exe_version: String,
    pub used_fallback: bool,
}

static RESOLVED: std::sync::Mutex<Option<ResolvedInfo>> = std::sync::Mutex::new(None);

/// What `get()` decided, or `None` if the table has not been resolved yet.
pub fn resolved_info() -> Option<ResolvedInfo> {
    RESOLVED.lock().ok().and_then(|guard| guard.clone())
}

fn exe_version_string(module: &PeView) -> Option<String> {
    use pelite::pe64::Pe;
    let info = module.resources().ok()?.version_info().ok()?;
    let v = info.fixed()?.dwProductVersion;
    Some(format!("{}.{}.{}.{}", v.Major, v.Minor, v.Patch, v.Build))
}

/// Returns the RVA bundle for the current executable region and version.
///
/// An unrecognised version falls back to the newest known table instead of panicking; the
/// caller is expected to validate the structures it reads before relying on them.
pub fn get() -> &'static RvaBundle {
    static RVAS: LazyLock<RvaBundle> = LazyLock::new(|| {
        let module = unsafe {
            PeView::module(GetModuleHandleA(PCSTR(std::ptr::null())).unwrap().0 as *const u8)
        };
        let detected = exe_version_string(&module).unwrap_or_else(|| "unknown".to_string());
        // Any detection failure falls back instead of panicking: an unknown patch is the
        // common case, and the caller validates the layout before touching game memory.
        // `NR_CASTING_FORCE_RVA` exercises the fallback on a known-good build.
        let forced = std::env::var("NR_CASTING_FORCE_RVA").map(|v| v == "1").unwrap_or(false);
        let detected_result = ERGameVersion::detect(&module);
        let (choice, version, used_fallback) = if forced {
            (ERGameVersion::Ww2720, detected, true)
        } else {
            match detected_result {
                Ok(v) => (v, detected, false),
                Err(e) => (ERGameVersion::Ww2720, format!("{detected} ({e})"), true),
            }
        };
        let table = match choice {
            ERGameVersion::Ww2720 => "Ww2720",
            ERGameVersion::Jp2721 => "Jp2721",
        };
        if let Ok(mut slot) = RESOLVED.lock() {
            *slot = Some(ResolvedInfo {
                table,
                exe_version: version,
                used_fallback,
            });
        }
        choice.rvas()
    });

    &RVAS
}
