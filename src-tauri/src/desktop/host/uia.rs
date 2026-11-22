//! UI Automation reads (host.cs "UIA reads"): the cached IUIAutomation, one
//! element's properties, refs, the tree walk, getText, findText, docText,
//! the text-range ops, elementAt, focused and resolve.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::c_void;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use serde_json::{json, Map, Value};
use windows::core::{Interface, BSTR, GUID, HRESULT};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::*;

use super::input::pattern;
use super::wins::{
    foreground_root, is_excluded_pid, is_excluded_window, refuse_elevated, root, target_window,
    win_info, window_at, window_rect,
};
use super::{
    arg, check_cancel, clip, err, handle_str, int_arg, intersects, parse_ref, rect_arg, rect_json,
    ref_str, require_int, sleep_ms, str_arg, take_u16, u16len, DResult, Rect,
};

// Control type ids (UIA_*ControlTypeId).
pub(crate) const CT_BUTTON: i32 = 50000;
const CT_CHECKBOX: i32 = 50002;
pub(crate) const CT_COMBOBOX: i32 = 50003;
const CT_EDIT: i32 = 50004;
const CT_HYPERLINK: i32 = 50005;
const CT_LISTITEM: i32 = 50007;
pub(crate) const CT_LIST: i32 = 50008;
const CT_MENU: i32 = 50009;
const CT_MENUBAR: i32 = 50010;
const CT_MENUITEM: i32 = 50011;
const CT_RADIOBUTTON: i32 = 50013;
const CT_SLIDER: i32 = 50015;
const CT_SPINNER: i32 = 50016;
const CT_STATUSBAR: i32 = 50017;
const CT_TAB: i32 = 50018;
const CT_TABITEM: i32 = 50019;
const CT_TEXT: i32 = 50020;
const CT_TOOLBAR: i32 = 50021;
const CT_TREE: i32 = 50023;
const CT_TREEITEM: i32 = 50024;
const CT_CUSTOM: i32 = 50025;
const CT_GROUP: i32 = 50026;
const CT_THUMB: i32 = 50027;
const CT_DATAGRID: i32 = 50028;
const CT_DATAITEM: i32 = 50029;
const CT_DOCUMENT: i32 = 50030;
const CT_SPLITBUTTON: i32 = 50031;
const CT_WINDOW: i32 = 50032;
const CT_PANE: i32 = 50033;
const CT_HEADER: i32 = 50034;
const CT_HEADERITEM: i32 = 50035;
const CT_TABLE: i32 = 50036;
const CT_SEPARATOR: i32 = 50038;

/// ControlType.ProgrammaticName, lowercased after the dot, for the types
/// .NET Framework knows; anything newer is "unknown" there, and here.
const ROLES: [&str; 39] = [
    "button",
    "calendar",
    "checkbox",
    "combobox",
    "edit",
    "hyperlink",
    "image",
    "listitem",
    "list",
    "menu",
    "menubar",
    "menuitem",
    "progressbar",
    "radiobutton",
    "scrollbar",
    "slider",
    "spinner",
    "statusbar",
    "tab",
    "tabitem",
    "text",
    "toolbar",
    "tooltip",
    "tree",
    "treeitem",
    "custom",
    "group",
    "thumb",
    "datagrid",
    "dataitem",
    "document",
    "splitbutton",
    "window",
    "pane",
    "header",
    "headeritem",
    "table",
    "titlebar",
    "separator",
];

fn role_of(ct: i32) -> Option<&'static str> {
    let i = ct.checked_sub(CT_BUTTON)?;
    ROLES.get(usize::try_from(i).ok()?).copied()
}

const INTERACTIVE_TYPES: [i32; 15] = [
    CT_BUTTON,
    CT_CHECKBOX,
    CT_COMBOBOX,
    CT_EDIT,
    CT_HYPERLINK,
    CT_LISTITEM,
    CT_MENUITEM,
    CT_RADIOBUTTON,
    CT_SLIDER,
    CT_SPINNER,
    CT_SPLITBUTTON,
    CT_TABITEM,
    CT_TREEITEM,
    CT_DATAITEM,
    CT_HEADERITEM,
];
/// Containers that carry no meaning without a name. Unnamed ones are left
/// out of the tree and their children move up a level, the way read_page
/// drops anonymous divs.
const NOISE_TYPES: [i32; 7] = [
    CT_PANE,
    CT_GROUP,
    CT_CUSTOM,
    CT_SEPARATOR,
    CT_THUMB,
    CT_TOOLBAR,
    CT_STATUSBAR,
];
const NOT_FOCUS_TARGETS: [i32; 12] = [
    CT_PANE,
    CT_GROUP,
    CT_CUSTOM,
    CT_WINDOW,
    CT_DOCUMENT,
    CT_LIST,
    CT_TREE,
    CT_TABLE,
    CT_DATAGRID,
    CT_TAB,
    CT_TOOLBAR,
    CT_MENUBAR,
];
/// The roles whose Name or Value is page text (get_page_text).
const TEXT_ROLES: [i32; 10] = [
    CT_TEXT,
    CT_EDIT,
    CT_DOCUMENT,
    CT_LISTITEM,
    CT_DATAITEM,
    CT_HYPERLINK,
    CT_HEADER,
    CT_HEADERITEM,
    CT_TREEITEM,
    CT_MENUITEM,
];
/// What a piece of text inside it acts through: Find presses the button a
/// label sits in, not the label.
const ACT_TYPES: [i32; 10] = [
    CT_BUTTON,
    CT_HYPERLINK,
    CT_MENUITEM,
    CT_TABITEM,
    CT_LISTITEM,
    CT_CHECKBOX,
    CT_RADIOBUTTON,
    CT_TREEITEM,
    CT_SPLITBUTTON,
    CT_DATAITEM,
];

const PROPS: [UIA_PROPERTY_ID; 27] = [
    UIA_NamePropertyId,
    UIA_ControlTypePropertyId,
    UIA_BoundingRectanglePropertyId,
    UIA_IsOffscreenPropertyId,
    UIA_IsEnabledPropertyId,
    UIA_IsPasswordPropertyId,
    UIA_HasKeyboardFocusPropertyId,
    UIA_IsKeyboardFocusablePropertyId,
    UIA_AutomationIdPropertyId,
    UIA_HelpTextPropertyId,
    UIA_RuntimeIdPropertyId,
    UIA_ProcessIdPropertyId,
    UIA_IsInvokePatternAvailablePropertyId,
    UIA_IsValuePatternAvailablePropertyId,
    UIA_IsTogglePatternAvailablePropertyId,
    UIA_IsExpandCollapsePatternAvailablePropertyId,
    UIA_IsSelectionItemPatternAvailablePropertyId,
    UIA_IsScrollPatternAvailablePropertyId,
    UIA_IsScrollItemPatternAvailablePropertyId,
    UIA_IsTextPatternAvailablePropertyId,
    UIA_IsRangeValuePatternAvailablePropertyId,
    UIA_ValueValuePropertyId,
    UIA_ValueIsReadOnlyPropertyId,
    UIA_ToggleToggleStatePropertyId,
    UIA_ExpandCollapseExpandCollapseStatePropertyId,
    UIA_SelectionItemIsSelectedPropertyId,
    UIA_LocalizedControlTypePropertyId,
];

const MENU_PROPS: [UIA_PROPERTY_ID; 6] = [
    UIA_ControlTypePropertyId,
    UIA_NamePropertyId,
    UIA_IsEnabledPropertyId,
    UIA_ProcessIdPropertyId,
    UIA_IsExpandCollapsePatternAvailablePropertyId,
    UIA_IsSelectionItemPatternAvailablePropertyId,
];

// ------------------------------------------------------------ the instance

/// An element. UI Automation objects are free-threaded inside the MTA, and
/// every thread that touches one has joined it.
#[derive(Clone)]
pub(crate) struct El(pub IUIAutomationElement);
unsafe impl Send for El {}
unsafe impl Sync for El {}

/// The IUIAutomation and the conditions and cache requests every read
/// uses, made once.
pub(crate) struct Uia {
    pub a: IUIAutomation,
    pub view: IUIAutomationCondition,
    pub all: IUIAutomationCondition,
    pub walker: IUIAutomationTreeWalker,
    pub el_cache: IUIAutomationCacheRequest,
    pub tree_cache: IUIAutomationCacheRequest,
    pub menu_cache: IUIAutomationCacheRequest,
}
unsafe impl Send for Uia {}
unsafe impl Sync for Uia {}

impl Uia {
    fn new() -> windows::core::Result<Uia> {
        unsafe {
            let a: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let view = a.ControlViewCondition()?;
            let all = a.CreateTrueCondition()?;
            let walker = a.ControlViewWalker()?;
            let el_cache = cache(&a, &view, TreeScope_Element, &PROPS)?;
            let tree_cache = cache(&a, &view, TreeScope_Subtree, &PROPS)?;
            let menu_cache = cache(&a, &view, TreeScope_Element, &MENU_PROPS)?;
            Ok(Uia {
                a,
                view,
                all,
                walker,
                el_cache,
                tree_cache,
                menu_cache,
            })
        }
    }
}

unsafe fn cache(
    a: &IUIAutomation,
    view: &IUIAutomationCondition,
    scope: TreeScope,
    props: &[UIA_PROPERTY_ID],
) -> windows::core::Result<IUIAutomationCacheRequest> {
    let cr = a.CreateCacheRequest()?;
    for p in props {
        cr.AddProperty(*p)?;
    }
    cr.SetTreeScope(scope)?;
    cr.SetTreeFilter(view)?;
    cr.SetAutomationElementMode(AutomationElementMode_Full)?;
    Ok(cr)
}

