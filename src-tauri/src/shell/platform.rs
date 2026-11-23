//! Slint on plain Win32: every page window is a window of ours, created
//! here with the styles the window manager wants from the start (tool
//! window, topmost, no activation, hidden), drawn by Slint's software
//! renderer and put on screen with GDI.
//!
//! Two kinds of window:
//! - opaque: an ordinary popup with a thick frame, so DWM gives it rounded
//!   corners and its shadow (Settings, History, the answer windows, cards);
//! - layered: per-pixel alpha through UpdateLayeredWindow, for what has to
//!   be see-through (the bar and its menus, the orb, the area picker, the
//!   lens, the pointer, the voice pill). A fully transparent pixel takes no
//!   clicks, so their margins fall through to the app underneath.
//!
//! The event loop sleeps in MsgWaitForMultipleObjectsEx until a message, a
//! Slint timer, or a closure posted from another thread (`post`) is due:
//! with nothing on screen changing, the UI thread uses no CPU.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use slint::platform::software_renderer::{
    PremultipliedRgbaColor, RepaintBufferType, SoftwareRenderer, TargetPixel,
};
use slint::platform::{
    Clipboard, EventLoopProxy, Key, Platform, PointerEventButton, WindowAdapter, WindowEvent,
};
use slint::{EventLoopError, LogicalPosition, LogicalSize, PlatformError};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleDC, CreateDIBSection, CreateSolidBrush, DeleteDC,
    DeleteObject, EndPaint, FillRect, GetDC, ReleaseDC, ScreenToClient, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, BLENDFUNCTION, HBITMAP, HDC, HGDIOBJ, PAINTSTRUCT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, GetKeyState, MapVirtualKeyW, ReleaseCapture, SetCapture, SetFocus,
    TrackMouseEvent, TRACKMOUSEEVENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, IsWindow,
    IsWindowVisible, IsZoomed, LoadCursorW, MsgWaitForMultipleObjectsEx, PeekMessageW,
    PostMessageW, RegisterClassExW, SendMessageW, SetForegroundWindow, SetWindowPos, ShowWindow,
    TranslateMessage, UpdateLayeredWindow, IDC_ARROW, MSG, WNDCLASSEXW,
};

use super::Result as ShellResult;

// Messages, styles and keys as numbers: the values are fixed by Windows,
// and a number cannot move between windows-sys modules.
mod c {
    pub const WM_DESTROY: u32 = 0x0002;
    pub const WM_MOVE: u32 = 0x0003;
    pub const WM_SIZE: u32 = 0x0005;
    pub const WM_SETFOCUS: u32 = 0x0007;
    pub const WM_KILLFOCUS: u32 = 0x0008;
    pub const WM_PAINT: u32 = 0x000F;
    pub const WM_CLOSE: u32 = 0x0010;
    pub const WM_ERASEBKGND: u32 = 0x0014;
    pub const WM_SHOWWINDOW: u32 = 0x0018;
    pub const WM_SETTINGCHANGE: u32 = 0x001A;
    pub const WM_MOUSEACTIVATE: u32 = 0x0021;
    pub const WM_NCCALCSIZE: u32 = 0x0083;
    pub const WM_NCHITTEST: u32 = 0x0084;
    pub const WM_NCLBUTTONDOWN: u32 = 0x00A1;
    pub const WM_KEYDOWN: u32 = 0x0100;
    pub const WM_KEYUP: u32 = 0x0101;
    pub const WM_CHAR: u32 = 0x0102;
    pub const WM_SYSKEYDOWN: u32 = 0x0104;
    pub const WM_SYSKEYUP: u32 = 0x0105;
    pub const WM_SYSCHAR: u32 = 0x0106;
    pub const WM_MOUSEMOVE: u32 = 0x0200;
    pub const WM_LBUTTONDOWN: u32 = 0x0201;
    pub const WM_LBUTTONUP: u32 = 0x0202;
    pub const WM_RBUTTONDOWN: u32 = 0x0204;
    pub const WM_RBUTTONUP: u32 = 0x0205;
    pub const WM_MBUTTONDOWN: u32 = 0x0207;
    pub const WM_MBUTTONUP: u32 = 0x0208;
    pub const WM_MOUSEWHEEL: u32 = 0x020A;
    pub const WM_MOUSEHWHEEL: u32 = 0x020E;
    pub const WM_MOUSELEAVE: u32 = 0x02A3;
    pub const WM_DPICHANGED: u32 = 0x02E0;
    pub const WM_QUIT: u32 = 0x0012;
    pub const WM_APP: u32 = 0x8000;

    pub const WS_POPUP: u32 = 0x8000_0000;
    pub const WS_THICKFRAME: u32 = 0x0004_0000;
    pub const WS_MAXIMIZEBOX: u32 = 0x0001_0000;
    pub const WS_CLIPCHILDREN: u32 = 0x0200_0000;
    pub const WS_EX_TOPMOST: u32 = 0x0000_0008;
    pub const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
    pub const WS_EX_LAYERED: u32 = 0x0008_0000;
    pub const WS_EX_NOACTIVATE: u32 = 0x0800_0000;

    pub const HTCLIENT: isize = 1;
    pub const HTCAPTION: usize = 2;
    pub const MA_NOACTIVATE: isize = 3;

    pub const SW_HIDE: i32 = 0;
    pub const SW_MAXIMIZE: i32 = 3;
    pub const SW_RESTORE: i32 = 9;

    pub const SWP_NOZORDER: u32 = 0x0004;
    pub const SWP_NOACTIVATE: u32 = 0x0010;

    pub const PM_REMOVE: u32 = 0x0001;
    pub const QS_ALLINPUT: u32 = 0x04FF;
    pub const MWMO_INPUTAVAILABLE: u32 = 0x0004;
    pub const INFINITE: u32 = 0xFFFF_FFFF;
    pub const ULW_ALPHA: u32 = 0x0000_0002;
    pub const TME_LEAVE: u32 = 0x0000_0002;
    pub const MAPVK_VK_TO_CHAR: u32 = 2;
    pub const SRCCOPY: u32 = 0x00CC_0020;

