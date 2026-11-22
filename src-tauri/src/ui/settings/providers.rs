//! Settings' Providers and Models pages (settings.js: providers, the
//! clipboard key, Connect OpenRouter, order, failover, key storage, who
//! receives the screen, the models and their three pickers).

use std::collections::HashMap;

use serde_json::{json, Value};
use slint::{Model, ModelRc, SharedString, VecModel};

use super::{ago, ask, clock, flag, now_ms, num, said_no, text, Reply, SettingsPage};
use crate::ui::{SetChoice, SetComboRow, SetComboView, SetProvAction, SetProvRow};

const DEFAULT_ORDER: [&str; 4] = ["openrouter", "anthropic", "openai", "gemini"];
const PRIVACY_URL: &str = "https://openrouter.ai/settings/privacy";
const ORDINAL: [&str; 6] = ["first", "second", "third", "fourth", "fifth", "sixth"];
/// Where the models' mode is kept (it was the page's localStorage).
const MODE_KEY: &str = "settingsModelsMode";

struct Info {
    name: &'static str,
    offer: &'static str,
    placeholder: &'static str,
    key_url: &'static str,
    key_host: &'static str,
    credit_url: &'static str,
}

fn info(id: &str) -> Option<Info> {
    Some(match id {
        "openrouter" => Info {
            name: "OpenRouter",
            offer: "Free and paid models",
            placeholder: "sk-or-v1-…",
            key_url: "https://openrouter.ai/keys",
            key_host: "openrouter.ai/keys",
            credit_url: "https://openrouter.ai/settings/credits",
        },
        "anthropic" => Info {
            name: "Anthropic",
            offer: "Claude models",
            placeholder: "sk-ant-…",
            key_url: "https://console.anthropic.com/settings/keys",
            key_host: "console.anthropic.com",
            credit_url: "https://console.anthropic.com/settings/billing",
        },
        "openai" => Info {
            name: "OpenAI",
            offer: "GPT models",
            placeholder: "sk-…",
            key_url: "https://platform.openai.com/api-keys",
            key_host: "platform.openai.com/api-keys",
            credit_url: "https://platform.openai.com/settings/organization/billing/overview",
        },
        "gemini" => Info {
            name: "Google Gemini",
            offer: "Gemini models",
            placeholder: "AIza…",
            key_url: "https://aistudio.google.com/apikey",
            key_host: "aistudio.google.com/apikey",
            credit_url: "https://aistudio.google.com/apikey",
        },
        _ => return None,
    })
}

fn name_of(id: &str) -> String {
    info(id).map_or_else(|| id.to_string(), |i| i.name.to_string())
}

fn article(name: &str) -> &'static str {
    let first = name.chars().next().unwrap_or(' ').to_ascii_lowercase();
    if "aeiou".contains(first) {
        "an"
    } else {
        "a"
    }
}

/// Every state is a word plus an icon; the icon is decoration.
fn status_info(s: &str) -> (&'static str, &'static str, &'static str) {
    match s {
        "not-added" => ("Not added", "\u{EA3A}", ""),
        "checking" => ("Checking…", "\u{E72C}", ""),
        "ready" => ("Ready", "\u{E930}", "ok"),
        "rejected" => ("Key rejected", "\u{EA39}", "danger"),
        "no-credit" => ("No credit", "\u{E7BA}", "warn"),
        "busy" => ("Busy", "\u{E823}", ""),
        "blocked" => ("Free models off", "\u{E7BA}", "warn"),
        // Before Rust reports statuses: a key is stored, nothing more.
        "saved" => ("Saved", "\u{E73E}", ""),
        _ => ("Not checked", "\u{E738}", ""),
    }
}

/// The provider a pasted key belongs to, by its prefix.
fn detect_provider(key: &str) -> &'static str {
    let k = key.trim();
    if k.starts_with("sk-or-") {
        "openrouter"
    } else if k.starts_with("sk-ant-") {
        "anthropic"
    } else if k.starts_with("AIza") {
        "gemini"
    } else if k.starts_with("sk-") {
        "openai"
    } else {
        ""
    }
}

#[derive(Clone)]
pub(super) struct Prov {
    pub(super) id: String,
    name: String,
    has_key: bool,
    key_hint: String,
    status: String,
    checked_at: f64,
    busy_until: f64,
    credit: Option<f64>,
}

/// What a row is in the middle of, on this page only.
#[derive(Clone, Default)]
struct ProvUi {
    testing: bool,
    removing: bool,
    connecting: bool,
    detected: String,
    error: String,
    /// "", "paste" or "replace".
    mode: String,
    /// The masked key on the clipboard.
    clip: String,
}

#[derive(Clone, Default)]
struct ModelInfo {
    id: String,
    name: String,
    tag: String,
    note: String,
    detail: String,
}

/// One of the three pickers (screen, answer and task model).
#[derive(Default)]
struct Picker {
    current: String,
    groups: Vec<(String, Vec<ModelInfo>)>,
    offline: String,
    loaded: bool,
}

impl Picker {
    fn known(&self) -> Option<&ModelInfo> {
        self.groups
            .iter()
            .flat_map(|(_, m)| m.iter())
            .find(|m| m.id == self.current)
    }
}

#[derive(Default)]
pub(super) struct Providers {
    pub(super) list: Vec<Prov>,
    ui: HashMap<String, ProvUi>,
    ordered: bool,
    /// What each key field holds.
    keys: HashMap<String, String>,
    /// A field takes `keys` when its epoch changes, the keyboard when its
    /// focus changes; `next` makes every value new.
    epochs: HashMap<String, i32>,
    focus: HashMap<String, i32>,
    next: i32,
    row_ids: Vec<String>,
    /// The models: "auto" or "custom", and whose models are listed.
    mode: String,
    provider: String,
    pickers: HashMap<&'static str, Picker>,
}

fn answers(p: &Prov) -> bool {
    p.status == "ready" || p.status == "saved"
}

fn credit_of(v: &Value) -> Option<f64> {
    match v {
        Value::Object(_) => v["remaining"].as_f64(),
        _ => v.as_f64(),
    }
}

/// The providers in Rust's order, or the four in the old order from an
/// older build's presence map.
fn providers_from(d: &Value) -> (Vec<Prov>, bool) {
    let hints = &d["keyHints"];
    if let Some(list) = d["providers"].as_array().filter(|l| !l.is_empty()) {
        let mut ps: Vec<&Value> = list
            .iter()
            .filter(|p| info(p["id"].as_str().unwrap_or_default()).is_some())
            .collect();
        ps.sort_by(|a, b| num(a, "order").total_cmp(&num(b, "order")));
        let out = ps
            .into_iter()
            .map(|p| {
                let id = text(p, "id");
                let has_key = flag(p, "hasKey");
                let mut key_hint = text(p, "keyHint");
                if key_hint.is_empty() {
                    key_hint = text(hints, &id);
                }
                let status = if has_key {
                    Some(text(p, "status"))
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "saved".into())
                } else {
                    "not-added".into()
                };
                Prov {
                    name: name_of(&id),
                    has_key,
                    key_hint,
                    status,
                    checked_at: num(p, "checkedAt"),
                    busy_until: num(p, "busyUntil"),
                    credit: credit_of(&p["credit"]),
                    id,
                }
            })
            .collect();
        return (out, true);
    }
    let keys = if d["keys"].is_object() {
        d["keys"].clone()
    } else {
        json!({ "openrouter": flag(d, "hasKey") })
    };
    let out = DEFAULT_ORDER
        .iter()
        .map(|id| {
            let has_key = flag(&keys, id);
            Prov {
                id: (*id).to_string(),
                name: name_of(id),
                has_key,
                key_hint: text(hints, id),
                status: if has_key { "saved" } else { "not-added" }.into(),
                checked_at: 0.0,
                busy_until: 0.0,
                credit: None,
            }
        })
        .collect();
    (out, false)
}