static UIA: Mutex<Option<Arc<Uia>>> = Mutex::new(None);

pub(crate) fn uia() -> DResult<Arc<Uia>> {
    let mut g = UIA.lock();
    if let Some(u) = g.as_ref() {
        return Ok(u.clone());
    }
    let u = Arc::new(Uia::new()?);
    *g = Some(u.clone());
    Ok(u)
}

pub(crate) fn hwnd(h: isize) -> HWND {
    HWND(h as *mut c_void)
}

// ------------------------------------------------------------ variants

/// One property value, from the VARIANT UI Automation hands back.
#[derive(Debug, PartialEq)]
enum Var {
    Bool(bool),
    Int(i32),
    Str(String),
    Ints(Vec<i32>),
    Doubles(Vec<f64>),
    Other,
}

#[repr(C)]
struct RawVariant {
    vt: u16,
    reserved: [u16; 3],
    data: usize,
    extra: usize,
}

/// A one-dimensional SAFEARRAY's header.
#[repr(C)]
struct RawSafeArray {
    dims: u16,
    features: u16,
    elem_size: u32,
    locks: u32,
    data: *const u8,
    count: u32,
    lower: i32,
}

const _: () = assert!(std::mem::size_of::<VARIANT>() == std::mem::size_of::<RawVariant>());

unsafe fn array_of<T: Copy>(sa: *const RawSafeArray) -> Vec<T> {
    if sa.is_null() {
        return Vec::new();
    }
    let sa = &*sa;
    if sa.dims != 1 || sa.data.is_null() || sa.elem_size as usize != std::mem::size_of::<T>() {
        return Vec::new();
    }
    std::slice::from_raw_parts(sa.data as *const T, sa.count as usize).to_vec()
}

unsafe fn bstr_at(p: usize) -> String {
    if p == 0 {
        return String::new();
    }
    let ptr = p as *const u16;
    let bytes = *(ptr as *const u32).sub(1);
    String::from_utf16_lossy(std::slice::from_raw_parts(ptr, bytes as usize / 2))
}

fn parse_var(v: &VARIANT) -> Var {
    unsafe {
        let raw = &*(v as *const VARIANT as *const RawVariant);
        match raw.vt {
            11 => Var::Bool(raw.data as u16 != 0),
            3 | 22 => Var::Int(raw.data as u32 as i32),
            8 => Var::Str(bstr_at(raw.data)),
            0x2003 => Var::Ints(array_of::<i32>(raw.data as *const RawSafeArray)),
            0x2005 => Var::Doubles(array_of::<f64>(raw.data as *const RawSafeArray)),
            _ => Var::Other,
        }
    }
}

/// A property, cached or live. A gone element is an error; any other
/// failure is no value, as host.cs's Get.
fn prop(el: &IUIAutomationElement, id: UIA_PROPERTY_ID, cached: bool) -> DResult<Var> {
    let got = unsafe {
        if cached {
            el.GetCachedPropertyValueEx(id, false)
        } else {
            el.GetCurrentPropertyValueEx(id, false)
        }
    };
    match got {
        Ok(v) => Ok(parse_var(&v)),
        Err(e) if super::is_stale(&e) => Err(e.into()),
        Err(_) => Ok(Var::Other),
    }
}

fn pbool(el: &IUIAutomationElement, id: UIA_PROPERTY_ID, cached: bool) -> DResult<bool> {
    Ok(matches!(prop(el, id, cached)?, Var::Bool(true)))
}

fn pstr(el: &IUIAutomationElement, id: UIA_PROPERTY_ID, cached: bool) -> DResult<String> {
    Ok(match prop(el, id, cached)? {
        Var::Str(s) => s,
        Var::Int(i) => i.to_string(),
        Var::Bool(b) => if b { "True" } else { "False" }.to_string(),
        _ => String::new(),
    })
}

fn pint(el: &IUIAutomationElement, id: UIA_PROPERTY_ID, cached: bool) -> DResult<Option<i32>> {
    Ok(match prop(el, id, cached)? {
        Var::Int(i) => Some(i),
        _ => None,
    })
}

fn rect_of(d: &[f64]) -> Option<Rect> {
    if d.len() < 4 || d.iter().take(4).any(|v| !v.is_finite()) {
        return None;
    }
    Some([
        d[0].round() as i32,
        d[1].round() as i32,
        d[2].round() as i32,
        d[3].round() as i32,
    ])
}

fn prect(el: &IUIAutomationElement, cached: bool) -> DResult<Option<Rect>> {
    Ok(match prop(el, UIA_BoundingRectanglePropertyId, cached)? {
        Var::Doubles(d) => rect_of(&d),
        _ => None,
    })
}

// ------------------------------------------------------------ one element

#[derive(Clone, Default)]
pub(crate) struct Info {
    pub name: String,
    pub role: String,
    pub value: Option<String>,
    pub automation_id: String,
    pub help: String,
    pub rid: Option<String>,
    pub rect: Option<Rect>,
    pub offscreen: bool,
    pub enabled: bool,
    pub focused: bool,
    pub focusable: bool,
    pub password: bool,
    pub invoke: bool,
    pub value_p: bool,
    pub value_ro: bool,
    pub toggle: bool,
    pub expand: bool,
    pub select: bool,
    pub scroll: bool,
    pub text: bool,
    pub range: bool,
    pub scroll_item: bool,
    pub toggle_state: Option<&'static str>,
    pub expand_state: Option<&'static str>,
    pub selected: bool,
    pub pid: i32,
    pub control_type: i32,
}

pub(crate) fn read(el: &IUIAutomationElement, cached: bool) -> DResult<Info> {
    let mut i = Info::default();
    let ct = pint(el, UIA_ControlTypePropertyId, cached)?.unwrap_or(0);
    match role_of(ct) {
        Some(r) => {
            i.control_type = ct;
            i.role = r.to_string();
        }
        None => i.role = "unknown".into(),
    }
    // A web page's dialog (and a XAML one) is a Pane, Group or Custom whose
    // localized type says dialog. The agent's gate reads the text inside one
    // before a click in it, so it gets its own role.
    if matches!(i.control_type, CT_PANE | CT_GROUP | CT_CUSTOM) {
        let lct = pstr(el, UIA_LocalizedControlTypePropertyId, cached)?.to_lowercase();
        if matches!(lct.as_str(), "dialog" | "alert dialog" | "alertdialog") {
            i.role = "dialog".into();
        }
    }
    i.name = pstr(el, UIA_NamePropertyId, cached)?;
    i.rect = prect(el, cached)?;
    i.offscreen = pbool(el, UIA_IsOffscreenPropertyId, cached)?;
    i.enabled = !matches!(prop(el, UIA_IsEnabledPropertyId, cached)?, Var::Bool(false));
    i.password = pbool(el, UIA_IsPasswordPropertyId, cached)?;
    i.focused = pbool(el, UIA_HasKeyboardFocusPropertyId, cached)?;
    i.focusable = pbool(el, UIA_IsKeyboardFocusablePropertyId, cached)?;
    i.automation_id = pstr(el, UIA_AutomationIdPropertyId, cached)?;
    i.help = pstr(el, UIA_HelpTextPropertyId, cached)?;
    if let Var::Ints(rid) = prop(el, UIA_RuntimeIdPropertyId, cached)? {
        if !rid.is_empty() {
            let parts: Vec<String> = rid.iter().map(i32::to_string).collect();
            i.rid = Some(parts.join("."));
        }
    }
    i.pid = pint(el, UIA_ProcessIdPropertyId, cached)?.unwrap_or(0);
    i.invoke = pbool(el, UIA_IsInvokePatternAvailablePropertyId, cached)?;
    i.value_p = pbool(el, UIA_IsValuePatternAvailablePropertyId, cached)?;
    i.toggle = pbool(el, UIA_IsTogglePatternAvailablePropertyId, cached)?;
    i.expand = pbool(el, UIA_IsExpandCollapsePatternAvailablePropertyId, cached)?;
    i.select = pbool(el, UIA_IsSelectionItemPatternAvailablePropertyId, cached)?;
    i.scroll = pbool(el, UIA_IsScrollPatternAvailablePropertyId, cached)?;
    i.scroll_item = pbool(el, UIA_IsScrollItemPatternAvailablePropertyId, cached)?;
    i.text = pbool(el, UIA_IsTextPatternAvailablePropertyId, cached)?;
    i.range = pbool(el, UIA_IsRangeValuePatternAvailablePropertyId, cached)?;
    if i.value_p {
        i.value = Some(pstr(el, UIA_ValueValuePropertyId, cached)?);
        i.value_ro = pbool(el, UIA_ValueIsReadOnlyPropertyId, cached)?;
    }
    // Never hand a password to a model, even when a provider leaks it.
    if i.password && i.value.as_deref().is_some_and(|v| !v.is_empty()) {
        i.value = Some("\u{2022}\u{2022}\u{2022}".into());
    }
    if i.toggle {
        i.toggle_state = match pint(el, UIA_ToggleToggleStatePropertyId, cached)? {
            Some(1) => Some("on"),
            Some(0) => Some("off"),
            Some(_) => Some("mixed"),
            None => None,
        };
    }
    if i.expand {
        i.expand_state = match pint(el, UIA_ExpandCollapseExpandCollapseStatePropertyId, cached)? {
            Some(1) => Some("expanded"),
            Some(0) => Some("collapsed"),
            Some(2) => Some("partial"),
            _ => None,
        };
    }
    if i.select {
        i.selected = pbool(el, UIA_SelectionItemIsSelectedPropertyId, cached)?;
    }
    Ok(i)
}