    pub const VK_BACK: u16 = 0x08;
    pub const VK_TAB: u16 = 0x09;
    pub const VK_RETURN: u16 = 0x0D;
    pub const VK_SHIFT: u16 = 0x10;
    pub const VK_CONTROL: u16 = 0x11;
    pub const VK_MENU: u16 = 0x12;
    pub const VK_PAUSE: u16 = 0x13;
    pub const VK_CAPITAL: u16 = 0x14;
    pub const VK_ESCAPE: u16 = 0x1B;
    pub const VK_PRIOR: u16 = 0x21;
    pub const VK_NEXT: u16 = 0x22;
    pub const VK_END: u16 = 0x23;
    pub const VK_HOME: u16 = 0x24;
    pub const VK_LEFT: u16 = 0x25;
    pub const VK_UP: u16 = 0x26;
    pub const VK_RIGHT: u16 = 0x27;
    pub const VK_DOWN: u16 = 0x28;
    pub const VK_INSERT: u16 = 0x2D;
    pub const VK_DELETE: u16 = 0x2E;
    pub const VK_LWIN: u16 = 0x5B;
    pub const VK_RWIN: u16 = 0x5C;
    pub const VK_APPS: u16 = 0x5D;
    pub const VK_F1: u16 = 0x70;
    pub const VK_F24: u16 = 0x87;
    pub const VK_SCROLL: u16 = 0x91;
}

/// Our wake-up message on the hidden message window.
const WM_WAKE: u32 = c::WM_APP + 1;

fn hw(x: isize) -> HWND {
    x as HWND
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ------------------------------------------------------------ threading

static UI_THREAD: AtomicU32 = AtomicU32::new(0);
static MSG_HWND: AtomicIsize = AtomicIsize::new(0);
static WOKEN: AtomicBool = AtomicBool::new(false);
static EXIT: AtomicBool = AtomicBool::new(false);
static EXIT_CODE: AtomicI32 = AtomicI32::new(0);

type Task = Box<dyn FnOnce() + Send>;

fn queue() -> &'static Mutex<VecDeque<Task>> {
    static Q: OnceLock<Mutex<VecDeque<Task>>> = OnceLock::new();
    Q.get_or_init(Default::default)
}

pub fn is_ui_thread() -> bool {
    let t = UI_THREAD.load(Ordering::SeqCst);
    // SAFETY: no arguments.
    t != 0 && t == unsafe { GetCurrentThreadId() }
}

/// Queues `f` for the UI thread, in order, and wakes it.
pub fn post(f: Task) -> ShellResult<()> {
    queue().lock().push_back(f);
    wake();
    Ok(())
}

fn wake() {
    if WOKEN.swap(true, Ordering::SeqCst) {
        return;
    }
    let h = MSG_HWND.load(Ordering::SeqCst);
    if h != 0 {
        // SAFETY: posting to our own message window.
        unsafe { PostMessageW(hw(h), WM_WAKE, 0, 0) };
    }
}

fn drain_queue() {
    WOKEN.store(false, Ordering::SeqCst);
    // One batch at a time, so a task that posts more cannot starve input.
    let batch: Vec<Task> = queue().lock().drain(..).collect();
    for task in batch {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(task)).is_err() {
            eprintln!("[aevaleron] a UI task panicked");
        }
    }
}

/// Ends the loop; `run` returns `code`.
pub fn request_exit(code: i32) {
    EXIT_CODE.store(code, Ordering::SeqCst);
    EXIT.store(true, Ordering::SeqCst);
    wake();
}

struct Proxy;

impl EventLoopProxy for Proxy {
    fn quit_event_loop(&self) -> std::result::Result<(), EventLoopError> {
        request_exit(0);
        Ok(())
    }

    fn invoke_from_event_loop(
        &self,
        event: Box<dyn FnOnce() + Send>,
    ) -> std::result::Result<(), EventLoopError> {
        let _ = post(event);
        Ok(())
    }
}

// --------------------------------------------------------------- specs

/// How the next window is made. Set around a page's construction with
/// `create_with`.
#[derive(Clone, Debug)]
pub struct Spec {
    pub label: String,
    /// Per-pixel alpha (see the module docs); else opaque with DWM corners.
    pub layered: bool,
    /// Physical pixels.
    pub rect: (i32, i32, i32, i32),
    pub maximizable: bool,
    /// Clicks never activate it.
    pub no_activate: bool,
    pub title: String,
}

impl Default for Spec {
    fn default() -> Self {
        Spec {
            label: String::new(),
            layered: true,
            rect: (0, 0, 1, 1),
            maximizable: false,
            no_activate: false,
            title: "Aevaleron".into(),
        }
    }
}

thread_local! {
    static NEXT: RefCell<Option<Spec>> = const { RefCell::new(None) };
    static WINS: RefCell<HashMap<isize, Weak<Win>>> = RefCell::new(HashMap::new());
    static LAST: RefCell<Option<Rc<Win>>> = const { RefCell::new(None) };
    static DISPATCHING: Cell<bool> = const { Cell::new(false) };
    static PENDING: RefCell<VecDeque<(isize, WindowEvent)>> = const { RefCell::new(VecDeque::new()) };
}

/// Builds a page with `make` (a Slint component's `new`), its window made
/// from `spec`. Returns what `make` returned and the window's HWND.
pub fn create_with<T>(spec: Spec, make: impl FnOnce() -> T) -> (T, isize) {
    NEXT.with(|n| *n.borrow_mut() = Some(spec));
    let out = make();
    NEXT.with(|n| *n.borrow_mut() = None);
    let hwnd = LAST
        .with(|l| l.borrow_mut().take())
        .map(|w| w.hwnd.get())
        .unwrap_or(0);
    (out, hwnd)
}