/// One action under a provider.
struct Act {
    id: &'static str,
    label: String,
    kind: &'static str,
    disabled: bool,
    href: String,
}

fn act(id: &'static str, label: &str, kind: &'static str) -> Act {
    Act {
        id,
        label: label.to_string(),
        kind,
        disabled: false,
        href: String::new(),
    }
}

fn link(label: &str, href: &str, button: bool) -> Act {
    Act {
        id: "",
        label: label.to_string(),
        kind: if button { "link-button" } else { "link" },
        disabled: false,
        href: href.to_string(),
    }
}

struct View {
    status: String,
    msg: String,
    field: bool,
    field_label: String,
    actions: Vec<Act>,
    order: bool,
    busy: bool,
}

fn credit_text(p: &Prov) -> String {
    if p.id != "openrouter" {
        return String::new();
    }
    match p.credit {
        Some(c) if c <= 0.0 => " Free models only.".into(),
        Some(c) => format!(" About ${c:.2} of credit left."),
        None => String::new(),
    }
}

fn busy_text(p: &Prov, list: &[Prov]) -> String {
    let until = if p.busy_until > 0.0 {
        format!(" until about {}", clock(p.busy_until))
    } else {
        " for a minute".into()
    };
    let head = if p.id == "openrouter" {
        "OpenRouter's free models are busy.".to_string()
    } else {
        format!("{} is limiting requests right now.", p.name)
    };
    match list.iter().find(|q| q.id != p.id && answers(q)) {
        Some(next) => format!(
            "{head} Your key is fine. Questions go to {}{until}.",
            next.name
        ),
        None => format!("{head} Your key is fine. Questions wait and retry{until}."),
    }
}

/// What an open row says and offers, from its status and what is going on
/// in it right now (design-ux 3.2).
fn view_of(p: &Prov, u: &ProvUi, list: &[Prov]) -> View {
    let n = p.name.as_str();
    let inf = info(&p.id);
    let (key_url, key_host, credit_url) = inf
        .as_ref()
        .map_or(("", "", ""), |i| (i.key_url, i.key_host, i.credit_url));
    let mut v = View {
        status: p.status.clone(),
        msg: String::new(),
        field: false,
        field_label: format!("{n} API key"),
        actions: Vec::new(),
        order: true,
        busy: false,
    };
    let get_key = || link(&format!("Get a key at {key_host}"), key_url, false);
    let or = p.id == "openrouter";

    if u.testing || p.status == "checking" {
        v.status = "checking".into();
        v.msg = format!("Checking the key with {n}…");
        v.busy = true;
        // The field stays, disabled, while the key in it is checked.
        v.field = if p.has_key {
            !u.mode.is_empty()
        } else {
            !or || u.mode == "paste"
        };
        return v;
    }
    if u.removing {
        v.msg = format!("Remove the {n} key from this PC? This doesn't revoke it at {n}.");
        v.actions = vec![
            act("remove-yes", "Remove", "danger"),
            act("remove-no", "Keep", ""),
        ];
        v.order = false;
        return v;
    }
    if !u.detected.is_empty() {
        let dn = name_of(&u.detected);
        v.msg = format!("This is {} {dn} key.", article(&dn));
        v.field = true;
        v.actions = vec![act("save-under", &format!("Save it under {dn}"), "prominent")];
        return v;
    }
    if !u.error.is_empty() {
        v.msg = u.error.clone();
        if !p.has_key && or && u.mode != "paste" {
            // Connecting failed: try again, or paste a key.
            v.actions = vec![
                act("connect", "Connect OpenRouter", "prominent"),
                act("paste", "Paste a key instead", "text"),
            ];
            return v;
        }
        v.field = !p.has_key || !u.mode.is_empty();
        v.actions = if p.has_key {
            vec![act("cancel", "Cancel", "")]
        } else if or {
            vec![get_key(), act("connect", "Connect instead", "text")]
        } else {
            vec![get_key()]
        };
        return v;
    }
    if u.mode == "replace" && p.has_key {
        v.msg = "Paste the new key. The one saved now stays until the new one works.".into();
        v.field = true;
        v.field_label = format!("New {n} API key");
        v.actions = vec![act("cancel", "Cancel", ""), get_key()];
        return v;
    }

    match p.status.as_str() {
        "not-added" => {
            if or {
                v.msg = "Free models, no card needed. Connecting takes about a minute.".into();
                if u.connecting {
                    v.msg = "Approve Aevaleron in the OpenRouter tab that opened in your browser. This page carries on when you have.".into();
                    let mut wait = act("connect", "Waiting for OpenRouter…", "prominent");
                    wait.disabled = true;
                    v.actions = vec![wait, act("paste", "Paste a key instead", "text")];
                } else if u.mode == "paste" {
                    v.field = true;
                    v.actions = vec![get_key(), act("connect", "Connect instead", "text")];
                } else {
                    v.actions = vec![
                        act("connect", "Connect OpenRouter", "prominent"),
                        act("paste", "Paste a key instead", "text"),
                    ];
                }
            } else {
                v.msg = format!("Use your own {n} account. Each question is billed to it.");
                v.field = true;
                v.actions = vec![get_key()];
            }
        }
        "ready" => {
            let when = ago(p.checked_at);
            let when = if when.is_empty() { "just now".into() } else { when };
            v.msg = format!("Works. Checked {when}.{}", credit_text(p));
            v.actions = vec![
                act("test", "Test again", ""),
                act("replace", "Replace key", ""),
                act("remove", "Remove", ""),
            ];
        }
        "saved" => {
            v.msg = "Saved on this PC.".into();
            v.actions = vec![
                act("test", "Test", ""),
                act("replace", "Replace key", ""),
                act("remove", "Remove", ""),
            ];
        }
        "rejected" => {
            v.msg = format!("{n} rejected this key. It may have been revoked or mistyped. Aevaleron is skipping {n} until you replace it.");
            v.actions = vec![
                act("replace", "Replace key", "prominent"),
                act("remove", "Remove", ""),
            ];
        }
        "no-credit" => {
            v.msg = if or {
                "The key works, but the account has no credit, so only free models answer.".into()
            } else {
                format!("The key works, but the {n} account has no credit. Aevaleron is skipping {n}.")
            };
            v.actions = vec![
                link("Add credit", credit_url, true),
                act("test", "Test again", ""),
                act("remove", "Remove", "text"),
            ];
        }
        "busy" => {
            v.msg = busy_text(p, list);
            v.actions = vec![act("test", "Try now", "")];
        }
        "blocked" => {
            v.msg = "Your OpenRouter privacy setting blocks free models. They're offered on the condition that their providers may keep and train on what they receive, which includes your screen. Turn on \"Free model publication\" to use them, or add credit and pick a paid model.".into();
            v.actions = vec![
                link("Open OpenRouter privacy settings", PRIVACY_URL, true),
                act("test", "Test again", ""),
            ];
        }
        _ => {
            v.msg = format!("Can't reach {n} right now. The key is saved; Aevaleron checks again when you're online.");
            v.actions = vec![act("test", "Test again", ""), act("remove", "Remove", "text")];
        }
    }
    v
}