fn is_interactive(i: &Info) -> bool {
    if INTERACTIVE_TYPES.contains(&i.control_type) {
        return true;
    }
    if i.invoke || i.toggle || i.select || i.range {
        return true;
    }
    if i.value_p && !i.value_ro {
        return true;
    }
    if i.expand && i.expand_state.is_some() {
        return true;
    }
    // A text editor is the most interactive thing in its window, and
    // Document is otherwise left out with the other containers.
    if i.control_type == CT_DOCUMENT && i.focusable && (i.text || (i.value_p && !i.value_ro)) {
        return true;
    }
    i.focusable && !NOT_FOCUS_TARGETS.contains(&i.control_type) && !i.name.is_empty()
}

fn is_act_target(i: &Info) -> bool {
    ACT_TYPES.contains(&i.control_type) && (i.invoke || i.toggle || i.select || i.expand)
}

// ------------------------------------------------------------ refs

#[derive(Clone)]
pub(crate) struct RefEntry {
    pub el: El,
    pub top: isize,
    pub pid: i32,
    rid: Option<String>,
}

const MAX_REFS: usize = 30000;

#[derive(Default)]
struct Refs {
    map: HashMap<i32, RefEntry>,
    by_rid: HashMap<String, i32>,
    order: VecDeque<i32>,
    next: i32,
}

static REFS: Mutex<Option<Refs>> = Mutex::new(None);

pub(crate) fn ref_count() -> usize {
    REFS.lock().as_ref().map_or(0, |r| r.map.len())
}

fn ref_for(el: &IUIAutomationElement, i: &Info, top: isize) -> i32 {
    let mut g = REFS.lock();
    let refs = g.get_or_insert_with(|| Refs {
        next: 1,
        ..Refs::default()
    });
    if let Some(rid) = &i.rid {
        if let Some(&r) = refs.by_rid.get(rid) {
            if let Some(e) = refs.map.get_mut(&r) {
                e.el = El(el.clone());
                e.top = top;
                return r;
            }
        }
    }
    let r = refs.next;
    refs.next += 1;
    refs.map.insert(
        r,
        RefEntry {
            el: El(el.clone()),
            top,
            pid: i.pid,
            rid: i.rid.clone(),
        },
    );
    if let Some(rid) = &i.rid {
        refs.by_rid.insert(rid.clone(), r);
    }
    refs.order.push_back(r);
    while refs.order.len() > MAX_REFS {
        let Some(old) = refs.order.pop_front() else {
            break;
        };
        if let Some(oe) = refs.map.remove(&old) {
            if let Some(rid) = oe.rid {
                refs.by_rid.remove(&rid);
            }
        }
    }
    r
}

/// The browser toolset's wording, so a model trained on it knows the
/// recovery.
fn stale(r: i32) -> super::DesktopError {
    err(
        "STALE_REF",
        format!(
            "ref_{r} is stale or not found on the current page. Re-read the page to get fresh references."
        ),
    )
}

pub(crate) fn get_ref(v: Option<&Value>) -> DResult<RefEntry> {
    let r = parse_ref(v)?;
    if r <= 0 {
        return Err(err("BAD_ARGS", "missing ref"));
    }
    let e = REFS
        .lock()
        .as_ref()
        .and_then(|refs| refs.map.get(&r).cloned())
        .ok_or_else(|| stale(r))?;
    if is_excluded_pid(e.pid as u32) || is_excluded_window(e.top) {
        return Err(err(
            "BLOCKED_OWN_WINDOW",
            "refusing to act on Aevaleron's own window",
        ));
    }
    match unsafe { e.el.0.CurrentControlType() } {
        Ok(_) => Ok(e),
        Err(x) if super::is_stale(&x) => Err(stale(r)),
        Err(x) => Err(x.into()),
    }
}

pub(crate) fn node_dict(i: &Info, r: i32, parent: i32, depth: i32, value_max: usize) -> Value {
    let mut d = Map::new();
    d.insert("ref".into(), json!(ref_str(r)));
    let parent = if parent > 0 {
        json!(ref_str(parent))
    } else {
        Value::Null
    };
    d.insert("parentRef".into(), parent);
    d.insert("depth".into(), json!(depth));
    d.insert("role".into(), json!(i.role));
    d.insert("name".into(), json!(clip(&i.name, 400)));
    if let Some(v) = i.value.as_deref().filter(|v| !v.is_empty()) {
        d.insert("value".into(), json!(clip(v, value_max)));
    }
    d.insert("rect".into(), rect_json(i.rect));
    let mut states = Vec::new();
    if i.focused {
        states.push("focused");
    }
    if !i.enabled {
        states.push("disabled");
    }
    match i.toggle_state {
        Some("on") => states.push("checked"),
        Some("mixed") => states.push("mixed"),
        _ => {}
    }
    match i.expand_state {
        Some("expanded") => states.push("expanded"),
        Some("collapsed") => states.push("collapsed"),
        _ => {}
    }
    if i.selected {
        states.push("selected");
    }
    if i.password {
        states.push("password");
    }
    if i.offscreen {
        states.push("offscreen");
    }
    if i.value_p && i.value_ro {
        states.push("readonly");
    }
    d.insert("states".into(), json!(states));
    let mut patterns = Vec::new();
    for (on, name) in [
        (i.invoke, "invoke"),
        (i.value_p, "value"),
        (i.toggle, "toggle"),
        (i.expand, "expandcollapse"),
        (i.select, "selectionitem"),
        (i.range, "rangevalue"),
        (i.scroll, "scroll"),
        (i.scroll_item, "scrollitem"),
        (i.text, "text"),
    ] {
        if on {
            patterns.push(name);
        }
    }
    d.insert("patterns".into(), json!(patterns));
    let aid = if i.automation_id.is_empty() {
        Value::Null
    } else {
        json!(clip(&i.automation_id, 120))
    };
    d.insert("automationId".into(), aid);
    if !i.help.is_empty() {
        d.insert("helpText".into(), json!(clip(&i.help, 200)));
    }
    if is_interactive(i) {
        d.insert("interactive".into(), json!(true));
    }
    Value::Object(d)
}

// ------------------------------------------------------------ generation

/// A COM object for UI Automation's structure-changed event: it bumps a
/// window's generation, so a caller holding refs knows the tree it read is
/// no longer the tree on screen.
#[repr(C)]
struct Bumper {
    vtbl: *const BumperVtbl,
    refs: AtomicU32,
    gen: Arc<AtomicI32>,
}

#[repr(C)]
struct BumperVtbl {
    query: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    handle: unsafe extern "system" fn(*mut c_void, *mut c_void, i32, *const c_void) -> HRESULT,
}

static BUMPER_VTBL: BumperVtbl = BumperVtbl {
    query: bumper_query,
    add_ref: bumper_add_ref,
    release: bumper_release,
    handle: bumper_handle,
};

unsafe extern "system" fn bumper_query(
    this: *mut c_void,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    if out.is_null() {
        return HRESULT(0x8000_4003_u32 as i32);
    }
    let iid = &*iid;
    let unknown = windows::core::IUnknown::IID;
    if *iid == unknown || *iid == IUIAutomationStructureChangedEventHandler::IID {
        bumper_add_ref(this);
        *out = this;
        HRESULT(0)
    } else {
        *out = std::ptr::null_mut();
        HRESULT(0x8000_4002_u32 as i32)
    }
}

unsafe extern "system" fn bumper_add_ref(this: *mut c_void) -> u32 {
    (*(this as *const Bumper))
        .refs
        .fetch_add(1, Ordering::SeqCst)
        + 1
}

unsafe extern "system" fn bumper_release(this: *mut c_void) -> u32 {
    let left = (*(this as *const Bumper))
        .refs
        .fetch_sub(1, Ordering::SeqCst)
        - 1;
    if left == 0 {
        drop(Box::from_raw(this as *mut Bumper));
    }
    left
}

unsafe extern "system" fn bumper_handle(
    this: *mut c_void,
    _sender: *mut c_void,
    _change: i32,
    _runtime_id: *const c_void,
) -> HRESULT {
    (*(this as *const Bumper))
        .gen
        .fetch_add(1, Ordering::SeqCst);
    HRESULT(0)
}

fn bumper(gen: Arc<AtomicI32>) -> IUIAutomationStructureChangedEventHandler {
    let b = Box::new(Bumper {
        vtbl: &BUMPER_VTBL,
        refs: AtomicU32::new(1),
        gen,
    });
    // The one reference the box starts with becomes the interface's.
    unsafe { IUIAutomationStructureChangedEventHandler::from_raw(Box::into_raw(b).cast()) }
}

struct Watched {
    key: isize,
    el: El,
    handler: IUIAutomationStructureChangedEventHandler,
}
unsafe impl Send for Watched {}

#[derive(Default)]
struct Gens {
    gens: HashMap<isize, Arc<AtomicI32>>,
    watched: VecDeque<Watched>,
}

static GENS: Mutex<Option<Gens>> = Mutex::new(None);
const MAX_WATCHED: usize = 6;

fn unsubscribe(w: Watched) {
    let _ = super::spawn_op(None, move || {
        // The whole Watched moves in, not its non-Send fields.
        let w = w;
        if let Ok(u) = uia() {
            let _ = unsafe { u.a.RemoveStructureChangedEventHandler(&w.el.0, &w.handler) };
        }
    });
}