// ------------------------------------------------------------ the window

/// A 32-bit top-down DIB section and its memory DC.
struct Surface {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    w: i32,
    h: i32,
}

impl Surface {
    fn new(w: i32, h: i32) -> Option<Surface> {
        // SAFETY: GDI calls with valid arguments; failures return null.
        unsafe {
            let screen = GetDC(std::ptr::null_mut());
            let dc = CreateCompatibleDC(screen);
            ReleaseDC(std::ptr::null_mut(), screen);
            if dc.is_null() {
                return None;
            }
            let mut bmi: BITMAPINFO = std::mem::zeroed();
            bmi.bmiHeader = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(dc, &bmi, 0, &mut bits, std::ptr::null_mut(), 0);
            if bmp.is_null() || bits.is_null() {
                DeleteDC(dc);
                return None;
            }
            let old = SelectObject(dc, bmp);
            Some(Surface {
                dc,
                bmp,
                old,
                bits: bits as *mut u32,
                w,
                h,
            })
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: the objects this surface created.
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bmp);
            DeleteDC(self.dc);
        }
    }
}

/// Information about a key press, for a page's key hook.
#[derive(Clone, Debug)]
pub struct KeyPress {
    pub vk: u16,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
    pub repeat: bool,
    /// The unshifted character the key types, lowercase, if any.
    pub ch: Option<char>,
}

pub type KeyHook = Rc<dyn Fn(&KeyPress) -> bool>;

/// One pixel as the DIB holds it: premultiplied alpha, 0xAARRGGBB, which is
/// what UpdateLayeredWindow and BitBlt read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct Bgra(pub u32);

impl TargetPixel for Bgra {
    fn blend(&mut self, c: PremultipliedRgbaColor) {
        let v = self.0;
        let keep = u32::from(255 - c.alpha);
        let ch = |shift: u32, add: u8| -> u32 {
            ((((v >> shift) & 0xFF) * keep + 127) / 255 + u32::from(add)).min(255) << shift
        };
        self.0 = ch(24, c.alpha) | ch(16, c.red) | ch(8, c.green) | ch(0, c.blue);
    }

    fn from_rgb(red: u8, green: u8, blue: u8) -> Self {
        Bgra(0xFF00_0000 | (u32::from(red) << 16) | (u32::from(green) << 8) | u32::from(blue))
    }

    /// Clear: a layered window's empty pixels let clicks through.
    fn background() -> Self {
        Bgra(0)
    }
}

pub struct Win {
    window: slint::Window,
    renderer: SoftwareRenderer,
    hwnd: Cell<isize>,
    label: String,
    layered: bool,
    size: Cell<slint::PhysicalSize>,
    scale: Cell<f32>,
    dirty: Cell<bool>,
    surface: RefCell<Option<Surface>>,
    tracking: Cell<bool>,
    buttons: Cell<u8>,
    last_pos: Cell<LogicalPosition>,
    surrogate: Cell<u16>,
    skip_char: Cell<bool>,
    key_hook: RefCell<Option<KeyHook>>,
    background: Cell<u32>,
}

impl WindowAdapter for Win {
    fn window(&self) -> &slint::Window {
        &self.window
    }

    fn size(&self) -> slint::PhysicalSize {
        self.size.get()
    }

    fn renderer(&self) -> &dyn slint::platform::Renderer {
        &self.renderer
    }

    fn request_redraw(&self) {
        self.dirty.set(true);
        wake();
    }

    // Showing and hiding belong to the window manager (windows/flags.rs),
    // which works on the HWND; Slint's own show() only records the wish.
    fn set_visible(&self, _visible: bool) -> std::result::Result<(), PlatformError> {
        Ok(())
    }

    fn position(&self) -> Option<slint::PhysicalPosition> {
        let mut r = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: our window, a valid out pointer.
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(hw(self.hwnd.get()), &mut r)
        };
        Some(slint::PhysicalPosition::new(r.left, r.top))
    }

    fn set_position(&self, position: slint::WindowPosition) {
        let p = position.to_physical(self.scale.get());
        // SAFETY: our window.
        unsafe {
            SetWindowPos(
                hw(self.hwnd.get()),
                std::ptr::null_mut(),
                p.x,
                p.y,
                0,
                0,
                0x0001 | c::SWP_NOZORDER | c::SWP_NOACTIVATE,
            )
        };
    }

    fn set_size(&self, size: slint::WindowSize) {
        let s = size.to_physical(self.scale.get());
        // SAFETY: our window.
        unsafe {
            SetWindowPos(
                hw(self.hwnd.get()),
                std::ptr::null_mut(),
                0,
                0,
                s.width as i32,
                s.height as i32,
                0x0002 | c::SWP_NOZORDER | c::SWP_NOACTIVATE,
            )
        };
    }
}

impl Drop for Win {
    fn drop(&mut self) {
        let h = self.hwnd.get();
        WINS.with(|w| {
            if let Ok(mut m) = w.try_borrow_mut() {
                m.remove(&h);
            }
        });
        super::forget_label(&self.label, h);
        // SAFETY: IsWindow accepts any value; DestroyWindow on our own.
        unsafe {
            if IsWindow(hw(h)) != 0 {
                DestroyWindow(hw(h));
            }
        }
    }
}

fn class_name() -> &'static [u16] {
    static NAME: OnceLock<Vec<u16>> = OnceLock::new();
    NAME.get_or_init(|| wide("AevaleronWindow"))
}