/// The status a test result means: Rust's word when it gives one, else
/// read from the sentence an older build returns.
fn status_of_test(r: &Reply) -> String {
    let Some(v) = r.value() else {
        return "unchecked".into();
    };
    let s = text(v, "status");
    if !s.is_empty() {
        return s;
    }
    if flag(v, "ok") {
        return "ready".into();
    }
    let m = format!("{} {}", text(v, "message"), text(v, "error")).to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| m.contains(w));
    if has(&["reject", "invalid", "not valid"]) {
        "rejected".into()
    } else if has(&["402", "credit", "quota", "billing"]) {
        "no-credit".into()
    } else if has(&["429", "rate limit", "busy"]) {
        "busy".into()
    } else if has(&["privacy", "data policy", "publication"]) {
        "blocked".into()
    } else {
        "unchecked".into()
    }
}

/// The credit an older build only writes into its sentence: "About $7.50
/// of credit left." or "the credit is used up".
fn credit_of_message(m: &str) -> Option<f64> {
    if let Some(at) = m.find(" of credit left") {
        let head = &m[..at];
        let dollar = head.rfind('$')?;
        return head[dollar + 1..].parse().ok();
    }
    if m.contains("credit is used up") {
        return Some(0.0);
    }
    None
}

impl SettingsPage {
    // ------------------------------------------------------------ load

    /// The first `settings:get`: everything on the page.
    pub(super) fn load(&self) {
        self.call("settings:get", Value::Null, 0, |p, r| {
            let Some(d) = r.value().cloned() else {
                return;
            };
            let (list, ordered) = providers_from(&d);
            let picks_default = {
                let dm = text(&d, "defaultModel");
                let da = text(&d, "defaultAgentModel");
                (dm.is_empty() || text(&d, "model") == dm)
                    && !flag(&d, "brainEnabled")
                    && (da.is_empty() || text(&d, "agentModel") == da)
            };
            let stored = crate::store::get()
                .raw(MODE_KEY)
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            {
                let mut st = p.st.borrow_mut();
                st.data = d.clone();
                st.prov.list = list;
                st.prov.ordered = ordered;
                st.prov.mode = if !picks_default || stored == "custom" {
                    "custom".into()
                } else {
                    "auto".into()
                };
            }
            p.render_all();
            p.render_providers();
            p.render_models_mode();
            p.reload_pickers();
            // With no provider yet, the OpenRouter row is open: Connect is
            // the first thing on the page.
            let first = {
                let st = p.st.borrow();
                if st.prov.list.iter().any(|q| q.has_key) {
                    None
                } else {
                    st.prov
                        .list
                        .iter()
                        .find(|q| q.id == "openrouter")
                        .or_else(|| st.prov.list.first())
                        .map(|q| q.id.clone())
                }
            };
            if let Some(id) = first {
                p.open_row(&format!("prov-{id}"));
            }
            p.check_clipboard_key();
        });
    }

    /// Rust's view after a change: statuses, hints, order and recipients,
    /// without undoing what a row is in the middle of.
    fn refresh_from_rust(&self) {
        self.call("settings:get", Value::Null, 0, |p, r| {
            let Some(d) = r.value().cloned() else {
                return;
            };
            let (mut fresh, ordered) = providers_from(&d);
            {
                let mut st = p.st.borrow_mut();
                for q in &mut fresh {
                    let Some(old) = st.prov.list.iter().find(|o| o.id == q.id) else {
                        continue;
                    };
                    let testing = st.prov.ui.get(&q.id).is_some_and(|u| u.testing);
                    if testing {
                        q.status = old.status.clone();
                    }
                    if q.checked_at <= 0.0 && old.checked_at > 0.0 {
                        q.checked_at = old.checked_at;
                    }
                    if q.status == "saved" && old.has_key && old.status != "saved" {
                        q.status = old.status.clone();
                    }
                }
                for k in [
                    "providers",
                    "keys",
                    "keyHints",
                    "keyStorage",
                    "failover",
                    "recipients",
                    "hasKey",
                ] {
                    st.data[k] = d[k].clone();
                }
                st.prov.list = fresh;
                st.prov.ordered = ordered;
            }
            p.render_key_storage();
            p.render_providers();
        });
    }

    // ------------------------------------------------------- the rows

    fn set_ui(&self, id: &str, f: impl FnOnce(&mut ProvUi)) {
        let mut st = self.st.borrow_mut();
        f(st.prov.ui.entry(id.to_string()).or_default());
    }

    fn pui(&self, id: &str) -> ProvUi {
        self.st.borrow().prov.ui.get(id).cloned().unwrap_or_default()
    }

    fn prov(&self, id: &str) -> Option<Prov> {
        self.st.borrow().prov.list.iter().find(|p| p.id == id).cloned()
    }

    fn with_prov(&self, id: &str, f: impl FnOnce(&mut Prov)) {
        let mut st = self.st.borrow_mut();
        if let Some(p) = st.prov.list.iter_mut().find(|p| p.id == id) {
            f(p);
        }
    }

    fn clear_field(&self, id: &str) {
        let mut st = self.st.borrow_mut();
        let pv = &mut st.prov;
        pv.keys.insert(id.to_string(), String::new());
        pv.next += 1;
        let n = pv.next;
        pv.epochs.insert(id.to_string(), n);
    }

    fn focus_field(&self, id: &str) {
        let mut st = self.st.borrow_mut();
        let pv = &mut st.prov;
        pv.next += 1;
        let n = pv.next;
        pv.focus.insert(id.to_string(), n);
    }

    pub(super) fn render_providers(&self) {
        let (rows, ordered) = {
            let mut st = self.st.borrow_mut();
            let pv = &mut st.prov;
            let list = pv.list.clone();
            let n = list.len();
            let def = list.iter().find(|p| answers(p)).map(|p| p.id.clone());
            let mut rows = Vec::with_capacity(n);
            for (i, p) in list.iter().enumerate() {
                // A row that now shows another provider takes that one's
                // field text.
                if pv.row_ids.get(i) != Some(&p.id) {
                    pv.next += 1;
                    let e = pv.next;
                    pv.epochs.insert(p.id.clone(), e);
                }
                let u = pv.ui.get(&p.id).cloned().unwrap_or_default();
                let v = view_of(p, &u, &list);
                let (word, icon, tone) = status_info(&v.status);
                let inf = info(&p.id);
                let sub = if p.key_hint.is_empty() {
                    inf.as_ref().map_or("", |i| i.offer).to_string()
                } else {
                    p.key_hint.clone()
                };
                let acts: Vec<SetProvAction> = v
                    .actions
                    .iter()
                    .map(|a| SetProvAction {
                        id: a.id.into(),
                        label: a.label.as_str().into(),
                        kind: a.kind.into(),
                        disabled: a.disabled,
                        href: a.href.as_str().into(),
                    })
                    .collect();
                let clip = if !u.clip.is_empty() && !p.has_key && !v.busy && !u.removing {
                    u.clip.clone()
                } else {
                    String::new()
                };
                rows.push(SetProvRow {
                    id: p.id.as_str().into(),
                    name: p.name.as_str().into(),
                    pos: (i + 1) as i32,
                    mono: !p.key_hint.is_empty(),
                    sub: sub.into(),
                    word: word.into(),
                    icon: icon.into(),
                    tone: tone.into(),
                    is_default: def.as_deref() == Some(p.id.as_str()),
                    msg: v.msg.as_str().into(),
                    msg_danger: (!u.error.is_empty() && !v.busy)
                        || (p.status == "rejected" && u.mode.is_empty() && !v.busy),
                    clip: clip.into(),
                    field: v.field,
                    field_label: v.field_label.as_str().into(),
                    placeholder: inf.as_ref().map_or("", |i| i.placeholder).into(),
                    busy: v.busy,
                    // One prominent button at a time: when the key belongs
                    // elsewhere, the way forward is "Save it under".
                    save_prominent: u.detected.is_empty(),
                    field_text: pv.keys.get(&p.id).cloned().unwrap_or_default().into(),
                    epoch: pv.epochs.get(&p.id).copied().unwrap_or(0),
                    focus: pv.focus.get(&p.id).copied().unwrap_or(0),
                    actions: ModelRc::new(VecModel::from(acts)),
                    order: v.order && pv.ordered && n > 1,
                    can_up: i > 0 && !v.busy,
                    can_down: i + 1 < n && !v.busy,
                    show_first: i > 0,
                });
            }
            pv.row_ids = list.iter().map(|p| p.id.clone()).collect();
            (rows, pv.ordered)
        };
        let m = &self.prov_model;
        for (i, r) in rows.iter().enumerate() {
            if i < m.row_count() {
                m.set_row_data(i, r.clone());
            } else {
                m.push(r.clone());
            }
        }
        while m.row_count() > rows.len() {
            m.remove(m.row_count() - 1);
        }
        self.s().set_ordered(ordered);
        self.cli_provider_picker();
        self.render_failover();
        self.render_recipients();
        self.render_model_providers();
    }

