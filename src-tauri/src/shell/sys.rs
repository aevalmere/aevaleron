//! What the Tauri plugins did, in plain Win32: the clipboard, opening a
//! link in the browser, the system theme, DPI awareness.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::LPARAM;
use windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext;
use windows_sys::Win32::UI::Shell::ShellExecuteW;

/// The clipboard's text, if it holds any.
pub fn clipboard_read() -> Option<String> {
    crate::voice::insert::clipboard_text()
}

pub fn clipboard_write(text: &str) -> bool {
    crate::voice::insert::set_clipboard_text(text).is_ok()
}

/// Opens `url` with the default handler (the browser for http and https).
pub fn open_url(url: &str) -> bool {
    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let target: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: NUL-terminated strings that outlive the call.
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
        )
    };
    // Values above 32 mean success.
    r as isize > 32
}

/// Opens a folder in Explorer.
pub fn open_path(path: &std::path::Path) -> bool {
    open_url(&path.to_string_lossy())
}

/// Per-monitor DPI awareness (v2), as Tauri set it.
pub fn dpi_aware() {
    // SAFETY: a process-wide setting, made once before any window exists.
    // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 is -4.
    unsafe { SetProcessDpiAwarenessContext(-4isize as _) };
}

static DARK: AtomicBool = AtomicBool::new(false);
static READ: AtomicBool = AtomicBool::new(false);

fn read_dark() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    let key = winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    match key.and_then(|k| k.get_value::<u32, _>("AppsUseLightTheme")) {
        Ok(v) => v == 0,
        Err(_) => false,
    }
}

/// Whether Windows is set to dark mode for apps.
pub fn dark_mode() -> bool {
    if !READ.swap(true, Ordering::SeqCst) {
        DARK.store(read_dark(), Ordering::SeqCst);
    }
    DARK.load(Ordering::SeqCst)
}

/// The window background behind every page before it draws (COLORREF,
/// 0x00BBGGRR): Aevalsistant's paper #FFF6F1, whatever Windows' theme
/// (the pages are paper too, ui::dark).
pub fn theme_background() -> u32 {
    0x00F1_F6FF
}

/// The frame DWM draws around every opaque window (COLORREF): the paper
/// edge #F0DDD3, Aevalsistant's card hairline. One crisp line that follows
/// the rounded corners, so paper on a white app still has an edge.
pub fn frame_border() -> u32 {
    0x00D3_DDF0
}

/// Who hears that the theme changed (ui::theme).
pub type ThemeHook = fn(bool);

static THEME_HOOK: OnceLock<ThemeHook> = OnceLock::new();

pub fn on_theme_change(f: ThemeHook) {
    let _ = THEME_HOOK.set(f);
}

/// WM_SETTINGCHANGE: "ImmersiveColorSet" means light or dark may have
/// changed.
pub fn settings_changed(lparam: LPARAM) {
    if lparam == 0 {
        return;
    }
    let p = lparam as *const u16;
    let mut name = Vec::new();
    // SAFETY: the system passes a NUL-terminated string; at most 64 units
    // are read.
    unsafe {
        for i in 0..64 {
            let u = *p.add(i);
            if u == 0 {
                break;
            }
            name.push(u);
        }
    }
    if String::from_utf16_lossy(&name) != "ImmersiveColorSet" {
        return;
    }
    let now = read_dark();
    if DARK.swap(now, Ordering::SeqCst) == now {
        return;
    }
    super::platform::theme_changed();
    if let Some(f) = THEME_HOOK.get().copied() {
        f(now);
    }
}