fn register_classes() {
    // SAFETY: registering window classes with our own procedures.
    unsafe {
        let inst = GetModuleHandleW(std::ptr::null());
        let mut wc: WNDCLASSEXW = std::mem::zeroed();
        wc.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
        wc.lpfnWndProc = Some(wndproc);
        wc.hInstance = inst;
        wc.hCursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
        wc.lpszClassName = class_name().as_ptr();
        RegisterClassExW(&wc);

        let msg_class = wide("AevaleronMessages");
        let mut mc: WNDCLASSEXW = std::mem::zeroed();
        mc.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
        mc.lpfnWndProc = Some(msgproc);
        mc.hInstance = inst;
        mc.lpszClassName = msg_class.as_ptr();
        RegisterClassExW(&mc);
        // HWND_MESSAGE: a window that only receives posted messages.
        let hwnd = CreateWindowExW(
            0,
            msg_class.as_ptr(),
            msg_class.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            -3isize as HWND,
            std::ptr::null_mut(),
            inst,
            std::ptr::null(),
        );
        MSG_HWND.store(hwnd as isize, Ordering::SeqCst);
    }
}

fn create_adapter() -> std::result::Result<Rc<Win>, PlatformError> {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let spec = NEXT.with(|n| n.borrow().clone()).unwrap_or_else(|| Spec {
        label: format!("slint-{}", SEQ.fetch_add(1, Ordering::SeqCst)),
        ..Spec::default()
    });
    let mut ex = c::WS_EX_TOPMOST | c::WS_EX_TOOLWINDOW;
    if spec.layered {
        ex |= c::WS_EX_LAYERED;
    }
    if spec.no_activate {
        ex |= c::WS_EX_NOACTIVATE;
    }
    let mut style = c::WS_POPUP | c::WS_CLIPCHILDREN;
    if !spec.layered {
        // The frame is what DWM rounds and shadows; WM_NCCALCSIZE hides it.
        style |= c::WS_THICKFRAME;
    }
    if spec.maximizable {
        style |= c::WS_MAXIMIZEBOX;
    }
    let title = wide(&spec.title);
    let (x, y, w, h) = spec.rect;
    // SAFETY: our registered class; the window starts hidden.
    let hwnd = unsafe {
        CreateWindowExW(
            ex,
            class_name().as_ptr(),
            title.as_ptr(),
            style,
            x,
            y,
            w.max(1),
            h.max(1),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        )
    } as isize;
    if hwnd == 0 {
        return Err(PlatformError::Other("CreateWindowExW failed".into()));
    }
    if !spec.layered {
        dwm_frame(hwnd);
    }
    let scale = dpi_scale(hwnd);
    let size = client_size(hwnd);
    let win = Rc::new_cyclic(|weak: &Weak<Win>| {
        let adapter: Weak<dyn WindowAdapter> = weak.clone();
        Win {
            window: slint::Window::new(adapter),
            renderer: SoftwareRenderer::new_with_repaint_buffer_type(RepaintBufferType::NewBuffer),
            hwnd: Cell::new(hwnd),
            label: spec.label.clone(),
            layered: spec.layered,
            size: Cell::new(size),
            scale: Cell::new(scale),
            dirty: Cell::new(true),
            surface: RefCell::new(None),
            tracking: Cell::new(false),
            buttons: Cell::new(0),
            last_pos: Cell::new(LogicalPosition::new(0.0, 0.0)),
            surrogate: Cell::new(0),
            skip_char: Cell::new(false),
            key_hook: RefCell::new(None),
            background: Cell::new(super::sys::theme_background()),
        }
    });
    WINS.with(|m| m.borrow_mut().insert(hwnd, Rc::downgrade(&win)));
    LAST.with(|l| *l.borrow_mut() = Some(win.clone()));
    super::register_label(&spec.label, hwnd);
    // The first scale and size, once the component that owns this window
    // exists.
    let _ = post(Box::new(move || {
        if let Some(w) = find(hwnd) {
            let s = w.scale.get();
            w.dispatch(WindowEvent::ScaleFactorChanged { scale_factor: s });
            let sz = w.size.get();
            w.dispatch(WindowEvent::Resized {
                size: LogicalSize::new(sz.width as f32 / s, sz.height as f32 / s),
            });
            w.dirty.set(true);
        }
    }));
    Ok(win)
}

/// Windows 11 rounds and shadows the opaque windows and draws their one
/// border: a paper-edge hairline (sys::frame_border), on a light frame to
/// match the paper pages.
fn dwm_frame(hwnd: isize) {
    let round: i32 = 2; // DWMWCP_ROUND
    let border: u32 = super::sys::frame_border();
    let dark: i32 = 0;
    // SAFETY: attributes with their documented sizes; unsupported ones
    // (Windows 10) fail harmlessly.
    unsafe {
        DwmSetWindowAttribute(hw(hwnd), 33, &round as *const i32 as *const _, 4);
        DwmSetWindowAttribute(hw(hwnd), 34, &border as *const u32 as *const _, 4);
        DwmSetWindowAttribute(hw(hwnd), 20, &dark as *const i32 as *const _, 4);
    }
}

fn dpi_scale(hwnd: isize) -> f32 {
    if let Some(s) = crate::windows::screen::forced_scale() {
        return s as f32;
    }
    // SAFETY: our window.
    let dpi = unsafe { GetDpiForWindow(hw(hwnd)) };
    if dpi == 0 {
        1.0
    } else {
        dpi as f32 / 96.0
    }
}

fn client_size(hwnd: isize) -> slint::PhysicalSize {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: our window, a valid out pointer.
    unsafe { GetClientRect(hw(hwnd), &mut r) };
    slint::PhysicalSize::new(
        (r.right - r.left).max(0) as u32,
        (r.bottom - r.top).max(0) as u32,
    )
}

fn find(hwnd: isize) -> Option<Rc<Win>> {
    WINS.with(|m| m.try_borrow().ok()?.get(&hwnd)?.upgrade())
}

fn all_windows() -> Vec<Rc<Win>> {
    WINS.with(|m| {
        m.try_borrow()
            .map(|m| m.values().filter_map(Weak::upgrade).collect())
            .unwrap_or_default()
    })
}