    // CLI provider picker (lane cli-providers): wire here
    // It renders nothing yet; that lane fills the slot in ProvidersPage.
    fn cli_provider_picker(&self) {
        self.s().set_cli_picker_shown(false);
    }

    fn announce_status(&self, id: &str) {
        let Some(p) = self.prov(id) else {
            return;
        };
        let n = &p.name;
        let t = match p.status.as_str() {
            "ready" => format!("{n} is ready."),
            "saved" => format!("{n} key saved."),
            "rejected" => format!("{n} rejected the key."),
            "no-credit" => format!("{n} has no credit."),
            "busy" => format!("{n} is busy."),
            "blocked" => "OpenRouter's free models are off.".into(),
            "unchecked" => format!("Couldn't check {n}."),
            "not-added" => format!("{n} key removed."),
            "checking" => format!("Checking {n}…"),
            _ => String::new(),
        };
        if !t.is_empty() {
            self.say(&t);
        }
    }

    fn apply_test(&self, id: &str, r: &Reply) {
        let status = status_of_test(r);
        let v = r.value().cloned();
        self.with_prov(id, |p| {
            p.status = if p.has_key { status } else { "not-added".into() };
            if p.status == "ready" {
                p.checked_at = now_ms();
            }
            let Some(v) = v else {
                return;
            };
            if v["credit"].is_null() {
                if let Some(c) = credit_of_message(&text(&v, "message")) {
                    p.credit = Some(c);
                }
            } else {
                p.credit = credit_of(&v["credit"]);
            }
            if num(&v, "busyUntil") > 0.0 {
                p.busy_until = num(&v, "busyUntil");
            }
            let hint = text(&v, "keyHint");
            if !hint.is_empty() {
                p.key_hint = hint;
            }
        });
    }

    fn test_key(&self, id: &str) {
        let fut = test_flow(self.me.clone(), id.to_string());
        let _ = slint::spawn_local(fut);
    }

    fn save_key(&self, id: &str, under: Option<String>) {
        let key = self
            .st
            .borrow()
            .prov
            .keys
            .get(id)
            .map(|k| k.trim().to_string())
            .unwrap_or_default();
        if key.is_empty() {
            self.set_ui(id, |u| u.error = "Paste a key first.".into());
            self.focus_field(id);
            self.render_providers();
            return;
        }
        let detected = detect_provider(&key);
        if under.is_none() && !detected.is_empty() && detected != id {
            self.set_ui(id, |u| {
                u.detected = detected.into();
                u.error.clear();
            });
            self.render_providers();
            let dn = name_of(detected);
            self.say(&format!("This is {} {dn} key.", article(&dn)));
            return;
        }
        let target = under.unwrap_or_else(|| id.to_string());
        if target != id {
            // The key moves to its own provider's row, which opens; this one
            // closes as it was.
            let other_has = self.prov(&target).is_some_and(|p| p.has_key);
            self.st
                .borrow_mut()
                .prov
                .keys
                .insert(target.clone(), key.clone());
            self.clear_field(id);
            self.set_ui(id, |u| {
                u.detected.clear();
                u.error.clear();
                u.mode.clear();
            });
            let mode = if other_has {
                "replace"
            } else if target == "openrouter" {
                "paste"
            } else {
                ""
            };
            self.set_ui(&target, |u| u.mode = mode.into());
            // Its field shows the key it is saving.
            {
                let mut st = self.st.borrow_mut();
                st.prov.next += 1;
                let n = st.prov.next;
                st.prov.epochs.insert(target.clone(), n);
            }
            self.open_row(&format!("prov-{target}"));
            self.save_key(&target, Some(target.clone()));
            return;
        }
        self.set_ui(&target, |u| {
            u.testing = true;
            u.error.clear();
            u.detected.clear();
        });
        self.render_providers();
        self.say(&format!("Checking the key with {}…", name_of(&target)));
        let me = self.me.clone();
        let fut = ask(
            self.label.clone(),
            "settings:save-key",
            json!({ "provider": target, "key": key }),
            15000,
        );
        let id = id.to_string();
        let _ = slint::spawn_local(async move {
            let r = fut.await;
            let need_test = {
                let Some(p) = me.upgrade() else {
                    return;
                };
                p.after_save(&id, &target, r)
            };
            let Some(need_test) = need_test else {
                return;
            };
            if need_test {
                test_flow(me.clone(), target.clone()).await;
            }
            if let Some(p) = me.upgrade() {
                p.refresh_from_rust();
            }
        });
    }

    /// The save's reply: None when it went no further, else whether the key
    /// still needs a test (an older build stores without testing).
    fn after_save(&self, id: &str, target: &str, r: Reply) -> Option<bool> {
        self.set_ui(target, |u| u.testing = false);
        let name = name_of(target);
        if matches!(r, Reply::TimedOut) {
            // Rust keeps a key it could not test; this page stops waiting.
            self.clear_field(id);
            self.set_ui(target, |u| u.mode.clear());
            self.with_prov(target, |p| {
                p.has_key = true;
                p.status = "unchecked".into();
            });
            self.render_providers();
            self.announce_status(target);
            return Some(false);
        }
        let Some(v) = r.value().cloned() else {
            self.set_ui(target, |u| {
                u.error = "Aevaleron couldn't save the key. Try again.".into();
            });
            self.focus_field(id);
            self.render_providers();
            self.say("Aevaleron couldn't save the key.");
            return None;
        };
        if !flag(&v, "ok") {
            let det = text(&v, "detected");
            if !det.is_empty() && det != target {
                let dn = name_of(&det);
                self.set_ui(id, |u| u.detected = det);
                self.render_providers();
                self.say(&format!("This is {} {dn} key.", article(&dn)));
                return None;
            }
            let err = text(&v, "error");
            let low = err.to_lowercase();
            let rejected = text(&v, "status") == "rejected"
                || ["reject", "invalid", "not valid"]
                    .iter()
                    .any(|w| low.contains(w));
            let shown = if rejected {
                format!("{name} rejected this key, so it wasn't saved. Check it and paste it again.")
            } else if err.is_empty() {
                "Aevaleron couldn't save the key. Try again.".into()
            } else {
                err.clone()
            };
            self.set_ui(target, |u| u.error = shown);
            self.focus_field(id);
            self.render_providers();
            let spoken = if rejected {
                format!("{name} rejected this key, so it wasn't saved.")
            } else if err.is_empty() {
                "Couldn't save the key.".into()
            } else {
                err
            };
            self.say(&spoken);
            return None;
        }
        self.clear_field(id);
        self.set_ui(target, |u| {
            u.mode.clear();
            u.error.clear();
            u.clip.clear();
        });
        let hint = text(&v, "keyHint");
        self.with_prov(target, |p| {
            p.has_key = true;
            if !hint.is_empty() {
                p.key_hint = hint;
            }
        });
        if text(&v, "status").is_empty() {
            return Some(true);
        }
        self.apply_test(target, &Reply::Got(v));
        self.render_providers();
        self.announce_status(target);
        Some(false)
    }