/// The window's generation, and whether this is its first read. The first
/// read subscribes; the least recent of more than six is unsubscribed.
fn generation(u: &Uia, h: isize, root_el: &IUIAutomationElement) -> (i32, bool) {
    let gen = {
        let mut g = GENS.lock();
        let gens = g.get_or_insert_with(Gens::default);
        if let Some(n) = gens.gens.get(&h) {
            return (n.load(Ordering::SeqCst), false);
        }
        let n = Arc::new(AtomicI32::new(1));
        gens.gens.insert(h, n.clone());
        n
    };
    // Outside the lock: a slow app can take seconds to answer this, and
    // every other window's read needs the lock.
    let handler = bumper(gen.clone());
    let subscribed = unsafe {
        u.a.AddStructureChangedEventHandler(
            root_el,
            TreeScope_Subtree,
            None::<&IUIAutomationCacheRequest>,
            &handler,
        )
    }
    .is_ok();
    let mut drop_list = Vec::new();
    {
        let mut g = GENS.lock();
        let gens = g.get_or_insert_with(Gens::default);
        if subscribed {
            gens.watched.push_back(Watched {
                key: h,
                el: El(root_el.clone()),
                handler,
            });
        }
        while gens.watched.len() > MAX_WATCHED {
            if let Some(old) = gens.watched.pop_front() {
                gens.gens.remove(&old.key);
                drop_list.push(old);
            }
        }
    }
    for w in drop_list {
        unsubscribe(w);
    }
    (gen.load(Ordering::SeqCst), true)
}

pub(crate) fn reset() {
    *REFS.lock() = None;
    let old = GENS.lock().take();
    if let Some(g) = old {
        for w in g.watched {
            unsubscribe(w);
        }
    }
}

// ------------------------------------------------------------ the walk

type OnNode<'a> = dyn FnMut(&mut Walk, &IUIAutomationElement, &Info, i32, &str) -> bool + 'a;

pub(crate) struct Walk {
    filter: String,
    max_depth: i32,
    max_nodes: usize,
    value_max: usize,
    lazy: bool,
    by_find: bool,
    pub top: isize,
    pub win_rect: Option<Rect>,
    visited: usize,
    pub truncated: bool,
    nodes: Vec<Value>,
    /// The nearest button, link or item above the node being visited, for
    /// getText: the element a piece of text acts through.
    act_ref: i32,
    act_info: Option<Info>,
}

fn element_array(arr: &IUIAutomationElementArray) -> Vec<IUIAutomationElement> {
    unsafe {
        let n = arr.Length().unwrap_or(0);
        (0..n).filter_map(|i| arr.GetElement(i).ok()).collect()
    }
}

#[allow(clippy::too_many_arguments)]
fn visit(
    u: &Uia,
    w: &mut Walk,
    on: &mut Option<&mut OnNode<'_>>,
    el: &IUIAutomationElement,
    parent_ref: i32,
    depth: i32,
    is_root: bool,
    parent_name: &str,
) {
    if w.truncated {
        return;
    }
    w.visited += 1;
    let Ok(mut i) = read(el, true) else {
        return;
    };
    let interactive = is_interactive(&i);
    // IsOffscreen lies in some WPF apps, so a rect outside the window counts
    // as offscreen too.
    let visible = !i.offscreen
        && match i.rect {
            None => true,
            Some(r) => r[2] > 0 && r[3] > 0 && intersects(Some(r), w.win_rect),
        };
    if !visible && !i.offscreen && i.rect.is_some() {
        i.offscreen = true;
    }
    let has_value = i.value.as_deref().is_some_and(|v| !v.is_empty());
    let emit = if is_root {
        true
    } else {
        let noise_type = NOISE_TYPES.contains(&i.control_type);
        let noise =
            noise_type && i.name.is_empty() && !has_value && !interactive && i.role != "dialog";
        // A button's text child repeats the button's name.
        let echo = i.control_type == CT_TEXT
            && !i.name.is_empty()
            && i.name == parent_name
            && !interactive;
        // Chromium marks unnamed wrapper groups invokable; listing them
        // gives the model a column of "group [ref_7]" to guess at.
        let bare = noise_type && i.name.is_empty() && !has_value;
        match w.filter.as_str() {
            "interactive" => interactive && visible && !bare,
            "visible" => !noise && !echo && visible,
            _ => !noise && !echo,
        }
    };
    let mut my_ref = parent_ref;
    let mut child_depth = depth;
    let mut my_name = parent_name.to_string();
    let mut descend = true;
    if emit {
        my_ref = ref_for(el, &i, w.top);
        match on.as_mut() {
            Some(f) => descend = f(w, el, &i, my_ref, parent_name),
            None => {
                let node = node_dict(&i, my_ref, parent_ref, depth, w.value_max);
                w.nodes.push(node);
            }
        }
        child_depth = depth + 1;
        my_name = i.name.clone();
        if w.nodes.len() >= w.max_nodes {
            w.truncated = true;
            return;
        }
    }
    if !descend || child_depth > w.max_depth {
        return;
    }
    // Content scrolled out of view is most of a long web page and has
    // nothing on screen to act on.
    if !is_root && i.offscreen && w.filter != "all" && w.lazy {
        return;
    }
    let kids = unsafe {
        if w.by_find {
            el.FindAllBuildCache(TreeScope_Children, &u.view, &u.el_cache)
        } else {
            el.GetCachedChildren()
        }
    };
    let Ok(kids) = kids else {
        return;
    };
    let held_ref = w.act_ref;
    let held_info = w.act_info.take();
    w.act_info = held_info.clone();
    if emit && !is_root && is_act_target(&i) {
        w.act_ref = my_ref;
        w.act_info = Some(i);
    }
    for k in element_array(&kids) {
        visit(u, w, on, &k, my_ref, child_depth, false, &my_name);
        if w.truncated {
            break;
        }
    }
    w.act_ref = held_ref;
    w.act_info = held_info;
}

/// 'subtree' asks UIA for the whole window in one cross-process call, which
/// is what keeps a Chrome or Explorer window well under a second. It cannot
/// stop early, so 'walk' (one call per container, stops at maxNodes, skips
/// offscreen subtrees) is there for pathological trees.
fn run_walk(a: &Value, filter: &str, mut on: Option<&mut OnNode<'_>>) -> DResult<(Walk, i32)> {
    let u = uia()?;
    let mut start: Option<IUIAutomationElement> = None;
    let h = if arg(a, "ref").is_some() {
        let e = get_ref(arg(a, "ref"))?;
        start = Some(e.el.0.clone());
        e.top
    } else {
        target_window(a)?
    };
    let mut w = Walk {
        filter: filter.to_string(),
        max_depth: int_arg(a, "depth", 15)?.max(0),
        max_nodes: int_arg(a, "maxNodes", 3000)?.max(1) as usize,
        value_max: int_arg(a, "valueMax", 300)?.max(16) as usize,
        lazy: false,
        by_find: false,
        top: h,
        win_rect: window_rect(h),
        visited: 0,
        truncated: false,
        nodes: Vec::new(),
        act_ref: 0,
        act_info: None,
    };
    let strategy = str_arg(a, "strategy", "subtree");
    let win_el = unsafe { u.a.ElementFromHandle(hwnd(h))? };
    let (gen, first_read) = generation(&u, h, &win_el);
    let start = start.unwrap_or(win_el);
    let mut waits = [300u64, 700].into_iter();
    loop {
        let root_el = if strategy == "walk" {
            w.lazy = true;
            w.by_find = true;
            unsafe { start.BuildUpdatedCache(&u.el_cache)? }
        } else {
            unsafe { start.BuildUpdatedCache(&u.tree_cache)? }
        };
        visit(&u, &mut w, &mut on, &root_el, 0, 0, true, "");
        // Chromium, Electron and Firefox build their tree the first time a
        // client asks, so a first read of a window can come back nearly
        // empty. Retried after 300ms, then 700ms more.
        if !first_read || w.visited >= 30 {
            break;
        }
        let Some(ms) = waits.next() else {
            break;
        };
        sleep_ms(ms);
        w.nodes.clear();
        w.visited = 0;
        w.truncated = false;
        w.act_ref = 0;
        w.act_info = None;
    }
    Ok((w, gen))
}

pub(crate) fn tree(a: &Value) -> DResult<Value> {
    let filter = str_arg(a, "filter", "visible");
    if !matches!(filter.as_str(), "interactive" | "all" | "visible") {
        return Err(err(
            "BAD_ARGS",
            "filter must be 'visible', 'interactive' or 'all'",
        ));
    }
    let t0 = Instant::now();
    let (w, gen) = run_walk(a, &filter, None)?;
    Ok(json!({
        "window": win_info(w.top, None, foreground_root()),
        "windowHandle": handle_str(w.top),
        "filter": filter,
        "nodes": w.nodes,
        "visited": w.visited,
        "truncated": w.truncated,
        "generation": gen,
        "readMs": t0.elapsed().as_millis() as u64,
    }))
}

// ------------------------------------------------------------ text

fn bounding_rects(r: &IUIAutomationTextRange) -> Vec<[f64; 4]> {
    unsafe {
        let Ok(sa) = r.GetBoundingRectangles() else {
            return Vec::new();
        };
        let d = array_of::<f64>(sa as *const RawSafeArray);
        if !sa.is_null() {
            windows_sys::Win32::System::Ole::SafeArrayDestroy(sa as *const _);
        }
        d.chunks_exact(4)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect()
    }
}