impl Win {
    /// Slint events never nest: one that arrives while another is being
    /// handled (a SetWindowPos inside a callback sends WM_SIZE at once)
    /// waits for it to finish.
    fn dispatch(&self, e: WindowEvent) {
        if DISPATCHING.with(Cell::get) {
            PENDING.with(|p| p.borrow_mut().push_back((self.hwnd.get(), e)));
            return;
        }
        DISPATCHING.with(|d| d.set(true));
        self.window.dispatch_event(e);
        loop {
            let next = PENDING.with(|p| p.borrow_mut().pop_front());
            let Some((h, e)) = next else {
                break;
            };
            if let Some(w) = find(h) {
                w.window.dispatch_event(e);
            }
        }
        DISPATCHING.with(|d| d.set(false));
    }

    fn logical(&self, lparam: LPARAM) -> LogicalPosition {
        let x = (lparam & 0xFFFF) as i16 as f32;
        let y = ((lparam >> 16) & 0xFFFF) as i16 as f32;
        let s = self.scale.get();
        LogicalPosition::new(x / s, y / s)
    }

    fn visible(&self) -> bool {
        // SAFETY: IsWindowVisible accepts any value.
        unsafe { IsWindowVisible(hw(self.hwnd.get())) != 0 }
    }

    /// Draws and presents when something changed.
    fn render(&self) {
        let animating = self.window.has_active_animations();
        if !(self.dirty.get() || animating) || !self.visible() {
            return;
        }
        let hwnd = self.hwnd.get();
        let size = client_size(hwnd);
        if size != self.size.get() {
            self.resized(size);
        }
        let (w, h) = (size.width as usize, size.height as usize);
        if w == 0 || h == 0 {
            return;
        }
        self.dirty.set(false);
        let mut surf = self.surface.borrow_mut();
        if surf.as_ref().map(|s| (s.w, s.h)) != Some((w as i32, h as i32)) {
            *surf = Surface::new(w as i32, h as i32);
        }
        let Some(s) = surf.as_ref() else {
            return;
        };
        // Slint draws straight into the DIB the OS reads: one frame in
        // memory per window, not two, and no copy.
        // SAFETY: the DIB holds w*h 32-bit pixels, and Bgra is one u32.
        let out = unsafe { std::slice::from_raw_parts_mut(s.bits.cast::<Bgra>(), w * h) };
        self.renderer.render(out, w);
        // A resize from another thread since the size was read: draw again
        // at the new size rather than put the old one back.
        if client_size(hwnd) != size {
            self.dirty.set(true);
            return;
        }
        if self.layered {
            let sz = SIZE {
                cx: w as i32,
                cy: h as i32,
            };
            let origin = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: 0,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: 1,
            };
            // SAFETY: a layered window of ours and a DC holding a w*h DIB.
            unsafe {
                UpdateLayeredWindow(
                    hw(hwnd),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    &sz,
                    s.dc,
                    &origin,
                    0,
                    &blend,
                    c::ULW_ALPHA,
                )
            };
        } else {
            // SAFETY: our window's DC, released at once.
            unsafe {
                let dc = GetDC(hw(hwnd));
                BitBlt(dc, 0, 0, w as i32, h as i32, s.dc, 0, 0, c::SRCCOPY);
                ReleaseDC(hw(hwnd), dc);
            }
        }
    }

    fn resized(&self, size: slint::PhysicalSize) {
        self.size.set(size);
        let s = self.scale.get();
        self.dispatch(WindowEvent::Resized {
            size: LogicalSize::new(size.width as f32 / s, size.height as f32 / s),
        });
        self.dirty.set(true);
    }

    /// Frees the pixels of a hidden window; showing it draws them again.
    fn release(&self) {
        *self.surface.borrow_mut() = None;
        self.dirty.set(true);
    }

    fn paint(&self) {
        let hwnd = hw(self.hwnd.get());
        // SAFETY: WM_PAINT handling on our own window.
        unsafe {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            let surf = self.surface.borrow();
            match surf.as_ref() {
                Some(s) if !self.layered => {
                    BitBlt(dc, 0, 0, s.w, s.h, s.dc, 0, 0, c::SRCCOPY);
                }
                None if !self.layered => {
                    let brush = CreateSolidBrush(self.background.get());
                    FillRect(dc, &ps.rcPaint, brush);
                    DeleteObject(brush);
                    self.dirty.set(true);
                    wake();
                }
                _ => {}
            }
            EndPaint(hwnd, &ps);
        }
    }

    fn release_modifiers(&self) {
        for k in [Key::Shift, Key::Control, Key::Alt, Key::Meta] {
            self.dispatch(WindowEvent::KeyReleased { text: k.into() });
        }
    }

    fn key_down(&self, vk: u16, lparam: LPARAM, sys: bool) -> bool {
        let repeat = lparam & (1 << 30) != 0;
        let down = |k: u16| {
            // SAFETY: reads the thread's key state.
            unsafe { GetKeyState(i32::from(k)) < 0 }
        };
        let ctrl = down(c::VK_CONTROL);
        let alt = down(c::VK_MENU);
        let shift = down(c::VK_SHIFT);
        let meta = down(c::VK_LWIN) || down(c::VK_RWIN);
        // SAFETY: a pure table lookup.
        let mapped = unsafe { MapVirtualKeyW(u32::from(vk), c::MAPVK_VK_TO_CHAR) };
        let ch = char::from_u32(mapped & 0x7FFF_FFFF)
            .filter(|c| !c.is_control())
            .map(|c| c.to_ascii_lowercase());
        self.skip_char.set(false);
        let hook = self.key_hook.borrow().clone();
        if let Some(hook) = hook {
            let press = KeyPress {
                vk,
                ctrl,
                alt,
                shift,
                meta,
                repeat,
                ch,
            };
            if hook(&press) {
                self.skip_char.set(true);
                return true;
            }
        }
        if let Some(k) = special(vk, shift) {
            let text: slint::SharedString = k.into();
            self.dispatch(if repeat {
                WindowEvent::KeyPressRepeated { text }
            } else {
                WindowEvent::KeyPressed { text }
            });
            // Alt+F4 still reaches DefWindowProc and closes as before.
            return !(sys && vk == 0x73);
        }
        // With Ctrl or Alt (not both: that is AltGr typing) the character
        // never arrives as WM_CHAR, so it goes from here.
        if ctrl != alt {
            if let Some(ch) = ch {
                let text: slint::SharedString = ch.to_string().into();
                self.dispatch(WindowEvent::KeyPressed { text: text.clone() });
                self.dispatch(WindowEvent::KeyReleased { text });
                self.skip_char.set(true);
                return true;
            }
        }
        false
    }

    fn key_up(&self, vk: u16) {
        // SAFETY: as in key_down.
        let shift = unsafe { GetKeyState(i32::from(c::VK_SHIFT)) < 0 };
        if let Some(k) = special(vk, shift) {
            self.dispatch(WindowEvent::KeyReleased { text: k.into() });
        }
    }

    fn char_in(&self, unit: u16) {
        if self.skip_char.replace(false) {
            return;
        }
        let ch = if (0xD800..0xDC00).contains(&unit) {
            self.surrogate.set(unit);
            return;
        } else if (0xDC00..0xE000).contains(&unit) {
            let hi = self.surrogate.replace(0);
            char::decode_utf16([hi, unit]).next().and_then(|r| r.ok())
        } else {
            char::from_u32(u32::from(unit))
        };
        let Some(ch) = ch.filter(|c| !c.is_control()) else {
            return;
        };
        let text: slint::SharedString = ch.to_string().into();
        self.dispatch(WindowEvent::KeyPressed { text: text.clone() });
        self.dispatch(WindowEvent::KeyReleased { text });
    }

    fn button(&self, button: PointerEventButton, down: bool, lparam: LPARAM) {
        let position = self.logical(lparam);
        self.last_pos.set(position);
        let bit = match button {
            PointerEventButton::Left => 1,
            PointerEventButton::Right => 2,
            _ => 4,
        };
        let hwnd = hw(self.hwnd.get());
        if down {
            if self.buttons.get() == 0 {
                // SAFETY: our window takes the mouse while a button is held.
                unsafe { SetCapture(hwnd) };
            }
            self.buttons.set(self.buttons.get() | bit);
            self.dispatch(WindowEvent::PointerPressed { position, button });
        } else {
            self.buttons.set(self.buttons.get() & !bit);
            if self.buttons.get() == 0 {
                // SAFETY: releasing our own capture.
                unsafe { ReleaseCapture() };
            }
            self.dispatch(WindowEvent::PointerReleased { position, button });
        }
    }
}