    /// A key Rust took from the clipboard (only a masked form ever reaches
    /// this page) is saved under its provider.
    fn use_clip_key(&self, id: &str) {
        self.set_ui(id, |u| u.testing = true);
        self.render_providers();
        let me = self.me.clone();
        let fut = ask(
            self.label.clone(),
            "settings:use-clip-key",
            json!({ "provider": id }),
            15000,
        );
        let id = id.to_string();
        let _ = slint::spawn_local(async move {
            let r = fut.await;
            let target = {
                let Some(p) = me.upgrade() else {
                    return;
                };
                p.set_ui(&id, |u| {
                    u.testing = false;
                    u.clip.clear();
                });
                let ok = r.value().is_some_and(|v| flag(v, "ok"));
                if !ok {
                    let err = r
                        .value()
                        .map(|v| text(v, "error"))
                        .filter(|e| !e.is_empty())
                        .unwrap_or_else(|| "The clipboard no longer holds a key.".into());
                    p.set_ui(&id, |u| u.error = err);
                    p.render_providers();
                    return;
                }
                p.took_key(&id, &r)
            };
            if let Some(t) = target {
                test_flow(me.clone(), t).await;
            }
            if let Some(p) = me.upgrade() {
                p.refresh_from_rust();
            }
        });
    }

    /// A key that went in by the clipboard or Connect: the row says so, or
    /// names the provider still to test (an older build).
    fn took_key(&self, id: &str, r: &Reply) -> Option<String> {
        let v = r.value().cloned().unwrap_or(Value::Null);
        let named = text(&v, "provider");
        let target = if info(&named).is_some() {
            named
        } else {
            id.to_string()
        };
        let hint = text(&v, "keyHint");
        self.with_prov(&target, |p| {
            p.has_key = true;
            if !hint.is_empty() {
                p.key_hint = hint;
            }
        });
        if text(&v, "status").is_empty() {
            return Some(target);
        }
        self.apply_test(&target, r);
        self.render_providers();
        self.announce_status(&target);
        None
    }

    fn connect_openrouter(&self) {
        self.set_ui("openrouter", |u| {
            u.connecting = true;
            u.error.clear();
            u.mode.clear();
        });
        self.render_providers();
        self.say("Your browser opened OpenRouter. Approve Aevaleron there.");
        let me = self.me.clone();
        let fut = ask(
            self.label.clone(),
            "auth:openrouter-connect",
            json!({}),
            6 * 60 * 1000,
        );
        let _ = slint::spawn_local(async move {
            let r = fut.await;
            let target = {
                let Some(p) = me.upgrade() else {
                    return;
                };
                p.set_ui("openrouter", |u| u.connecting = false);
                let ok = r.value().is_some_and(|v| flag(v, "ok"));
                if !ok {
                    let why = if matches!(r, Reply::TimedOut) {
                        "Didn't hear back from OpenRouter. Try again, or paste a key instead."
                            .to_string()
                    } else {
                        r.value()
                            .map(|v| text(v, "error"))
                            .filter(|e| !e.is_empty())
                            .unwrap_or_else(|| {
                                "Couldn't connect to OpenRouter. Try again, or paste a key instead."
                                    .into()
                            })
                    };
                    p.set_ui("openrouter", |u| u.error = why.clone());
                    p.render_providers();
                    p.say(&why);
                    return;
                }
                p.took_key("openrouter", &r)
            };
            if let Some(t) = target {
                test_flow(me.clone(), t).await;
            }
            if let Some(p) = me.upgrade() {
                p.refresh_from_rust();
            }
        });
    }

    fn remove_key(&self, id: &str) {
        let Some(p0) = self.prov(id) else {
            return;
        };
        self.set_ui(id, |u| u.removing = false);
        let id = id.to_string();
        self.call(
            "settings:clear-key",
            json!({ "provider": id }),
            0,
            move |p, r| {
                if r.failed() || said_no(r.value()) {
                    p.render_providers();
                    p.say(&format!("Couldn't remove the {} key.", p0.name));
                    return;
                }
                p.with_prov(&id, |q| {
                    q.has_key = false;
                    q.key_hint.clear();
                    q.status = "not-added".into();
                    q.credit = None;
                });
                p.set_ui(&id, |u| {
                    u.mode.clear();
                    u.error.clear();
                });
                p.render_providers();
                p.say(&format!("{} key removed.", p0.name));
                p.refresh_from_rust();
            },
        );
    }

    /// Esc with a remove question up: it goes. True when there was one.
    pub(super) fn cancel_remove(&self) -> bool {
        let removing = {
            let st = self.st.borrow();
            st.prov
                .ui
                .iter()
                .find(|(_, u)| u.removing)
                .map(|(id, _)| id.clone())
        };
        let Some(id) = removing else {
            return false;
        };
        self.set_ui(&id, |u| u.removing = false);
        self.render_providers();
        true
    }

    fn announce_move(&self, id: &str, to: usize) {
        let Some(p) = self.prov(id) else {
            return;
        };
        if to > 0 {
            let place = ORDINAL
                .get(to)
                .map_or_else(|| format!("number {}", to + 1), |o| (*o).to_string());
            self.say(&format!("{} is now {place}.", p.name));
            return;
        }
        let def = self
            .st
            .borrow()
            .prov
            .list
            .iter()
            .find(|q| answers(q))
            .cloned();
        match def {
            Some(d) if d.id == id => {
                self.say(&format!("{} is now first. It answers by default.", p.name));
            }
            Some(d) => self.say(&format!(
                "{} is now first. {} answers until {} is ready.",
                p.name, d.name, p.name
            )),
            None => self.say(&format!("{} is now first.", p.name)),
        }
    }

    fn move_provider(&self, id: &str, to: i32) {
        let (before, ids, to) = {
            let mut st = self.st.borrow_mut();
            let before = st.prov.list.clone();
            let Some(from) = before.iter().position(|p| p.id == id) else {
                return;
            };
            let to = to.clamp(0, before.len() as i32 - 1) as usize;
            if from == to {
                return;
            }
            let moved = st.prov.list.remove(from);
            st.prov.list.insert(to, moved);
            let ids: Vec<String> = st.prov.list.iter().map(|p| p.id.clone()).collect();
            (before, ids, to)
        };
        self.render_providers();
        self.announce_move(id, to);
        self.call(
            "settings:set-provider-order",
            json!({ "order": ids }),
            0,
            move |p, r| {
                if r.failed() || said_no(r.value()) {
                    p.st.borrow_mut().prov.list = before;
                    p.render_providers();
                    p.say("Couldn't change the order.");
                    return;
                }
                p.refresh_from_rust();
            },
        );
    }