fn union(rects: &[[f64; 4]]) -> Option<Rect> {
    if rects.is_empty() {
        return None;
    }
    let (mut l, mut t, mut r, mut b) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for x in rects {
        l = l.min(x[0]);
        t = t.min(x[1]);
        r = r.max(x[0] + x[2]);
        b = b.max(x[1] + x[3]);
    }
    Some([
        l.round() as i32,
        t.round() as i32,
        (r - l).round() as i32,
        (b - t).round() as i32,
    ])
}

fn span(text: &str, rect: Option<Rect>, r: i32, source: &str) -> Map<String, Value> {
    let mut d = Map::new();
    d.insert("text".into(), json!(text));
    d.insert("rect".into(), rect_json(rect));
    let rv = if r > 0 {
        json!(ref_str(r))
    } else {
        Value::Null
    };
    d.insert("ref".into(), rv);
    d.insert("source".into(), json!(source));
    d
}

/// The patterns Find chooses an action from.
fn act_patterns(i: &Info) -> Vec<&'static str> {
    let mut p = Vec::new();
    for (on, name) in [
        (i.invoke, "invoke"),
        (i.toggle, "toggle"),
        (i.select, "selectionitem"),
        (i.expand, "expandcollapse"),
        (i.text, "text"),
        (i.value_p && !i.value_ro, "value"),
    ] {
        if on {
            p.push(name);
        }
    }
    p
}

fn act_target(r: i32, i: &Info) -> Value {
    let mut v = json!({ "ref": ref_str(r), "role": i.role, "pat": act_patterns(i) });
    if !i.enabled {
        v["enabled"] = json!(false);
    }
    v
}

/// What a span is, for Find's "act on a hit": its own role and patterns,
/// and the button, link or item it sits in, if any.
fn tag(d: &mut Map<String, Value>, i: &Info, via: Option<(i32, &Info)>, line: bool) {
    d.insert("role".into(), json!(i.role));
    d.insert("pat".into(), json!(act_patterns(i)));
    if !i.enabled {
        d.insert("enabled".into(), json!(false));
    }
    if line {
        d.insert("line".into(), json!(true));
    }
    if let Some((r, info)) = via.filter(|(r, _)| *r > 0) {
        d.insert("via".into(), act_target(r, info));
    }
}

fn text_pattern(el: &IUIAutomationElement) -> Option<IUIAutomationTextPattern> {
    pattern(el, UIA_TextPatternId)
}

fn range_text(r: &IUIAutomationTextRange, max: i32) -> windows::core::Result<String> {
    unsafe { r.GetText(max).map(|b| b.to_string()) }
}

fn ranges(arr: &IUIAutomationTextRangeArray) -> Vec<IUIAutomationTextRange> {
    unsafe {
        let n = arr.Length().unwrap_or(0);
        (0..n).filter_map(|i| arr.GetElement(i).ok()).collect()
    }
}

fn span_rect(s: &Map<String, Value>) -> Option<Rect> {
    let a = s.get("rect")?.as_array()?;
    if a.len() != 4 {
        return None;
    }
    let v: Vec<i32> = a
        .iter()
        .filter_map(|x| x.as_i64())
        .map(|x| x as i32)
        .collect();
    (v.len() == 4).then(|| [v[0], v[1], v[2], v[3]])
}