fn special(vk: u16, shift: bool) -> Option<Key> {
    Some(match vk {
        c::VK_BACK => Key::Backspace,
        c::VK_TAB if shift => Key::Backtab,
        c::VK_TAB => Key::Tab,
        c::VK_RETURN => Key::Return,
        c::VK_ESCAPE => Key::Escape,
        c::VK_DELETE => Key::Delete,
        c::VK_SHIFT => Key::Shift,
        c::VK_CONTROL => Key::Control,
        c::VK_MENU => Key::Alt,
        c::VK_CAPITAL => Key::CapsLock,
        c::VK_LWIN | c::VK_RWIN => Key::Meta,
        c::VK_LEFT => Key::LeftArrow,
        c::VK_RIGHT => Key::RightArrow,
        c::VK_UP => Key::UpArrow,
        c::VK_DOWN => Key::DownArrow,
        c::VK_HOME => Key::Home,
        c::VK_END => Key::End,
        c::VK_PRIOR => Key::PageUp,
        c::VK_NEXT => Key::PageDown,
        c::VK_INSERT => Key::Insert,
        c::VK_PAUSE => Key::Pause,
        c::VK_SCROLL => Key::ScrollLock,
        c::VK_APPS => Key::Menu,
        f @ c::VK_F1..=c::VK_F24 => FKEYS[(f - c::VK_F1) as usize],
        _ => return None,
    })
}

const FKEYS: [Key; 24] = [
    Key::F1,
    Key::F2,
    Key::F3,
    Key::F4,
    Key::F5,
    Key::F6,
    Key::F7,
    Key::F8,
    Key::F9,
    Key::F10,
    Key::F11,
    Key::F12,
    Key::F13,
    Key::F14,
    Key::F15,
    Key::F16,
    Key::F17,
    Key::F18,
    Key::F19,
    Key::F20,
    Key::F21,
    Key::F22,
    Key::F23,
    Key::F24,
];