    /// A row let go after a drag.
    pub(super) fn prov_dropped(&self, id: &str, to: i32) {
        let s = self.s();
        s.set_drag_id(SharedString::new());
        s.set_drag_to(-1);
        if self.st.borrow().prov.ordered {
            self.move_provider(id, to);
        }
    }

    /// Alt+Up and Alt+Down move the open provider.
    pub(super) fn alt_move(&self, up: bool) -> bool {
        let open = self.s().get_open_row().to_string();
        let Some(id) = open.strip_prefix("prov-") else {
            return false;
        };
        let at = {
            let st = self.st.borrow();
            if !st.prov.ordered {
                return false;
            }
            st.prov.list.iter().position(|p| p.id == id)
        };
        let Some(at) = at else {
            return false;
        };
        let to = if up { at as i32 - 1 } else { at as i32 + 1 };
        self.move_provider(id, to);
        true
    }

    /// A button in a provider row.
    pub(super) fn prov_act(&self, id: &str, what: &str, field: &str) {
        if matches!(what, "save" | "enter" | "save-under") {
            self.st
                .borrow_mut()
                .prov
                .keys
                .insert(id.to_string(), field.to_string());
        }
        let name = name_of(id);
        let at = self
            .st
            .borrow()
            .prov
            .list
            .iter()
            .position(|p| p.id == id)
            .map_or(0, |i| i as i32);
        match what {
            "save" => self.save_key(id, None),
            "enter" | "save-under" => {
                let d = self.pui(id).detected;
                self.save_key(id, Some(d).filter(|d| !d.is_empty()));
            }
            "clip" => self.use_clip_key(id),
            "connect" => self.connect_openrouter(),
            "paste" => {
                self.set_ui(id, |u| {
                    u.mode = "paste".into();
                    u.error.clear();
                });
                self.focus_field(id);
                self.render_providers();
            }
            "test" => self.test_key(id),
            "replace" => {
                self.set_ui(id, |u| {
                    u.mode = "replace".into();
                    u.error.clear();
                    u.detected.clear();
                });
                self.focus_field(id);
                self.render_providers();
            }
            "cancel" => {
                self.clear_field(id);
                self.set_ui(id, |u| {
                    u.mode.clear();
                    u.error.clear();
                    u.detected.clear();
                });
                self.render_providers();
            }
            "remove" => {
                self.set_ui(id, |u| u.removing = true);
                self.render_providers();
                self.say(&format!("Remove the {name} key from this PC?"));
            }
            "remove-no" => {
                self.set_ui(id, |u| u.removing = false);
                self.render_providers();
            }
            "remove-yes" => self.remove_key(id),
            "up" => self.move_provider(id, at - 1),
            "down" => self.move_provider(id, at + 1),
            "first" => self.move_provider(id, 0),
            _ => {}
        }
    }

    /// Typing in a key field: a new key, so the wrong-provider note and an
    /// old error no longer hold.
    pub(super) fn prov_edited(&self, id: &str, t: &str) {
        self.st
            .borrow_mut()
            .prov
            .keys
            .insert(id.to_string(), t.to_string());
        let u = self.pui(id);
        if !u.detected.is_empty() || !u.error.is_empty() {
            self.set_ui(id, |u| {
                u.detected.clear();
                u.error.clear();
            });
            self.render_providers();
        }
    }

    /// A key copied from a provider's site is offered when the window comes
    /// back. Rust reads the clipboard; only a masked form reaches the page.
    pub(super) fn check_clipboard_key(&self) {
        self.call("settings:clip-key", Value::Null, 0, |p, r| {
            {
                let mut st = p.st.borrow_mut();
                for u in st.prov.ui.values_mut() {
                    u.clip.clear();
                }
            }
            if let Some(v) = r.value().filter(|v| flag(v, "found")) {
                let named = text(v, "provider");
                let id = if info(&named).is_some() {
                    named
                } else {
                    "openrouter".to_string()
                };
                if p.prov(&id).is_some_and(|q| !q.has_key) {
                    let masked = text(v, "masked");
                    p.set_ui(&id, |u| u.clip = masked);
                    if p.s().get_open_row().is_empty() {
                        p.open_row(&format!("prov-{id}"));
                    }
                }
            }
            p.render_providers();
        });
    }

    pub(super) fn on_provider_status(&self, s: &Value) {
        let id = text(s, "id");
        let Some(before) = self.prov(&id) else {
            return;
        };
        let status = text(s, "status");
        let hint = text(s, "keyHint");
        self.with_prov(&id, |p| {
            if !status.is_empty() {
                p.status = if p.has_key || status != "not-added" {
                    status.clone()
                } else {
                    "not-added".into()
                };
            }
            if status == "ready" {
                let at = num(s, "checkedAt");
                p.checked_at = if at > 0.0 { at } else { now_ms() };
            }
            if !s["busyUntil"].is_null() {
                p.busy_until = num(s, "busyUntil");
            }
            if !s["credit"].is_null() {
                p.credit = credit_of(&s["credit"]);
            }
            if !hint.is_empty() {
                p.key_hint = hint;
            }
        });
        self.render_providers();
        let now = self.prov(&id).map(|p| p.status).unwrap_or_default();
        if now != before.status && !self.pui(&id).testing {
            self.announce_status(&id);
        }
    }

    /// Opens Providers at the first provider without a key (`sec-key` is
    /// the first-run tour pointing at the key field).
    pub(super) fn go_to_providers(&self, key_field: bool) {
        let (target, none) = {
            let st = self.st.borrow();
            let list = &st.prov.list;
            let t = list
                .iter()
                .find(|p| !p.has_key)
                .or_else(|| list.first())
                .map(|p| p.id.clone());
            (t, !list.iter().any(|p| p.has_key))
        };
        self.ui.invoke_jump("pg-providers".into());
        if let Some(t) = target.filter(|_| key_field || none) {
            if key_field && t == "openrouter" {
                self.set_ui(&t, |u| u.mode = "paste".into());
                self.focus_field(&t);
            }
            self.open_row(&format!("prov-{t}"));
            self.render_providers();
        }
    }

    pub(super) fn go_to_provider(&self, pid: &str) {
        let Some(p) = self.prov(pid) else {
            return;
        };
        if p.status == "rejected" {
            self.set_ui(pid, |u| u.mode = "replace".into());
            self.focus_field(pid);
        }
        self.open_row(&format!("prov-{pid}"));
        self.ui.invoke_jump("pg-providers".into());
        self.render_providers();
    }

    // -------------------------------------------------------- failover

    fn render_failover(&self) {
        let f = self.st.borrow().data["failover"].clone();
        let s = self.s();
        s.set_failover_shown(f.is_object());
        if f.is_object() {
            s.set_failover_across(f["acrossProviders"] != json!(false));
            s.set_failover_ask(f["askBeforePaid"] != json!(false));
        }
    }

    pub(super) fn set_failover(&self, patch: Value) {
        let before = self.st.borrow().data["failover"].clone();
        let mut next = json!({ "acrossProviders": true, "askBeforePaid": true });
        for src in [&before, &patch] {
            if let Some(m) = src.as_object() {
                for (k, v) in m {
                    next[k] = v.clone();
                }
            }
        }
        self.st.borrow_mut().data["failover"] = next.clone();
        self.render_failover();
        self.call("settings:set-failover", next, 0, move |p, r| {
            if r.failed() || said_no(r.value()) {
                p.st.borrow_mut().data["failover"] = before;
                p.render_failover();
                p.say("Couldn't change that setting.");
                return;
            }
            if let Some(f) = r.value().map(|v| v["failover"].clone()).filter(Value::is_object) {
                p.st.borrow_mut().data["failover"] = f;
            }
            p.render_failover();
        });
    }