fn span_text(s: &Map<String, Value>) -> String {
    s.get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// One span per visible line, with that line's rectangle, so Find can draw
/// on the exact words.
fn line_spans(
    el: &IUIAutomationElement,
    r: i32,
    clip_rect: Option<Rect>,
    spans: &mut Vec<Map<String, Value>>,
    chars: &mut usize,
    max_chars: usize,
) -> windows::core::Result<bool> {
    let Some(tp) = text_pattern(el) else {
        return Ok(false);
    };
    let mut got = 0;
    unsafe {
        for vis in ranges(&tp.GetVisibleRanges()?) {
            let line = vis.Clone()?;
            line.MoveEndpointByRange(
                TextPatternRangeEndpoint_End,
                &line,
                TextPatternRangeEndpoint_Start,
            )?;
            let mut n = 0;
            while n < 2000 && *chars < max_chars {
                n += 1;
                line.ExpandToEnclosingUnit(TextUnit_Line)?;
                let past = line.CompareEndpoints(
                    TextPatternRangeEndpoint_Start,
                    &vis,
                    TextPatternRangeEndpoint_End,
                )?;
                if past >= 0 {
                    break;
                }
                let room = (max_chars - *chars).max(1) as i32;
                let t = range_text(&line, room)?;
                let t = t.trim_end_matches(['\r', '\n']);
                if !t.trim().is_empty() {
                    let rect = union(&bounding_rects(&line));
                    if clip_rect.is_none() || intersects(rect, clip_rect) {
                        spans.push(span(t, rect, r, "uia"));
                        *chars += u16len(t) + 1;
                        got += 1;
                    }
                }
                if line.Move(TextUnit_Line, 1)? == 0 {
                    break;
                }
            }
        }
    }
    got += missed_lines(el, &tp, r, clip_rect, spans, chars, max_chars);
    Ok(got > 0)
}

/// Some providers (classic Win32 edits through the MSAA proxy) report stale
/// visible ranges, so a line typed a moment ago is missing from the walk
/// above. The whole document is compared against it, and a missing line is
/// added with a rectangle estimated from the lines that were found, as long
/// as it falls inside the control.
fn missed_lines(
    el: &IUIAutomationElement,
    tp: &IUIAutomationTextPattern,
    r: i32,
    clip_rect: Option<Rect>,
    spans: &mut Vec<Map<String, Value>>,
    chars: &mut usize,
    max_chars: usize,
) -> usize {
    let doc = unsafe { tp.DocumentRange().and_then(|d| range_text(&d, 20001)) };
    let Ok(doc) = doc else { return 0 };
    if u16len(&doc) > 20000 {
        return 0;
    }
    let Ok(Some(bx)) = prect(el, false) else {
        return 0;
    };
    let want = (r > 0).then(|| ref_str(r));
    let mut have: HashSet<String> = HashSet::new();
    let mut first: Option<(Rect, String)> = None;
    for s in spans.iter() {
        let sref = s.get("ref").and_then(Value::as_str).map(str::to_string);
        if sref != want {
            continue;
        }
        let t = span_text(s).trim().to_string();
        have.insert(t.clone());
        if first.is_none() {
            if let Some(rc) = span_rect(s) {
                first = Some((rc, t));
            }
        }
    }
    let doc = doc.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = doc.split('\n').collect();
    let line_h = first
        .as_ref()
        .map_or(16, |f| if f.0[3] > 0 { f.0[3] } else { 16 });
    let mut first_idx = 0;
    if let Some((_, ft)) = &first {
        if let Some(k) = lines.iter().position(|l| l.trim() == ft) {
            first_idx = k as i32;
        }
    }
    let top = first
        .as_ref()
        .map_or(bx[1], |f| f.0[1] - first_idx * line_h);
    let mut added = 0;
    for (i, line) in lines.iter().enumerate() {
        if *chars >= max_chars {
            break;
        }
        let t = line.trim_end();
        if t.trim().is_empty() || have.contains(t.trim()) {
            continue;
        }
        let y = top + i as i32 * line_h;
        if y < bx[1] || y + line_h > bx[1] + bx[3] {
            continue;
        }
        let rect = Some([bx[0] + 4, y, (bx[2] - 8).max(8), line_h]);
        if clip_rect.is_some() && !intersects(rect, clip_rect) {
            continue;
        }
        spans.push(span(t, rect, r, "uia"));
        *chars += u16len(t) + 1;
        added += 1;
    }
    added
}

pub(crate) fn get_text(mut a: Value) -> DResult<Value> {
    let max_chars = int_arg(&a, "maxChars", 50000)?.max(100) as usize;
    let clip_rect = rect_arg(&a, "rect")?;
    let mut spans: Vec<Map<String, Value>> = Vec::new();
    let mut chars = 0usize;
    let t0 = Instant::now();
    a["depth"] = json!(int_arg(&a, "depth", 60)?);
    let (w, _) = {
        let mut on = |w: &mut Walk, el: &IUIAutomationElement, i: &Info, r: i32, parent: &str| {
            if chars >= max_chars {
                w.truncated = true;
                return false;
            }
            if !TEXT_ROLES.contains(&i.control_type) || i.password {
                return true;
            }
            if i.text && matches!(i.control_type, CT_DOCUMENT | CT_EDIT) {
                // The document's own text covers its children.
                let before = spans.len();
                let lines = line_spans(el, r, clip_rect, &mut spans, &mut chars, max_chars)
                    .unwrap_or(false);
                for s in spans.iter_mut().skip(before) {
                    tag(s, i, None, true);
                }
                if lines {
                    return false;
                }
            }
            let text = match i.value.as_deref().filter(|v| !v.is_empty()) {
                Some(v) => v.to_string(),
                None if i.name != parent => i.name.clone(),
                None => String::new(),
            };
            if text.trim().is_empty() {
                return true;
            }
            if clip_rect.is_some() && !intersects(i.rect, clip_rect) {
                return true;
            }
            let text = clip(&text, (max_chars - chars).max(1));
            let mut s = span(&text, i.rect, r, "uia");
            let via = w.act_info.as_ref().map(|info| (w.act_ref, info));
            tag(&mut s, i, via, false);
            spans.push(s);
            chars += u16len(&text) + 1;
            true
        };
        run_walk(&a, "visible", Some(&mut on))?
    };
    // Reading order: top then left, with spans on one visual line (within
    // half a line height) kept left to right. Spans of one element keep
    // their own order.
    type Keyed = ((i32, i32, usize), Map<String, Value>);
    let mut keyed: Vec<Keyed> = Vec::new();
    let mut last = (i32::MIN, i32::MIN);
    for (n, s) in spans.into_iter().enumerate() {
        let key = match span_rect(&s) {
            Some(r) => {
                last = ((r[1] + r[3] / 2) / 12, r[0]);
                (last.0, last.1, n)
            }
            None => (last.0, last.1, n),
        };
        keyed.push((key, s));
    }
    keyed.sort_by_key(|k| k.0);
    let mut text = String::new();
    let mut out = Vec::new();
    for (_, s) in keyed {
        text.push_str(&span_text(&s));
        text.push('\n');
        out.push(Value::Object(s));
    }
    Ok(json!({
        "windowHandle": handle_str(w.top),
        "windowRect": rect_json(w.win_rect),
        "text": text,
        "spans": out,
        "source": "uia",
        "truncated": w.truncated,
        "readMs": t0.elapsed().as_millis() as u64,
    }))
}

// Find's whole-document search: every place the words occur in each
// document or editor, on screen or scrolled away, with the rectangle the
// application itself reports for a match that is showing.
const FIND_LINE_CONTEXT: i32 = 120;
const FIND_TARGET_MAX: i32 = 24;
const FIND_TARGET_BUDGET: u128 = 150;

/// The link or button a match in a document sits in, if any: Find presses
/// that instead of selecting its words. A few steps up, never past the
/// document.
fn range_target(
    u: &Uia,
    m: &IUIAutomationTextRange,
    doc: &IUIAutomationElement,
    top: isize,
) -> Option<Value> {
    unsafe {
        let mut el = m.GetEnclosingElement().ok()?;
        for _ in 0..4 {
            if u.a.CompareElements(&el, doc).ok()?.as_bool() {
                return None;
            }
            let c = el.BuildUpdatedCache(&u.el_cache).ok()?;
            let i = read(&c, true).ok()?;
            let ct = i.control_type;
            if ct == CT_DOCUMENT || ct == CT_WINDOW {
                return None;
            }
            let pressable = matches!(
                ct,
                CT_HYPERLINK
                    | CT_BUTTON
                    | CT_SPLITBUTTON
                    | CT_CHECKBOX
                    | CT_RADIOBUTTON
                    | CT_TABITEM
                    | CT_MENUITEM
            );
            if pressable && is_act_target(&i) && !is_excluded_pid(i.pid as u32) {
                return Some(act_target(ref_for(&c, &i, top), &i));
            }
            el = u.walker.GetParentElement(&el).ok()?;
        }
    }
    None
}

struct Found {
    doc: Value,
    found: i32,
    cut: bool,
}

#[allow(clippy::too_many_arguments)]
fn find_in_text(
    u: &Uia,
    el: &IUIAutomationElement,
    r: i32,
    bx: Option<Rect>,
    top: isize,
    query: &str,
    max: i32,
    t0: Instant,
    budget: u128,
) -> windows::core::Result<Option<Found>> {
    let Some(tp) = text_pattern(el) else {
        return Ok(None);
    };
    let mut found = 0;
    let mut cut = false;
    let mut asked = 0;
    let mut asking = 0u128;
    let mut matches = Vec::new();
    let q = BSTR::from(query);
    unsafe {
        let rest = tp.DocumentRange()?.Clone()?;
        let mut last: Option<IUIAutomationTextRange> = None;
        loop {
            if found >= max || t0.elapsed().as_millis() > budget {
                cut = true;
                break;
            }
            let Ok(m) = rest.FindText(&q, false, true) else {
                break;
            };
            // A provider that does not move forward would loop for ever.
            if let Some(l) = &last {
                let c = m.CompareEndpoints(
                    TextPatternRangeEndpoint_Start,
                    l,
                    TextPatternRangeEndpoint_Start,
                )?;
                if c <= 0 {
                    break;
                }
            }
            let hit = range_text(&m, 200)?;
            if hit.is_empty() {
                break;
            }
            let mut rect = bounding_rects(&m)
                .into_iter()
                .find(|x| x[2] > 0.0 && x[3] > 0.0)
                .map(|x| {
                    [
                        x[0].round() as i32,
                        x[1].round() as i32,
                        x[2].round() as i32,
                        x[3].round() as i32,
                    ]
                });
            if rect.is_some() && bx.is_some() && !intersects(rect, bx) {
                rect = None;
            }
            let mut text = hit.clone();
            if found < FIND_LINE_CONTEXT {
                if let Ok(ln) = m.Clone() {
                    if ln.ExpandToEnclosingUnit(TextUnit_Line).is_ok() {
                        if let Ok(t) = range_text(&ln, 400) {
                            let t = t.trim_end_matches(['\r', '\n']);
                            if t.to_lowercase().contains(&hit.to_lowercase()) {
                                text = t.to_string();
                            }
                        }
                    }
                }
            }
            let mut d = json!({ "n": found, "match": hit, "text": text, "rect": rect_json(rect) });
            if rect.is_some() && asked < FIND_TARGET_MAX && asking < FIND_TARGET_BUDGET {
                asked += 1;
                let t = Instant::now();
                let via = range_target(u, &m, el, top);
                asking += t.elapsed().as_millis();
                if let Some(via) = via {
                    d["via"] = via;
                }
            }
            matches.push(d);
            found += 1;
            rest.MoveEndpointByRange(
                TextPatternRangeEndpoint_Start,
                &m,
                TextPatternRangeEndpoint_End,
            )?;
            let empty = rest.CompareEndpoints(
                TextPatternRangeEndpoint_Start,
                &rest,
                TextPatternRangeEndpoint_End,
            )?;
            if empty >= 0 {
                break;
            }
            last = Some(m);
        }
    }
    Ok(Some(Found {
        // Where the document sits: Find drops its own estimates there.
        doc: json!({ "ref": ref_str(r), "box": rect_json(bx), "matches": matches, "truncated": cut }),
        found,
        cut,
    }))
}

pub(crate) fn find_text_cmd(mut a: Value) -> DResult<Value> {
    let query = str_arg(&a, "query", "");
    if query.trim().is_empty() {
        return Err(err("BAD_ARGS", "findText needs a query"));
    }
    let max = int_arg(&a, "max", 400)?.max(1);
    let budget = int_arg(&a, "budgetMs", 700)?.max(50) as u128;
    let t0 = Instant::now();
    let mut docs = Vec::new();
    let mut total = 0;
    let mut truncated = false;
    a["depth"] = json!(int_arg(&a, "depth", 60)?);
    let u = uia()?;
    let (w, _) = {
        let mut on = |w: &mut Walk, el: &IUIAutomationElement, i: &Info, r: i32, _: &str| {
            if truncated {
                return false;
            }
            if !i.text || i.password || r <= 0 {
                return true;
            }
            if !matches!(i.control_type, CT_DOCUMENT | CT_EDIT) {
                return true;
            }
            let got = find_in_text(&u, el, r, i.rect, w.top, &query, max - total, t0, budget);
            let Ok(Some(f)) = got else {
                return true;
            };
            docs.push(f.doc);
            total += f.found;
            if f.cut {
                truncated = true;
            }
            // The document's own text covers its children.
            false
        };
        run_walk(&a, "visible", Some(&mut on))?
    };
    Ok(json!({
        "windowHandle": handle_str(w.top),
        "docs": docs,
        "total": total,
        "truncated": truncated,
        "findMs": t0.elapsed().as_millis() as u64,
    }))
}

/// Ask's whole-document read: each document's or editor's full text, in
/// view or scrolled away, up to maxChars across the window. "seen" is the
/// start of the first range the application reports as visible.
pub(crate) fn doc_text_cmd(mut a: Value) -> DResult<Value> {
    let max_chars = int_arg(&a, "maxChars", 60000)?.max(100) as usize;
    let budget = int_arg(&a, "budgetMs", 700)?.max(50) as u128;
    let t0 = Instant::now();
    let mut docs = Vec::new();
    let mut total = 0usize;
    let mut truncated = false;
    a["depth"] = json!(int_arg(&a, "depth", 60)?);
    let (w, _) = {
        let mut on = |_: &mut Walk, el: &IUIAutomationElement, i: &Info, r: i32, _: &str| {
            if truncated {
                return false;
            }
            if total >= max_chars || t0.elapsed().as_millis() > budget {
                truncated = true;
                return false;
            }
            if !i.text || i.password || r <= 0 {
                return true;
            }
            if !matches!(i.control_type, CT_DOCUMENT | CT_EDIT) {
                return true;
            }
            let Some(tp) = text_pattern(el) else {
                return true;
            };
            let room = max_chars - total;
            // One character more than fits shows that there was more.
            let text = unsafe {
                tp.DocumentRange()
                    .and_then(|d| range_text(&d, room as i32 + 1))
            };
            let Ok(mut text) = text else {
                return true;
            };
            let seen = unsafe {
                tp.GetVisibleRanges()
                    .ok()
                    .and_then(|v| ranges(&v).into_iter().next())
                    .and_then(|v| range_text(&v, 400).ok())
                    .unwrap_or_default()
            };
            if text.trim().is_empty() {
                return false;
            }
            let cut = u16len(&text) > room;
            if cut {
                text = take_u16(&text, room);
                truncated = true;
            }
            total += u16len(&text);
            docs.push(json!({ "ref": ref_str(r), "text": text, "seen": seen, "truncated": cut }));
            // The document's own text covers its children.
            false
        };
        run_walk(&a, "visible", Some(&mut on))?
    };
    Ok(json!({
        "windowHandle": handle_str(w.top),
        "docs": docs,
        "total": total,
        "truncated": truncated,
        "readMs": t0.elapsed().as_millis() as u64,
    }))
}

/// Match `n` of `query` in the document, as findText numbered it.
fn nth_match(
    rest: &IUIAutomationTextRange,
    query: &str,
    n: i32,
) -> DResult<IUIAutomationTextRange> {
    let q = BSTR::from(query);
    let mut m = None;
    for _ in 0..=n {
        let Ok(found) = (unsafe { rest.FindText(&q, false, true) }) else {
            return Err(err("NOT_FOUND", "that match is no longer in the document"));
        };
        unsafe {
            rest.MoveEndpointByRange(
                TextPatternRangeEndpoint_Start,
                &found,
                TextPatternRangeEndpoint_End,
            )?;
        }
        m = Some(found);
    }
    m.ok_or_else(|| err("NOT_FOUND", "that match is no longer in the document"))
}

/// Brings match n of a document (as findText numbered it) into view.
pub(crate) fn scroll_to_text(a: &Value) -> DResult<Value> {
    let e = get_ref(arg(a, "ref"))?;
    refuse_elevated(e.top)?;
    let query = str_arg(a, "query", "");
    if query.trim().is_empty() {
        return Err(err("BAD_ARGS", "scrollToText needs a query"));
    }
    let n = int_arg(a, "n", 0)?.max(0);
    let Some(tp) = text_pattern(&e.el.0) else {
        return Err(err(
            "NO_PATTERN",
            "that element has no text to scroll through (no TextPattern)",
        ));
    };
    let rest = unsafe { tp.DocumentRange()?.Clone()? };
    let m = nth_match(&rest, &query, n)?;
    check_cancel()?;
    unsafe { m.ScrollIntoView(true)? };
    sleep_ms(80);
    Ok(json!({ "ok": true }))
}

fn point_to_rect(x: i32, y: i32, r: &[f64; 4]) -> i64 {
    let (x, y) = (f64::from(x), f64::from(y));
    let (l, t, rr, b) = (r[0], r[1], r[0] + r[2], r[1] + r[3]);
    let dx = if x < l {
        l - x
    } else if x > rr {
        x - rr
    } else {
        0.0
    };
    let dy = if y < t {
        t - y
    } else if y > b {
        y - b
    } else {
        0.0
    };
    (dx * dx + dy * dy) as i64
}

/// Find's "act" on words in a document: selects match n (as findText
/// numbered it), or with a point the match nearest to it, brings it into
/// view and gives the document the keyboard. Nothing is clicked.
pub(crate) fn select_text(a: &Value) -> DResult<Value> {
    let e = get_ref(arg(a, "ref"))?;
    refuse_elevated(e.top)?;
    let query = str_arg(a, "query", "");
    if query.trim().is_empty() {
        return Err(err("BAD_ARGS", "selectText needs a query"));
    }
    let n = int_arg(a, "n", 0)?.max(0);
    let near = arg(a, "x").is_some() && arg(a, "y").is_some();
    if pbool(&e.el.0, UIA_IsPasswordPropertyId, false)? {
        return Err(err(
            "NO_PATTERN",
            "a password field is never read or selected",
        ));
    }
    let Some(tp) = text_pattern(&e.el.0) else {
        return Err(err(
            "NO_PATTERN",
            "that element has no text to select (no TextPattern)",
        ));
    };
    let rest = unsafe { tp.DocumentRange()?.Clone()? };
    let m = if !near {
        nth_match(&rest, &query, n)?
    } else {
        let (px, py) = (require_int(a, "x")?, require_int(a, "y")?);
        let q = BSTR::from(query.as_str());
        let mut best = i64::MAX;
        let mut pick: Option<IUIAutomationTextRange> = None;
        let mut first: Option<IUIAutomationTextRange> = None;
        let mut last: Option<IUIAutomationTextRange> = None;
        let t0 = Instant::now();
        let mut k = 0;
        unsafe {
            while k < 2000 && t0.elapsed().as_millis() < 400 {
                k += 1;
                let Ok(c) = rest.FindText(&q, false, true) else {
                    break;
                };
                if let Some(l) = &last {
                    let o = c.CompareEndpoints(
                        TextPatternRangeEndpoint_Start,
                        l,
                        TextPatternRangeEndpoint_Start,
                    )?;
                    if o <= 0 {
                        break;
                    }
                }
                if first.is_none() {
                    first = Some(c.clone());
                }
                for x in bounding_rects(&c) {
                    if x[2] <= 0.0 || x[3] <= 0.0 {
                        continue;
                    }
                    let dist = point_to_rect(px, py, &x);
                    if dist < best {
                        best = dist;
                        pick = Some(c.clone());
                    }
                }
                if best == 0 {
                    break;
                }
                rest.MoveEndpointByRange(
                    TextPatternRangeEndpoint_Start,
                    &c,
                    TextPatternRangeEndpoint_End,
                )?;
                let empty = rest.CompareEndpoints(
                    TextPatternRangeEndpoint_Start,
                    &rest,
                    TextPatternRangeEndpoint_End,
                )?;
                if empty >= 0 {
                    break;
                }
                last = Some(c);
            }
        }
        pick.or(first)
            .ok_or_else(|| err("NOT_FOUND", "that text is no longer in the document"))?
    };
    check_cancel()?;
    let focus = focus_element(&e.el).unwrap_or("window");
    unsafe {
        let _ = m.ScrollIntoView(true);
        if let Err(x) = m.Select() {
            return Err(err(
                "NO_PATTERN",
                format!("that text cannot be selected ({})", x.message()),
            ));
        }
    }
    sleep_ms(60);
    let got = range_text(&m, 400).ok();
    Ok(json!({ "ok": true, "focus": focus, "selected": got }))
}

/// What is selected in a document or editor, for the self-check that proves
/// selectText. A password field answers with nothing.
pub(crate) fn selection_cmd(a: &Value) -> DResult<Value> {
    let e = get_ref(arg(a, "ref"))?;
    if pbool(&e.el.0, UIA_IsPasswordPropertyId, false)? {
        return Ok(json!({ "ranges": [], "text": "" }));
    }
    let Some(tp) = text_pattern(&e.el.0) else {
        return Err(err(
            "NO_PATTERN",
            "that element has no text (no TextPattern)",
        ));
    };
    let sel = unsafe { tp.GetSelection()? };
    let list: Vec<String> = ranges(&sel)
        .iter()
        .filter_map(|r| range_text(r, 2000).ok())
        .collect();
    let text = list.first().cloned().unwrap_or_default();
    Ok(json!({ "ranges": list, "text": text }))
}

// ------------------------------------------------------------ focus

fn is_focusable(el: &IUIAutomationElement) -> bool {
    matches!(
        prop(el, UIA_IsKeyboardFocusablePropertyId, false),
        Ok(Var::Bool(true))
    )
}

/// An element can be the right target and still refuse SetFocus: a classic
/// Win32 edit exposed through the MSAA proxy reports itself as a Document
/// that "cannot receive focus" while the HWND underneath takes the keyboard
/// fine. So focus goes to the element if it takes it, else to its nearest
/// focusable descendant, else to a focusable ancestor short of the window,
/// else nowhere: the window is already in front and its own focus is where
/// keystrokes land.
pub(crate) fn focusable_near(el: &El) -> Option<El> {
    if is_focusable(&el.0) {
        return Some(el.clone());
    }
    let u = uia().ok()?;
    let walker = &u.walker;
    let mut queue = VecDeque::from([el.0.clone()]);
    let mut seen = 0;
    unsafe {
        while let Some(cur) = queue.pop_front() {
            if seen >= 200 {
                break;
            }
            let mut c = walker.GetFirstChildElement(&cur).ok();
            while let Some(x) = c {
                if seen >= 200 {
                    break;
                }
                seen += 1;
                if is_focusable(&x) {
                    return Some(El(x));
                }
                c = walker.GetNextSiblingElement(&x).ok();
                queue.push_back(x);
            }
        }
        let mut p = walker.GetParentElement(&el.0).ok();
        for _ in 0..4 {
            let Some(x) = p else { break };
            match x.CurrentControlType() {
                Ok(ct) if ct.0 == CT_WINDOW => break,
                Ok(_) => {}
                Err(_) => break,
            }
            if is_focusable(&x) {
                return Some(El(x));
            }
            p = walker.GetParentElement(&x).ok();
        }
    }
    None
}

pub(crate) fn focus_element(el: &El) -> DResult<&'static str> {
    let Some(f) = focusable_near(el) else {
        return Ok("window");
    };
    check_cancel()?;
    if unsafe { f.0.SetFocus() }.is_err() {
        return Ok("window");
    }
    let same = unsafe { uia()?.a.CompareElements(&f.0, &el.0) }
        .map(|b| b.as_bool())
        .unwrap_or(false);
    Ok(if same { "element" } else { "near" })
}