unsafe extern "system" fn msgproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_WAKE {
        drain_queue();
        render_dirty();
        return 0;
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let key = hwnd as isize;
    if msg == c::WM_NCCALCSIZE && wp != 0 {
        // The client area is the whole window: no visible frame. A
        // maximized window would hang its frame off the screen, so it
        // keeps that much inside.
        if IsZoomed(hwnd) != 0 {
            let p = lp as *mut RECT;
            let inset = (8.0 * dpi_scale(key)) as i32;
            (*p).left += inset;
            (*p).top += inset;
            (*p).right -= inset;
            (*p).bottom -= inset;
        }
        return 0;
    }
    let Some(w) = find(key) else {
        return DefWindowProcW(hwnd, msg, wp, lp);
    };
    match msg {
        c::WM_NCHITTEST => return c::HTCLIENT,
        c::WM_ERASEBKGND => return 1,
        c::WM_PAINT => {
            w.paint();
            return 0;
        }
        c::WM_MOUSEACTIVATE => {
            let ex = windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, -20);
            if ex as u32 & c::WS_EX_NOACTIVATE != 0 {
                return c::MA_NOACTIVATE;
            }
        }
        c::WM_SHOWWINDOW => {
            if wp != 0 {
                w.dirty.set(true);
                wake();
            } else {
                w.release();
            }
        }
        c::WM_SIZE => {
            let size = slint::PhysicalSize::new((lp & 0xFFFF) as u32, ((lp >> 16) & 0xFFFF) as u32);
            if size != w.size.get() {
                w.resized(size);
                wake();
            }
            super::window_event(
                &w.label,
                key,
                super::WindowEvent::Resized(super::PhysicalSize::new(size.width, size.height)),
            );
        }
        c::WM_MOVE => {
            let x = (lp & 0xFFFF) as i16 as i32;
            let y = ((lp >> 16) & 0xFFFF) as i16 as i32;
            super::window_event(
                &w.label,
                key,
                super::WindowEvent::Moved(super::PhysicalPosition::new(x, y)),
            );
        }
        c::WM_DPICHANGED => {
            let scale = ((wp >> 16) & 0xFFFF) as f32 / 96.0;
            w.scale.set(scale);
            w.dispatch(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
            let r = &*(lp as *const RECT);
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                c::SWP_NOZORDER | c::SWP_NOACTIVATE,
            );
            w.dirty.set(true);
            super::window_event(
                &w.label,
                key,
                super::WindowEvent::ScaleFactorChanged {
                    scale_factor: f64::from(scale),
                },
            );
            return 0;
        }
        c::WM_SETFOCUS => {
            w.dispatch(WindowEvent::WindowActiveChanged(true));
            super::window_event(&w.label, key, super::WindowEvent::Focused(true));
        }
        c::WM_KILLFOCUS => {
            w.release_modifiers();
            w.dispatch(WindowEvent::WindowActiveChanged(false));
            super::window_event(&w.label, key, super::WindowEvent::Focused(false));
        }
        c::WM_SETTINGCHANGE => {
            super::sys::settings_changed(lp);
        }
        c::WM_MOUSEMOVE => {
            if !w.tracking.get() {
                let mut t = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: c::TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                TrackMouseEvent(&mut t);
                w.tracking.set(true);
            }
            let position = w.logical(lp);
            w.last_pos.set(position);
            w.dispatch(WindowEvent::PointerMoved { position });
            return 0;
        }
        c::WM_MOUSELEAVE => {
            w.tracking.set(false);
            w.dispatch(WindowEvent::PointerExited);
            return 0;
        }
        c::WM_LBUTTONDOWN => {
            w.button(PointerEventButton::Left, true, lp);
            return 0;
        }
        c::WM_LBUTTONUP => {
            w.button(PointerEventButton::Left, false, lp);
            return 0;
        }
        c::WM_RBUTTONDOWN => {
            w.button(PointerEventButton::Right, true, lp);
            return 0;
        }
        c::WM_RBUTTONUP => {
            w.button(PointerEventButton::Right, false, lp);
            return 0;
        }
        c::WM_MBUTTONDOWN => {
            w.button(PointerEventButton::Middle, true, lp);
            return 0;
        }
        c::WM_MBUTTONUP => {
            w.button(PointerEventButton::Middle, false, lp);
            return 0;
        }
        c::WM_MOUSEWHEEL | c::WM_MOUSEHWHEEL => {
            let mut pt = POINT {
                x: (lp & 0xFFFF) as i16 as i32,
                y: ((lp >> 16) & 0xFFFF) as i16 as i32,
            };
            ScreenToClient(hwnd, &mut pt);
            let s = w.scale.get();
            let position = LogicalPosition::new(pt.x as f32 / s, pt.y as f32 / s);
            let delta = ((wp >> 16) & 0xFFFF) as i16 as f32 / 120.0 * 60.0;
            let (dx, dy) = if msg == c::WM_MOUSEWHEEL {
                (0.0, delta)
            } else {
                (-delta, 0.0)
            };
            w.dispatch(WindowEvent::PointerScrolled {
                position,
                delta_x: dx,
                delta_y: dy,
            });
            return 0;
        }
        c::WM_KEYDOWN | c::WM_SYSKEYDOWN => {
            if w.key_down(wp as u16, lp, msg == c::WM_SYSKEYDOWN) {
                return 0;
            }
        }
        c::WM_KEYUP | c::WM_SYSKEYUP => {
            w.key_up(wp as u16);
        }
        c::WM_CHAR => {
            w.char_in(wp as u16);
            return 0;
        }
        c::WM_SYSCHAR => return 0,
        c::WM_CLOSE => {
            DestroyWindow(hwnd);
            return 0;
        }
        c::WM_DESTROY => {
            super::forget_label(&w.label, key);
            super::window_event(&w.label, key, super::WindowEvent::Destroyed);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

fn render_dirty() {
    for w in all_windows() {
        w.render();
    }
}

// ------------------------------------------------------------ helpers

/// Brings a window forward and gives it the keyboard.
pub fn focus_window(hwnd: isize) {
    // SAFETY: our window, on the UI thread.
    unsafe {
        SetForegroundWindow(hw(hwnd));
        SetFocus(hw(hwnd));
    }
}

pub fn destroy_window(hwnd: isize) {
    // SAFETY: DestroyWindow on the thread that made it.
    unsafe {
        if IsWindow(hw(hwnd)) != 0 {
            DestroyWindow(hw(hwnd));
        }
    }
}

/// A page's own key handling, before Slint sees the key (the bar's Enter,
/// arrows and chords). True from the hook means the key is taken.
pub fn set_key_hook(hwnd: isize, hook: KeyHook) {
    if let Some(w) = find(hwnd) {
        *w.key_hook.borrow_mut() = Some(hook);
    }
}

/// Moves the window with the mouse, as a title bar does. Called from a
/// pointer press; the move runs once the press has been handled.
pub fn begin_drag(hwnd: isize) {
    let _ = post(Box::new(move || {
        // SAFETY: our window; the system runs its move loop and returns.
        unsafe {
            ReleaseCapture();
            SendMessageW(hw(hwnd), c::WM_NCLBUTTONDOWN, c::HTCAPTION, 0);
        }
        if let Some(w) = find(hwnd) {
            w.buttons.set(0);
            let position = w.last_pos.get();
            w.dispatch(WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Left,
            });
        }
    }));
}

pub fn toggle_maximize(hwnd: isize) {
    // SAFETY: our window.
    unsafe {
        let cmd = if IsZoomed(hw(hwnd)) != 0 {
            c::SW_RESTORE
        } else {
            c::SW_MAXIMIZE
        };
        ShowWindow(hw(hwnd), cmd);
    }
}

pub fn hide_window(hwnd: isize) {
    // SAFETY: our window.
    unsafe { ShowWindow(hw(hwnd), c::SW_HIDE) };
}

/// Every page window's background follows the theme; opaque frames too.
pub fn theme_changed() {
    let bg = super::sys::theme_background();
    for w in all_windows() {
        w.background.set(bg);
        if !w.layered {
            dwm_frame(w.hwnd.get());
        }
    }
}

/// The pixels of a window as drawn last, RGBA, for screenshots (CI).
pub fn snapshot(hwnd: isize) -> Option<(u32, u32, Vec<u8>)> {
    let w = find(hwnd)?;
    let img = w.window.take_snapshot().ok()?;
    let (width, height) = (img.width(), img.height());
    let bytes = img.as_bytes().to_vec();
    Some((width, height, bytes))
}

/// Whether this window is drawn with per-pixel alpha.
pub fn is_layered(hwnd: isize) -> bool {
    find(hwnd).is_some_and(|w| w.layered)
}

// ---------------------------------------------------------- platform

struct Plat {
    start: Instant,
}

impl Platform for Plat {
    fn create_window_adapter(&self) -> std::result::Result<Rc<dyn WindowAdapter>, PlatformError> {
        let w: Rc<dyn WindowAdapter> = create_adapter()?;
        Ok(w)
    }

    fn run_event_loop(&self) -> std::result::Result<(), PlatformError> {
        run_loop();
        Ok(())
    }

    fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
        Some(Box::new(Proxy))
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }

    fn click_interval(&self) -> Duration {
        // SAFETY: reads a setting.
        Duration::from_millis(u64::from(unsafe { GetDoubleClickTime() }))
    }

    fn set_clipboard_text(&self, text: &str, clipboard: Clipboard) {
        if matches!(clipboard, Clipboard::DefaultClipboard) {
            super::sys::clipboard_write(text);
        }
    }

    fn clipboard_text(&self, clipboard: Clipboard) -> Option<String> {
        if matches!(clipboard, Clipboard::DefaultClipboard) {
            super::sys::clipboard_read()
        } else {
            None
        }
    }
}