    pub(super) fn render_key_storage(&self) {
        let ks = text(&self.st.borrow().data, "keyStorage");
        let t = match ks.as_str() {
            "encrypted" => "Keys are encrypted with your Windows account and only sent to their own provider.",
            "plain" => "Windows couldn't provide a key store, so keys are saved unencrypted in your Aevaleron settings file. Anyone signed in to this Windows account can read them.",
            _ => "",
        };
        let s = self.s();
        s.set_key_storage(t.into());
        s.set_key_storage_warn(ks == "plain");
    }

    /// Who receives the screen, from the ready providers in the order they
    /// are tried (Rust's recipients, else this page's own list).
    fn render_recipients(&self) {
        let names: Vec<String> = {
            let st = self.st.borrow();
            match st.data["recipients"].as_array() {
                Some(rec) => rec
                    .iter()
                    .map(|r| {
                        let id = text(r, "id");
                        info(&id).map_or_else(|| text(r, "name"), |i| i.name.to_string())
                    })
                    .filter(|n| !n.is_empty())
                    .collect(),
                None => st
                    .prov
                    .list
                    .iter()
                    .filter(|p| answers(p))
                    .map(|p| p.name.clone())
                    .collect(),
            }
        };
        let first = match names.len() {
            0 => "Nothing is sent until a provider is ready. Then asking sends your question and a screenshot, or the window's text, to it.".to_string(),
            1 => format!("Asking sends your question and a screenshot, or the window's text, to {}.", names[0]),
            _ => format!("Asking sends your question and a screenshot, or the window's text, to the first ready provider: {}.", names.join(", then ")),
        };
        self.s().set_recipients(format!("{first} Tasks send one each step. Free models are offered on the condition that their providers may keep and train on what they receive. Paid models don't.").into());
    }

    // ---------------------------------------------------------- models

    pub(super) fn render_models_mode(&self) {
        let m = self.st.borrow().prov.mode.clone();
        self.s().set_models_mode(m.into());
    }

    pub(super) fn set_models_mode(&self, m: &str) {
        if self.st.borrow().prov.mode == m {
            return;
        }
        self.st.borrow_mut().prov.mode = m.to_string();
        let keep = m.to_string();
        let _ = crate::shell::async_runtime::spawn_blocking(move || {
            crate::store::get().set_raw(MODE_KEY, json!(keep));
        });
        self.render_models_mode();
        if m == "auto" {
            self.use_recommended_models();
        }
    }

    /// One segment per provider with a key; the default provider first
    /// chosen.
    fn render_model_providers(&self) {
        let (choices, native) = {
            let mut st = self.st.borrow_mut();
            let pv = &mut st.prov;
            let keyed: Vec<Prov> = pv.list.iter().filter(|p| p.has_key).cloned().collect();
            if !keyed.iter().any(|p| p.id == pv.provider) {
                let def = pv.list.iter().find(|p| answers(p)).map(|p| p.id.clone());
                pv.provider = def
                    .or_else(|| keyed.first().map(|p| p.id.clone()))
                    .unwrap_or_else(|| "openrouter".into());
            }
            let choices: Vec<SetChoice> = if pv.ordered && keyed.len() > 1 {
                keyed
                    .iter()
                    .map(|p| SetChoice {
                        id: p.id.as_str().into(),
                        name: p.name.as_str().into(),
                        note: SharedString::new(),
                        checked: p.id == pv.provider,
                        enabled: true,
                    })
                    .collect()
            } else {
                Vec::new()
            };
            (choices, pv.provider != "openrouter")
        };
        let s = self.s();
        s.set_models_for(ModelRc::new(VecModel::from(choices)));
        s.set_paid_shown(!native);
    }

    pub(super) fn set_models_provider(&self, id: &str) {
        if self.st.borrow().prov.provider == id {
            return;
        }
        self.st.borrow_mut().prov.provider = id.to_string();
        self.render_model_providers();
        self.reload_pickers();
    }