/// Where to point at an element: its centre, scrolled into view first when
/// it is offscreen and can scroll itself.
pub(crate) fn point_for(e: &RefEntry) -> DResult<([i32; 2], Info)> {
    let mut info = read(&e.el.0, false)?;
    if info.offscreen && info.scroll_item {
        let p: Option<IUIAutomationScrollItemPattern> = pattern(&e.el.0, UIA_ScrollItemPatternId);
        if let Some(p) = p {
            if unsafe { p.ScrollIntoView() }.is_ok() {
                sleep_ms(150);
                info = read(&e.el.0, false)?;
            }
        }
    }
    if let Some(r) = info.rect.filter(|r| r[2] > 0 && r[3] > 0) {
        return Ok(([r[0] + r[2] / 2, r[1] + r[3] / 2], info));
    }
    let mut pt = POINT::default();
    if let Ok(got) = unsafe { e.el.0.GetClickablePoint(&mut pt) } {
        if got.as_bool() {
            return Ok(([pt.x, pt.y], info));
        }
    }
    Err(err("NOT_FOUND", "that element has no on-screen rectangle"))
}

// ------------------------------------------------------------ point reads

pub(crate) fn at(a: &Value) -> DResult<Value> {
    let (x, y) = (require_int(a, "x")?, require_int(a, "y")?);
    let hit = window_at(x, y);
    let top = root(hit);
    if is_excluded_window(hit) {
        return Err(err(
            "BLOCKED_OWN_WINDOW",
            "that point is on Aevaleron's own window",
        ));
    }
    let u = uia()?;
    let el = unsafe {
        u.a.ElementFromPointBuildCache(POINT { x, y }, &u.el_cache)?
    };
    let i = read(&el, true)?;
    if is_excluded_pid(i.pid as u32) {
        return Err(err(
            "BLOCKED_OWN_WINDOW",
            "that point is on Aevaleron's own window",
        ));
    }
    let r = ref_for(&el, &i, top);
    Ok(json!({ "node": node_dict(&i, r, 0, 0, 300), "windowHandle": handle_str(top) }))
}