/// Sets up the UI thread: call once, first, on the thread that will run
/// `run_loop`.
pub fn init() {
    // SAFETY: no arguments.
    UI_THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);
    super::sys::dpi_aware();
    register_classes();
    let _ = slint::platform::set_platform(Box::new(Plat {
        start: Instant::now(),
    }));
}

/// The UI thread's loop, until `request_exit`. Returns the exit code.
pub fn run_loop() -> i32 {
    loop {
        slint::platform::update_timers_and_animations();
        drain_queue();
        // Slint's `changed` handlers run inside update_timers_and_animations:
        // once for what the tasks just set, and again after drawing, for
        // the ones that read a size the layout only worked out then. What
        // they change is drawn before the loop sleeps (a page reports its
        // height that way), a few rounds at most so nothing can spin.
        for _ in 0..3 {
            slint::platform::update_timers_and_animations();
            render_dirty();
            slint::platform::update_timers_and_animations();
            if !all_windows().iter().any(|w| w.dirty.get() && w.visible()) {
                break;
            }
        }
        if EXIT.load(Ordering::SeqCst) {
            return EXIT_CODE.load(Ordering::SeqCst);
        }
        let animating = all_windows()
            .iter()
            .any(|w| w.visible() && w.window.has_active_animations());
        let timeout = if animating {
            16
        } else {
            match slint::platform::duration_until_next_timer_update() {
                Some(d) => d.as_millis().min(u128::from(u32::MAX - 1)) as u32,
                None => c::INFINITE,
            }
        };
        if !queue().lock().is_empty() {
            continue;
        }
        // SAFETY: waits for input or the timeout; no handles.
        unsafe {
            MsgWaitForMultipleObjectsEx(
                0,
                std::ptr::null(),
                timeout,
                c::QS_ALLINPUT,
                c::MWMO_INPUTAVAILABLE,
            )
        };
        // SAFETY: the standard pump, on the UI thread.
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, c::PM_REMOVE) != 0 {
                if msg.message == c::WM_QUIT {
                    EXIT.store(true, Ordering::SeqCst);
                    break;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

#[cfg(test)]
mod pixel_tests {
    use super::*;

    #[test]
    fn bgra_blends_premultiplied() {
        let mut p = Bgra::background();
        p.blend(PremultipliedRgbaColor {
            red: 128,
            green: 64,
            blue: 32,
            alpha: 128,
        });
        assert_eq!(p, Bgra(0x8080_4020));
        let mut q = Bgra::from_rgb(255, 255, 255);
        q.blend(PremultipliedRgbaColor {
            red: 0,
            green: 0,
            blue: 0,
            alpha: 255,
        });
        assert_eq!(q, Bgra(0xFF00_0000));
        let mut r = Bgra::from_rgb(200, 100, 50);
        r.blend(PremultipliedRgbaColor::default());
        assert_eq!(r, Bgra::from_rgb(200, 100, 50));
    }
}