    /// Automatic: every job back to the model Aevaleron recommends.
    fn use_recommended_models(&self) {
        let (d, others) = {
            let st = self.st.borrow();
            let others: Vec<String> = if st.prov.ordered {
                st.prov
                    .list
                    .iter()
                    .filter(|q| q.has_key && q.id != "openrouter")
                    .map(|q| q.id.clone())
                    .collect()
            } else {
                Vec::new()
            };
            (st.data.clone(), others)
        };
        let mut saves: Vec<(&'static str, Value)> = Vec::new();
        let dm = text(&d, "defaultModel");
        let da = text(&d, "defaultAgentModel");
        if !dm.is_empty() {
            saves.push(("settings:save-model", json!({ "model": dm })));
        }
        if !da.is_empty() {
            saves.push(("settings:save-agent-model", json!({ "model": da })));
        }
        if flag(&d, "brainEnabled") {
            saves.push(("settings:set-brain-enabled", json!({ "on": false })));
        }
        for id in &others {
            for ch in [
                "settings:save-model",
                "settings:save-brain",
                "settings:save-agent-model",
            ] {
                saves.push((ch, json!({ "provider": id, "model": "" })));
            }
        }
        let label = self.label.clone();
        let me = self.me.clone();
        let _ = slint::spawn_local(async move {
            for (ch, payload) in saves {
                let _ = ask(label.clone(), ch, payload, 0).await;
            }
            let Some(p) = me.upgrade() else {
                return;
            };
            {
                let mut st = p.st.borrow_mut();
                if !dm.is_empty() {
                    st.data["model"] = json!(dm);
                }
                if !da.is_empty() {
                    st.data["agentModel"] = json!(da);
                }
                st.data["brainEnabled"] = json!(false);
            }
            p.s().set_brain_enabled(false);
            p.sync_pickers();
            p.reload_pickers();
        });
    }

    fn picker_kind(cid: &str) -> (&'static str, &'static str, &'static str) {
        match cid {
            "brain" => ("brain", "text", "settings:save-brain"),
            "agent" => ("agent", "agent", "settings:save-agent-model"),
            _ => ("model", "vision", "settings:save-model"),
        }
    }

    pub(super) fn reload_pickers(&self) {
        let (provider, data) = {
            let st = self.st.borrow();
            (st.prov.provider.clone(), st.data.clone())
        };
        if provider == "openrouter" {
            let mut st = self.st.borrow_mut();
            for (cid, key) in [("model", "model"), ("brain", "brain"), ("agent", "agentModel")] {
                st.prov.pickers.entry(cid).or_default().current = text(&data, key);
            }
        }
        self.sync_pickers();
        for cid in ["model", "brain", "agent"] {
            let (cid, kind, _) = Self::picker_kind(cid);
            let mut payload = json!({ "kind": kind });
            if provider != "openrouter" {
                payload["provider"] = json!(provider);
            }
            let from = provider.clone();
            self.call("models:list", payload, 0, move |p, r| {
                let Some(res) = r.value() else {
                    return;
                };
                let groups: Vec<(String, Vec<ModelInfo>)> = res["groups"]
                    .as_array()
                    .map(|gs| {
                        gs.iter()
                            .map(|g| {
                                let models = g["models"]
                                    .as_array()
                                    .map(|ms| {
                                        ms.iter()
                                            .map(|m| ModelInfo {
                                                id: text(m, "id"),
                                                name: text(m, "name"),
                                                tag: text(m, "tag"),
                                                note: text(m, "note"),
                                                detail: text(m, "detail"),
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                (text(g, "label"), models)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let current = text(res, "current");
                let where_ = if from == "openrouter" {
                    "OpenRouter".to_string()
                } else {
                    name_of(&from)
                };
                let offline = if res["live"] == json!(false) {
                    format!("Could not reach {where_}, so this is the built-in list.")
                } else {
                    String::new()
                };
                {
                    let mut st = p.st.borrow_mut();
                    let pk = st.prov.pickers.entry(cid).or_default();
                    pk.groups = groups;
                    if !current.is_empty() {
                        pk.current = current;
                    }
                    pk.offline = offline;
                    pk.loaded = true;
                }
                p.sync_pickers();
            });
        }
    }

    /// The pickers' faces, lists and the rows' summaries.
    pub(super) fn sync_pickers(&self) {
        let brain_on = flag(&self.st.borrow().data, "brainEnabled");
        let s = self.s();
        let mut brain_label = String::new();
        for cid in ["model", "brain", "agent"] {
            let view = {
                let st = self.st.borrow();
                let pk = st.prov.pickers.get(cid);
                let empty = Picker::default();
                let pk = pk.unwrap_or(&empty);
                let known = pk.known();
                let label = match known {
                    Some(m) => m.name.clone(),
                    None if !pk.current.is_empty() => pk.current.clone(),
                    None if !pk.loaded => "Loading…".into(),
                    None => "Choose…".into(),
                };
                // The closed control shows price and context only; the
                // description goes underneath, where it has room to wrap.
                let detail = match known {
                    Some(m) => [m.tag.as_str(), m.detail.as_str()]
                        .iter()
                        .filter(|x| !x.is_empty())
                        .copied()
                        .collect::<Vec<_>>()
                        .join(" · "),
                    None if !pk.current.is_empty() => "Custom model id".into(),
                    None => String::new(),
                };
                let note = [known.map_or("", |m| m.note.as_str()), pk.offline.as_str()]
                    .iter()
                    .filter(|x| !x.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(" ");
                let summary = match known {
                    Some(m) => m.name.clone(),
                    None if !pk.current.is_empty() => pk.current.clone(),
                    None if !pk.loaded => "Loading…".into(),
                    None => "Not set".into(),
                };
                let mut rows = Vec::new();
                for (g, models) in &pk.groups {
                    rows.push(SetComboRow {
                        header: true,
                        id: SharedString::new(),
                        name: g.as_str().into(),
                        detail: SharedString::new(),
                        checked: false,
                    });
                    for m in models {
                        let d = [
                            if m.tag.is_empty() {
                                String::new()
                            } else {
                                format!("{}.", m.tag)
                            },
                            m.note.clone(),
                            m.detail.clone(),
                        ]
                        .iter()
                        .filter(|x| !x.is_empty())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" ");
                        rows.push(SetComboRow {
                            header: false,
                            id: m.id.as_str().into(),
                            name: m.name.as_str().into(),
                            detail: d.into(),
                            checked: m.id == pk.current,
                        });
                    }
                }
                (
                    SetComboView {
                        label: label.as_str().into(),
                        detail: detail.into(),
                        note: note.into(),
                        rows: ModelRc::new(VecModel::from(rows)),
                    },
                    summary,
                    label,
                )
            };
            let (v, summary, label) = view;
            match cid {
                "model" => {
                    s.set_model_combo(v);
                    s.set_model_summary(summary.into());
                }
                "brain" => {
                    s.set_brain_combo(v);
                    brain_label = label;
                }
                _ => {
                    s.set_agent_combo(v);
                    s.set_agent_model_summary(summary.into());
                }
            }
        }
        let brain = if !brain_on {
            "Off".to_string()
        } else if brain_label.is_empty() || brain_label == "Loading…" {
            "On".to_string()
        } else {
            brain_label
        };
        s.set_brain_summary(brain.into());
        s.set_brain_enabled(brain_on);
    }

    /// The rows of a model picker, for its keys.
    pub(super) fn model_rows(&self, cid: &str) -> Vec<(bool, String, String)> {
        let st = self.st.borrow();
        let mut out = Vec::new();
        if let Some(pk) = st.prov.pickers.get(cid) {
            for (g, models) in &pk.groups {
                out.push((true, String::new(), g.clone()));
                for m in models {
                    out.push((false, m.id.clone(), m.name.clone()));
                }
            }
        }
        out
    }

    /// The option a picker has chosen.
    pub(super) fn combo_checked(&self, cid: &str) -> String {
        match cid {
            "model" | "brain" | "agent" => self
                .st
                .borrow()
                .prov
                .pickers
                .get(cid)
                .map(|p| p.current.clone())
                .unwrap_or_default(),
            _ => self.voice_checked(cid),
        }
    }

    pub(super) fn combo_choose(&self, cid: &str, id: &str) {
        let (cid, _, channel) = Self::picker_kind(cid);
        let provider = {
            let mut st = self.st.borrow_mut();
            st.prov.pickers.entry(cid).or_default().current = id.to_string();
            let key = match cid {
                "brain" => "brain",
                "agent" => "agentModel",
                _ => "model",
            };
            if st.prov.provider == "openrouter" {
                st.data[key] = json!(id);
            }
            st.prov.provider.clone()
        };
        let mut payload = json!({ "model": id });
        if provider != "openrouter" {
            payload["provider"] = json!(provider);
        }
        self.call(channel, payload, 0, |p, _| p.sync_pickers());
        self.sync_pickers();
        self.close_combos();
    }

    pub(super) fn set_show_paid(&self, want: bool) {
        self.s().set_show_paid(want);
        self.call(
            "settings:set-show-paid",
            json!({ "on": want }),
            0,
            move |p, r| {
                if r.failed() {
                    p.s().set_show_paid(!want);
                    p.unsaved(None);
                    return;
                }
                p.st.borrow_mut().data["showPaid"] = json!(want);
                p.reload_pickers();
            },
        );
    }

    pub(super) fn set_brain_enabled(&self, want: bool) {
        self.s().set_brain_enabled(want);
        self.call(
            "settings:set-brain-enabled",
            json!({ "on": want }),
            0,
            move |p, r| {
                if r.failed() {
                    let was = flag(&p.st.borrow().data, "brainEnabled");
                    p.s().set_brain_enabled(was);
                    p.unsaved(None);
                    return;
                }
                let on = r.value().map_or(want, |v| flag(v, "brainEnabled"));
                p.st.borrow_mut().data["brainEnabled"] = json!(on);
                p.sync_pickers();
            },
        );
    }
}

/// Tests a provider's key with Rust and shows what it said.
async fn test_flow(me: std::rc::Weak<SettingsPage>, id: String) {
    let fut = {
        let Some(p) = me.upgrade() else {
            return;
        };
        p.set_ui(&id, |u| {
            u.testing = true;
            u.error.clear();
        });
        p.render_providers();
        ask(
            p.label.clone(),
            "settings:test-key",
            json!({ "provider": id }),
            15000,
        )
    };
    let r = fut.await;
    let Some(p) = me.upgrade() else {
        return;
    };
    p.set_ui(&id, |u| u.testing = false);
    p.apply_test(&id, &r);
    p.render_providers();
    p.announce_status(&id);
}