/// The element with keyboard focus, so the agent's gate can see what a
/// Space or Enter would press. node is null when the window asked for is
/// not in front: keys sent to it land on its own remembered focus, which UI
/// Automation cannot see from outside.
pub(crate) fn focused_cmd(a: &Value) -> DResult<Value> {
    let mut want = super::parse_handle(a.get("windowHandle"))?;
    if want != 0 {
        want = root(want);
    }
    let fg = foreground_root();
    let mut res = json!({ "node": null, "windowHandle": handle_str(fg) });
    if fg == 0 || is_excluded_window(fg) || (want != 0 && want != fg) {
        return Ok(res);
    }
    let u = uia()?;
    let Ok(el) = (unsafe { u.a.GetFocusedElementBuildCache(&u.el_cache) }) else {
        return Ok(res);
    };
    let i = read(&el, true)?;
    if is_excluded_pid(i.pid as u32) {
        return Ok(res);
    }
    let r = ref_for(&el, &i, fg);
    res["node"] = node_dict(&i, r, 0, 0, 300);
    Ok(res)
}

pub(crate) fn resolve(a: &Value) -> DResult<Value> {
    let e = get_ref(arg(a, "ref"))?;
    let i = read(&e.el.0, false)?;
    let value_max = int_arg(a, "valueMax", 4000)?.max(0) as usize;
    let r = parse_ref(arg(a, "ref"))?;
    Ok(json!({
        "node": node_dict(&i, r, 0, 0, value_max),
        "windowHandle": handle_str(e.top),
    }))
}

// ------------------------------------------------------------ for menus and setValue

/// Children (or descendants) of `el` with the menu cache: control type,
/// name, enabled, process and the two pattern flags.
pub(crate) fn menu_kids(el: &IUIAutomationElement, deep: bool) -> DResult<Vec<El>> {
    let u = uia()?;
    let scope = if deep {
        TreeScope_Descendants
    } else {
        TreeScope_Children
    };
    let arr = unsafe { el.FindAllBuildCache(scope, &u.all, &u.menu_cache)? };
    Ok(element_array(&arr).into_iter().map(El).collect())
}

pub(crate) struct MenuInfo {
    pub control_type: i32,
    pub name: String,
    pub enabled: bool,
    pub sub: bool,
    pub selectable: bool,
}

pub(crate) fn menu_info(el: &IUIAutomationElement) -> MenuInfo {
    let ct = pint(el, UIA_ControlTypePropertyId, true).ok().flatten();
    MenuInfo {
        control_type: ct.unwrap_or(0),
        name: pstr(el, UIA_NamePropertyId, true).unwrap_or_default(),
        enabled: !matches!(
            prop(el, UIA_IsEnabledPropertyId, true),
            Ok(Var::Bool(false))
        ),
        sub: pbool(el, UIA_IsExpandCollapsePatternAvailablePropertyId, true).unwrap_or(false),
        selectable: pbool(el, UIA_IsSelectionItemPatternAvailablePropertyId, true).unwrap_or(false),
    }
}

pub(crate) const MENU_ITEM: i32 = CT_MENUITEM;
pub(crate) const MENU: i32 = CT_MENU;
pub(crate) const MENU_BAR: i32 = CT_MENUBAR;
pub(crate) const PANE: i32 = CT_PANE;
pub(crate) const GROUP: i32 = CT_GROUP;
pub(crate) const CUSTOM: i32 = CT_CUSTOM;
pub(crate) const TOOLBAR: i32 = CT_TOOLBAR;
pub(crate) const WINDOW: i32 = CT_WINDOW;

/// A menu item named `name` under `under`: the first MenuItem with that
/// name, one level down or anywhere below.
pub(crate) fn menu_child(under: &IUIAutomationElement, name: &str, deep: bool) -> Option<El> {
    let u = uia().ok()?;
    unsafe {
        let ct =
            u.a.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(CT_MENUITEM))
                .ok()?;
        let nm =
            u.a.CreatePropertyCondition(UIA_NamePropertyId, &VARIANT::from(name))
                .ok()?;
        let both = u.a.CreateAndCondition(&ct, &nm).ok()?;
        let scope = if deep {
            TreeScope_Descendants
        } else {
            TreeScope_Children
        };
        under.FindFirst(scope, &both).ok().map(El)
    }
}

/// The options of a combo box or list: every descendant that can be
/// selected, with its name.
pub(crate) fn selectable_descendants(el: &El) -> DResult<Vec<(El, String)>> {
    Ok(menu_kids(&el.0, true)?
        .into_iter()
        .filter_map(|k| {
            let m = menu_info(&k.0);
            m.selectable.then_some((k, m.name))
        })
        .collect())
}

pub(crate) fn element_from_handle(h: isize) -> DResult<El> {
    let u = uia()?;
    Ok(El(unsafe { u.a.ElementFromHandle(hwnd(h))? }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_follow_dotnet_names() {
        assert_eq!(role_of(CT_BUTTON), Some("button"));
        assert_eq!(role_of(CT_SEPARATOR), Some("separator"));
        assert_eq!(role_of(CT_DOCUMENT), Some("document"));
        assert_eq!(role_of(CT_PANE), Some("pane"));
        assert_eq!(role_of(50040), None);
        assert_eq!(role_of(0), None);
    }

    #[test]
    fn variants_parse() {
        assert_eq!(parse_var(&VARIANT::from(true)), Var::Bool(true));
        assert_eq!(parse_var(&VARIANT::from(false)), Var::Bool(false));
        assert_eq!(parse_var(&VARIANT::from(42i32)), Var::Int(42));
        assert_eq!(parse_var(&VARIANT::from("hi")), Var::Str("hi".into()));
        assert_eq!(parse_var(&VARIANT::default()), Var::Other);
    }

    #[test]
    fn nodes_have_host_cs_shape() {
        let i = Info {
            name: "Send".into(),
            role: "button".into(),
            control_type: CT_BUTTON,
            enabled: true,
            invoke: true,
            focused: true,
            rect: Some([1, 2, 3, 4]),
            ..Info::default()
        };
        let n = node_dict(&i, 7, 3, 2, 300);
        assert_eq!(n["ref"], "ref_7");
        assert_eq!(n["parentRef"], "ref_3");
        assert_eq!(n["rect"], json!([1, 2, 3, 4]));
        assert_eq!(n["states"], json!(["focused"]));
        assert_eq!(n["patterns"], json!(["invoke"]));
        assert_eq!(n["interactive"], json!(true));
        assert_eq!(n["automationId"], Value::Null);
        let keys: Vec<&str> = n.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(&keys[..4], &["ref", "parentRef", "depth", "role"]);
    }

    #[test]
    fn unions_and_distances() {
        assert_eq!(
            union(&[[0.0, 0.0, 10.0, 10.0], [20.0, 5.0, 5.0, 10.0]]),
            Some([0, 0, 25, 15])
        );
        assert_eq!(union(&[]), None);
        assert_eq!(point_to_rect(5, 5, &[0.0, 0.0, 10.0, 10.0]), 0);
        assert_eq!(point_to_rect(13, 14, &[0.0, 0.0, 10.0, 10.0]), 25);
    }

    #[test]
    fn noise_and_interactive_rules() {
        let doc = Info {
            control_type: CT_DOCUMENT,
            focusable: true,
            text: true,
            ..Info::default()
        };
        assert!(is_interactive(&doc));
        let pane = Info {
            control_type: CT_PANE,
            focusable: true,
            name: "x".into(),
            ..Info::default()
        };
        assert!(!is_interactive(&pane));
    }

    /// UI Automation itself, on the test runner's desktop: the instance is
    /// made in the MTA and reads the desktop's root window.
    #[test]
    fn automation_reads_the_desktop() {
        let rx = super::super::spawn_op(None, || {
            let u = uia()?;
            let root = unsafe { u.a.GetRootElement()? };
            let c = unsafe { root.BuildUpdatedCache(&u.el_cache)? };
            read(&c, true).map(|i| i.role)
        });
        let role = rx.recv().unwrap().unwrap();
        assert_eq!(role.unwrap(), "pane");
    }
}
