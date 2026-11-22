//! Settings' Voice page (voice-settings.js), the coding agents
//! (agents-settings.js) and the private apps list
//! (private-apps-settings.js). Each saves at once and answers with its
//! whole state, so what is shown is what is stored.

use std::time::Duration;

use serde_json::{json, Value};
use slint::{ModelRc, SharedString, TimerMode, VecModel};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

use super::{flag, num, text, SettingsPage};
use crate::shell::platform::KeyPress;
use crate::ui::{SetAppRow, SetChoice, SetComboRow, SetComboView, SetToggle};

/// The engines, in the page's order, with what each is.
const ENGINES: [(&str, &str, &str); 6] = [
    ("whisper-base", "Whisper base", "On this PC. The default: every language, with fuller punctuation."),
    ("parakeet-110m", "Parakeet 110M", "On this PC. Fastest. English, with plainer punctuation."),
    ("whisper-tiny", "Whisper tiny", "On this PC. Small download, gets more words wrong."),
    ("whisper-small", "Whisper small", "On this PC. More accurate than base, slower on older PCs."),
    ("parakeet-0.6b", "Parakeet 0.6B", "On this PC. Best text. Large download, slower without a strong processor."),
    ("gemini", "Gemini", "Uses your Gemini key. The recording is sent to Google."),
];

/// A chord's modifiers, in the order it names them, with the keys that
/// hold each.
const MODS: [(&str, &[u16]); 4] = [
    ("Ctrl", &[0x11, 0xA2, 0xA3]),
    ("Alt", &[0x12, 0xA4, 0xA5]),
    ("Shift", &[0x10, 0xA0, 0xA1]),
    ("Super", &[0x5B, 0x5C]),
];

/// The cue the "Couldn't save" lines use on the agents page.
const NOT_SAVED: &str = "Couldn't save that change.";

fn label(acc: &str) -> String {
    acc.replace("Super", "Win")
}

fn mb(bytes: f64) -> String {
    format!("{} MB", (bytes / 1e6).round() as i64)
}

fn engine_name(id: &str) -> String {
    ENGINES
        .iter()
        .find(|e| e.0 == id)
        .map_or_else(|| id.to_string(), |e| e.1.to_string())
}

/// `{name: on}`, a patch of one switch.
fn one(name: &str, on: bool) -> Value {
    let mut m = serde_json::Map::new();
    m.insert(name.to_string(), json!(on));
    Value::Object(m)
}

fn down(vk: u16) -> bool {
    // SAFETY: a plain query of the keyboard.
    (unsafe { GetAsyncKeyState(i32::from(vk)) } as u16 & 0x8000) != 0
}

/// A hold-to-talk chord being recorded. The key hook only hears presses,
/// so letting go is read from the keyboard (`chord_timer`).
#[derive(Default)]
pub(super) struct Chord {
    mods: Vec<&'static str>,
    key: Option<(String, u16)>,
    best_mods: Vec<&'static str>,
    best_key: Option<String>,
}

fn chord_name(mods: &[&'static str], key: Option<&str>) -> String {
    let mut parts: Vec<&str> = MODS
        .iter()
        .map(|m| m.0)
        .filter(|m| mods.contains(m))
        .collect();
    if let Some(k) = key {
        parts.push(k);
    }
    parts.join("+")
}

/// A key a voice chord can hold besides its modifiers.
fn chord_key_name(vk: u16) -> Option<String> {
    match vk {
        0x41..=0x5A | 0x30..=0x39 => char::from_u32(u32::from(vk)).map(String::from),
        0x70..=0x87 => Some(format!("F{}", vk - 0x6F)),
        0x20 => Some("Space".into()),
        _ => None,
    }
}

#[derive(Default)]
pub(super) struct Voice {
    pub(super) chord: Option<Chord>,
    /// The last `voice:get`.
    snap: Option<Value>,
    downloading: bool,
    /// The last `agents:get`.
    watch: Option<Value>,
    /// The last `privacy:get`, and the changes sent and not yet answered:
    /// a reload that started before a change, or lands during one, is out
    /// of date and is dropped.
    privacy: Option<Value>,
    changes: u32,
    pending: u32,
}

impl SettingsPage {
    // ----------------------------------------------------------- voice

    pub(super) fn voice_load(&self) {
        self.call("voice:get", Value::Null, 0, |p, r| {
            let Some(v) = r.value().cloned() else {
                return;
            };
            p.st.borrow_mut().voice.snap = Some(v);
            p.voice_render();
        });
    }

    fn engine_status(&self, t: &str, error: bool) {
        let s = self.s();
        s.set_engine_status(t.into());
        s.set_engine_error(error);
    }

    fn voice_render(&self) {
        let (snap, capturing, downloading) = {
            let st = self.st.borrow();
            let Some(snap) = st.voice.snap.clone() else {
                return;
            };
            (snap, st.voice.chord.is_some(), st.voice.downloading)
        };
        let set = &snap["settings"];
        let s = self.s();
        let dictation = flag(set, "dictation");
        let hk = text(&snap, "hotkeyLabel");
        s.set_dictation(dictation);
        if !capturing {
            s.set_hotkey_label(hk.as_str().into());
        }
        s.set_hotkey_capturing(capturing);
        s.set_dictation_summary(if dictation { hk } else { "Off".into() }.into());

        let engine = text(set, "engine");
        let engines = snap["engines"].as_array().cloned().unwrap_or_default();
        let of = |id: &str| engines.iter().find(|e| text(e, "id") == id).cloned();
        let choices: Vec<SetChoice> = ENGINES
            .iter()
            .map(|(id, name, base)| {
                let e = of(id);
                let mut extra = String::new();
                if let Some(e) = &e {
                    if flag(e, "local") {
                        extra = if flag(e, "ready") {
                            " Downloaded.".into()
                        } else {
                            format!(" Downloads {} the first time.", mb(num(e, "bytes")))
                        };
                    } else if !flag(e, "available") {
                        extra = " Add a Gemini key under Providers first.".into();
                    }
                    // A Parakeet engine asked for a language it does not
                    // hear.
                    if flag(e, "standIn") {
                        extra = format!(" For the language chosen, Whisper base is used.{extra}");
                    }
                }
                SetChoice {
                    id: (*id).into(),
                    name: (*name).into(),
                    note: format!("{base}{extra}").into(),
                    checked: *id == engine,
                    enabled: e.as_ref().is_none_or(|e| flag(e, "available")),
                }
            })
            .collect();
        s.set_engines(ModelRc::new(VecModel::from(choices)));
        s.set_engine_summary(engine_name(&engine).into());
        let chosen = of(&engine);
        let local = chosen.as_ref().is_some_and(|e| flag(e, "local"));
        let ready = chosen.as_ref().is_some_and(|e| flag(e, "ready"));
        let busy = downloading || (!snap["download"].is_null() && snap["download"] != json!(false));
        s.set_download_shown(local && !ready && !busy);
        if snap["cpuOk"] == json!(false) && local {
            self.engine_status(
                "This PC lacks the processor features voice on this PC needs (AVX2). Choose Gemini instead.",
                true,
            );
        }

        // Language.
        let lang = text(set, "language");
        let langs = snap["languages"].as_array().cloned().unwrap_or_default();
        let lang_name = langs
            .iter()
            .find(|l| text(l, "id") == lang)
            .map_or_else(|| lang.clone(), |l| text(l, "name"));
        let rows: Vec<SetComboRow> = langs
            .iter()
            .map(|l| SetComboRow {
                header: false,
                id: text(l, "id").into(),
                name: text(l, "name").into(),
                detail: SharedString::new(),
                checked: text(l, "id") == lang,
            })
            .collect();
        s.set_lang_combo(SetComboView {
            label: lang_name.as_str().into(),
            detail: SharedString::new(),
            note: SharedString::new(),
            rows: ModelRc::new(VecModel::from(rows)),
        });
        s.set_language_summary(lang_name.into());

        // Microphone.
        let device = text(set, "device");
        let devices: Vec<String> = snap["devices"]
            .as_array()
            .map(|d| d.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        let default = match devices.first() {
            Some(d) => format!("Windows default ({d})"),
            None => "Windows default".to_string(),
        };
        let mut rows = vec![SetComboRow {
            header: false,
            id: SharedString::new(),
            name: default.as_str().into(),
            detail: SharedString::new(),
            checked: device.is_empty(),
        }];
        rows.extend(devices.iter().map(|d| SetComboRow {
            header: false,
            id: d.as_str().into(),
            name: d.as_str().into(),
            detail: SharedString::new(),
            checked: *d == device,
        }));
        s.set_mic_combo(SetComboView {
            label: if device.is_empty() {
                default.into()
            } else {
                device.as_str().into()
            },
            detail: SharedString::new(),
            note: SharedString::new(),
            rows: ModelRc::new(VecModel::from(rows)),
        });
        s.set_mic_enabled(!devices.is_empty());
        s.set_mic_summary(if !device.is_empty() {
            device.into()
        } else if devices.is_empty() {
            "No microphone found".into()
        } else {
            "Windows default".into()
        });

        // Cleanup.
        let cleanup = flag(set, "cleanup");
        s.set_cleanup(cleanup);
        s.set_cleanup_summary(if cleanup { "On" } else { "Off" }.into());
        let budget_ms = snap["cleanupBudgetMs"].as_f64().filter(|b| *b > 0.0).unwrap_or(800.0);
        let budget = format!("{:.1} s", budget_ms / 1000.0);
        let cost = if cleanup {
            format!("Each dictation waits up to {budget} longer for it.")
        } else {
            format!("Off, your words go in the moment they are heard, with \"um\" and \"uh\" taken out. On, each dictation waits up to {budget} longer.")
        };
        let model = text(&snap, "cleanupModel");
        s.set_cleanup_note(if model.is_empty() {
            format!("{cost} Needs a free OpenRouter model, set up under Providers. Until then you get the words as heard.")
        } else {
            format!("{cost} Uses {model}, a free model, and never adds words. If it is slower than that, you get the words as heard.")
        }
        .into());
        self.wake_render(&snap);
        let t = &snap["lastTimings"];
        s.set_last_timing(if t.is_object() {
            format!(
                "Your last dictation: text in {:.1} s after you let go.",
                num(t, "totalMs") / 1000.0
            )
            .into()
        } else {
            SharedString::new()
        });
    }

    /// Listening for the name: the switch, the name, and how it stands.
    fn wake_render(&self, snap: &Value) {
        let set = &snap["settings"];
        let word = Some(text(set, "wakeWord"))
            .filter(|w| !w.is_empty())
            .unwrap_or_else(|| "Aevaleron".into());
        let w = &snap["wake"];
        let state = text(w, "state");
        let msg = text(w, "message");
        let on = flag(set, "wake");
        let s = self.s();
        s.set_wake(on);
        s.set_wake_label(format!("Start when I say {word}").into());
        s.set_wake_word(word.as_str().into());
        s.set_wake_epoch(s.get_wake_epoch().wrapping_add(1));
        let (line, error) = if !on {
            (String::new(), false)
        } else {
            match state.as_str() {
                "listening" => (format!("Listening for \"{word}\"."), false),
                "waiting" if !msg.is_empty() => (msg, false),
                "error" if !msg.is_empty() => (msg, true),
                "error" => ("It cannot listen right now.".to_string(), true),
                _ => ("Starting…".to_string(), false),
            }
        };
        s.set_wake_summary(if on { word.as_str() } else { "Off" }.into());
        s.set_wake_status(line.into());
        s.set_wake_error(error);
        // Asks again while listening is waiting to start.
        self.wake_timer.stop();
        if on && state != "listening" {
            let me = self.me.clone();
            self.wake_timer
                .start(TimerMode::SingleShot, Duration::from_secs(3), move || {
                    if let Some(p) = me.upgrade() {
                        if p.st.borrow().voice.chord.is_none() {
                            p.voice_load();
                        }
                    }
                });
        }
    }

    /// Saves a change through `voice:set`, which checks it; a refusal (a
    /// chord another shortcut has) is said in place. `done` is said once
    /// it saved.
    fn voice_set(&self, patch: Value, done: Option<String>) {
        self.call("voice:set", patch, 0, move |p, r| {
            let Some(v) = r.value() else {
                p.voice_render();
                return;
            };
            if !flag(v, "ok") {
                let err = text(v, "error");
                p.s().set_hotkey_warn(err.as_str().into());
                p.announce(&err, "assertive");
                p.voice_render();
                return;
            }
            p.s().set_hotkey_warn(SharedString::new());
            if let Some(t) = &done {
                p.announce(t, "polite");
            }
            p.voice_load();
        });
    }

    pub(super) fn voice_save(&self, patch: Value) {
        self.voice_set(patch, None);
    }

    /// A switch on the page: shown at once, put back if it did not save.
    pub(super) fn voice_toggle(&self, what: &str, on: bool) {
        let s = self.s();
        match what {
            "dictation" => s.set_dictation(on),
            "wake" => s.set_wake(on),
            _ => s.set_cleanup(on),
        }
        self.voice_set(one(what, on), None);
    }

    pub(super) fn voice_engine(&self, id: &str) {
        let same = self
            .st
            .borrow()
            .voice
            .snap
            .as_ref()
            .is_some_and(|v| text(&v["settings"], "engine") == id);
        if !same {
            self.voice_save(json!({ "engine": id }));
        }
    }

    pub(super) fn voice_download(&self) {
        self.st.borrow_mut().voice.downloading = true;
        let s = self.s();
        s.set_download_shown(false);
        s.set_progress_shown(true);
        s.set_progress(0.0);
        self.engine_status("Downloading…", false);
        self.call("voice:download", Value::Null, 0, |p, r| {
            p.st.borrow_mut().voice.downloading = false;
            p.s().set_progress_shown(false);
            let Some(v) = r.value() else {
                p.voice_render();
                return;
            };
            if flag(v, "ok") {
                p.engine_status("Ready. Voice works offline now.", false);
                p.announce("Voice model downloaded.", "polite");
            } else {
                let err = text(v, "error");
                p.engine_status(&err, true);
                p.announce(&err, "assertive");
            }
            p.voice_load();
        });
    }

    pub(super) fn on_voice_progress(&self, p: &Value) {
        let total = num(p, "total");
        if total <= 0.0 {
            return;
        }
        let got = num(p, "got");
        let s = self.s();
        s.set_progress_shown(true);
        s.set_progress((got / total) as f32);
        self.engine_status(
            &format!("Downloading… {}%", (got * 100.0 / total).round() as i64),
            false,
        );
    }

    pub(super) fn save_wake_word(&self, typed: &str) {
        let word = typed.trim().to_string();
        let same = self
            .st
            .borrow()
            .voice
            .snap
            .as_ref()
            .is_none_or(|v| text(&v["settings"], "wakeWord") == word);
        if word.is_empty() || same {
            return;
        }
        self.call("voice:set", json!({ "wakeWord": word }), 0, move |p, r| {
            let Some(v) = r.value() else {
                return;
            };
            if !flag(v, "ok") {
                let err = text(v, "error");
                let s = p.s();
                s.set_wake_status(err.as_str().into());
                s.set_wake_error(true);
                p.announce(&err, "assertive");
                return;
            }
            p.announce(&format!("Listening for {word}."), "polite");
            p.voice_load();
        });
    }

    /// The picker rows for language and microphone.
    pub(super) fn voice_rows(&self, cid: &str) -> Vec<(bool, String, String)> {
        let st = self.st.borrow();
        let Some(snap) = &st.voice.snap else {
            return Vec::new();
        };
        match cid {
            "language" => snap["languages"]
                .as_array()
                .map(|l| {
                    l.iter()
                        .map(|l| (false, text(l, "id"), text(l, "name")))
                        .collect()
                })
                .unwrap_or_default(),
            "mic" => {
                let mut out = vec![(false, String::new(), "Windows default".to_string())];
                if let Some(d) = snap["devices"].as_array() {
                    out.extend(
                        d.iter()
                            .filter_map(Value::as_str)
                            .map(|d| (false, d.to_string(), d.to_string())),
                    );
                }
                out
            }
            _ => Vec::new(),
        }
    }

    pub(super) fn voice_checked(&self, cid: &str) -> String {
        let st = self.st.borrow();
        let Some(snap) = &st.voice.snap else {
            return String::new();
        };
        match cid {
            "language" => text(&snap["settings"], "language"),
            "mic" => text(&snap["settings"], "device"),
            _ => String::new(),
        }
    }

    // ---------------------------------------------------- chord capture

    pub(super) fn voice_hotkey_button(&self) {
        if self.st.borrow().voice.chord.is_some() {
            self.end_chord();
            return;
        }
        let on = self
            .st
            .borrow()
            .voice
            .snap
            .as_ref()
            .is_some_and(|v| flag(&v["settings"], "dictation"));
        if !on {
            return;
        }
        self.st.borrow_mut().voice.chord = Some(Chord::default());
        let s = self.s();
        s.set_hotkey_warn(SharedString::new());
        s.set_hotkey_label("Press keys…".into());
        self.send("voice:capture-hotkey", json!({ "on": true }));
        self.voice_render();
        self.announce("Hold the new keys, then let go. Esc cancels.", "polite");
        let me = self.me.clone();
        self.chord_timer
            .start(TimerMode::Repeated, Duration::from_millis(30), move || {
                if let Some(p) = me.upgrade() {
                    p.chord_poll();
                }
            });
    }

    fn end_chord(&self) {
        self.chord_timer.stop();
        self.st.borrow_mut().voice.chord = None;
        self.send("voice:capture-hotkey", json!({ "on": false }));
        self.voice_render();
    }

    fn show_chord(&self) {
        let name = {
            let st = self.st.borrow();
            let Some(c) = &st.voice.chord else {
                return;
            };
            chord_name(&c.mods, c.key.as_ref().map(|k| k.0.as_str()))
        };
        let shown = if name.is_empty() {
            "Press keys…".to_string()
        } else {
            label(&name)
        };
        self.s().set_hotkey_label(shown.into());
    }

    /// A key pressed while a chord is recorded.
    pub(super) fn chord_key(&self, k: &KeyPress) {
        let empty = self
            .st
            .borrow()
            .voice
            .chord
            .as_ref()
            .is_none_or(|c| c.mods.is_empty() && c.key.is_none());
        if k.vk == 0x1B && empty {
            self.end_chord();
            self.announce("Kept the old shortcut.", "polite");
            return;
        }
        {
            let mut st = self.st.borrow_mut();
            let Some(c) = st.voice.chord.as_mut() else {
                return;
            };
            for (on, name) in [
                (k.ctrl, "Ctrl"),
                (k.alt, "Alt"),
                (k.shift, "Shift"),
                (k.meta, "Super"),
            ] {
                if on && !c.mods.contains(&name) {
                    c.mods.push(name);
                }
            }
            if let Some((name, _)) = MODS.iter().find(|m| m.1.contains(&k.vk)) {
                if !c.mods.contains(name) {
                    c.mods.push(*name);
                }
            } else if let Some(key) = chord_key_name(k.vk) {
                c.key = Some((key, k.vk));
            }
            let now = c.mods.len() + usize::from(c.key.is_some());
            let best = c.best_mods.len() + usize::from(c.best_key.is_some());
            if now > best {
                c.best_mods.clone_from(&c.mods);
                c.best_key = c.key.as_ref().map(|k| k.0.clone());
            }
        }
        self.show_chord();
    }

    /// Lets go are read from the keyboard: once everything is up, the most
    /// keys held at once is the chord.
    fn chord_poll(&self) {
        let done = {
            let mut st = self.st.borrow_mut();
            let Some(c) = st.voice.chord.as_mut() else {
                return;
            };
            let before = c.mods.len() + usize::from(c.key.is_some());
            c.mods
                .retain(|m| MODS.iter().any(|(n, vks)| n == m && vks.iter().any(|&vk| down(vk))));
            if c.key.as_ref().is_some_and(|k| !down(k.1)) {
                c.key = None;
            }
            let after = c.mods.len() + usize::from(c.key.is_some());
            if after > 0 || (c.best_mods.is_empty() && c.best_key.is_none()) {
                (after != before).then(String::new)
            } else {
                Some(chord_name(&c.best_mods, c.best_key.as_deref()))
            }
        };
        match done {
            Some(acc) if !acc.is_empty() => {
                self.end_chord();
                let said = format!("Hold {} to talk.", label(&acc));
                self.voice_set(json!({ "hotkey": acc }), Some(said));
            }
            Some(_) => self.show_chord(),
            None => {}
        }
    }

    // ---------------------------------------------------------- agents

    pub(super) fn watch_load(&self) {
        self.call("agents:get", Value::Null, 0, |p, r| {
            if let Some(v) = r.value() {
                p.watch_take(v);
            }
        });
    }

    /// The whole page's state, from `agents:get`, `agents:set` or
    /// `agents:state`. False when it is not one.
    pub(super) fn watch_take(&self, r: &Value) -> bool {
        if !r["prefs"].is_object() {
            return false;
        }
        self.st.borrow_mut().voice.watch = Some(r.clone());
        self.watch_render();
        true
    }

    fn watch_status(&self, t: &str, error: bool) {
        let s = self.s();
        s.set_watch_status(t.into());
        s.set_watch_status_error(error);
    }

    fn watch_render(&self) {
        let Some(snap) = self.st.borrow().voice.watch.clone() else {
            return;
        };
        let p = &snap["prefs"];
        let s = self.s();
        let enabled = flag(p, "enabled");
        s.set_watch(enabled);
        s.set_chats(flag(p, "chats"));
        s.set_keep_awake(flag(p, "keepAwake"));
        s.set_display_on(flag(p, "displayOn"));
        s.set_lid_awake(flag(p, "lidAwake"));
        s.set_has_lid(flag(&snap, "hasLid"));
        s.set_notify_done(flag(p, "notifyDone"));
        s.set_notify_bg_done(flag(p, "notifyBackgroundDone"));
        s.set_notify_needs(flag(p, "notifyNeedsYou"));
        s.set_notify_bg_needs(flag(p, "notifyBackgroundNeedsYou"));
        s.set_sound(flag(p, "sound"));
        s.set_expand_hover(flag(p, "expandOnHover"));
        s.set_alt_tab(flag(p, "altTab"));
        s.set_show_children(flag(p, "showChildren"));
        s.set_card_time(p["cardTime"].as_i64().unwrap_or(1) as i32);

        // One switch per agent found on this PC, with what it still needs
        // from the person (restart open sessions, trust the hooks).
        let found: Vec<Value> = snap["agents"]
            .as_array()
            .map(|a| a.iter().filter(|a| flag(a, "found")).cloned().collect())
            .unwrap_or_default();
        let rows: Vec<SetToggle> = found
            .iter()
            .map(|a| {
                let err = text(a, "error");
                let note = if !err.is_empty() {
                    err.clone()
                } else if flag(a, "on") && flag(a, "connected") {
                    text(a, "note")
                } else {
                    String::new()
                };
                SetToggle {
                    id: text(a, "id").into(),
                    name: text(a, "name").into(),
                    note: note.into(),
                    on: flag(a, "on"),
                    error: !err.is_empty(),
                }
            })
            .collect();
        s.set_watch_agents(ModelRc::new(VecModel::from(rows)));

        let connected = found
            .iter()
            .filter(|a| flag(a, "on") && flag(a, "connected"))
            .count();
        let failed: Vec<String> = found
            .iter()
            .filter(|a| !text(a, "error").is_empty())
            .map(|a| text(a, "name"))
            .collect();
        s.set_watch_summary(if !enabled {
            "Off".into()
        } else if found.is_empty() {
            "On, none found yet".into()
        } else {
            format!("{connected} of {} connected", found.len()).into()
        });
        if enabled && found.is_empty() {
            self.watch_status("No coding agent was found on this PC. One installed later is picked up within 10 minutes.", false);
        } else if !failed.is_empty() {
            self.watch_status(
                &format!(
                    "Could not connect {}. The reason is under its name.",
                    failed.join(", ")
                ),
                true,
            );
        } else if enabled && flag(&snap, "aevalsistant") {
            self.watch_status("Aevalsistant is installed too. If it is running, quit it from its tray icon: both show the same notification in the same place, one over the other.", false);
        } else {
            self.watch_status("", false);
        }
        // Why chat watching stopped by itself, under its switch.
        s.set_chats_status(if enabled && flag(p, "chats") {
            text(&snap, "chatProblem").into()
        } else {
            SharedString::new()
        });

        let lid = flag(p, "lidAwake") && flag(&snap, "hasLid");
        let awake = if !flag(p, "keepAwake") {
            "Off"
        } else if lid {
            "On, lid closed too"
        } else if flag(p, "displayOn") {
            "On, screen too"
        } else {
            "On"
        };
        s.set_awake_summary(if flag(&snap, "working") && flag(&snap, "awake") {
            format!("{awake} · holding now").into()
        } else {
            awake.into()
        });
        s.set_awake_note(if lid {
            "While agents work, closing the lid does nothing. If the lid is still shut a minute after the last one stops, the PC sleeps or hibernates, whichever your own lid setting says."
        } else {
            "Windows will not sleep until the last agent stops. The screen can still turn off unless you keep it on."
        }
        .into());

        let cards = match (flag(p, "notifyDone"), flag(p, "notifyNeedsYou")) {
            (true, true) => "Finished and needs you",
            (true, false) => "Finished",
            (false, true) => "Needs you",
            (false, false) => "Off",
        };
        s.set_cards_summary(cards.into());
        s.set_test_enabled(true);
    }

    /// Saves through `agents:set`, which answers with the whole page; the
    /// change shows at once and goes back if it did not save. `done` is
    /// said once it saved.
    fn watch_set(&self, patch: Value, done: Option<&'static str>) {
        let before = self.st.borrow().voice.watch.clone();
        if let (Some(mut snap), Some(m)) = (before.clone(), patch.as_object()) {
            for (k, v) in m {
                snap["prefs"][k] = v.clone();
            }
            self.st.borrow_mut().voice.watch = Some(snap);
            self.watch_render();
        }
        self.call("agents:set", patch, 15000, move |p, r| {
            if r.value().is_some_and(|v| p.watch_take(v)) {
                if let Some(t) = done {
                    p.announce(t, "polite");
                }
                return;
            }
            p.st.borrow_mut().voice.watch = before;
            p.watch_render();
            p.watch_status(NOT_SAVED, true);
            p.announce(NOT_SAVED, "assertive");
        });
    }

    pub(super) fn watch_save(&self, patch: Value) {
        self.watch_set(patch, None);
    }

    pub(super) fn watch_enable(&self, on: bool) {
        self.watch_set(
            json!({ "enabled": on }),
            Some(if on {
                "Watching your coding agents."
            } else {
                "Agent watching is off."
            }),
        );
    }

    pub(super) fn watch_agent(&self, id: &str, on: bool) {
        let mut off: Vec<String> = self
            .st
            .borrow()
            .voice
            .watch
            .as_ref()
            .and_then(|w| w["prefs"]["agentsOff"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_str)
            .filter(|a| *a != id)
            .map(str::to_string)
            .collect();
        if !on {
            off.push(id.to_string());
        }
        self.watch_save(json!({ "agentsOff": off }));
    }

    pub(super) fn watch_pref(&self, name: &str, on: bool) {
        self.watch_save(one(name, on));
    }

    pub(super) fn watch_test(&self) {
        self.s().set_test_enabled(false);
        self.call("agents:test", Value::Null, 0, |p, _| {
            p.s().set_test_enabled(true);
        });
    }

    // ---------------------------------------------------- private apps

    pub(super) fn privacy_load(&self) {
        let at = self.st.borrow().voice.changes;
        self.call("privacy:get", Value::Null, 0, move |p, r| {
            let fresh = {
                let st = p.st.borrow();
                st.voice.changes == at && st.voice.pending == 0
            };
            if let Some(v) = r.value().filter(|_| fresh) {
                p.private_take(v);
            }
        });
    }

    /// The section's state; false when the reply is not one.
    fn private_take(&self, r: &Value) -> bool {
        if !r["apps"].is_array() {
            return false;
        }
        self.st.borrow_mut().voice.privacy = Some(r.clone());
        self.private_render();
        true
    }

    fn private_render(&self) {
        let Some(snap) = self.st.borrow().voice.privacy.clone() else {
            return;
        };
        let apps: Vec<SetAppRow> = snap["apps"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|a| SetAppRow {
                        name: text(a, "name").into(),
                        exe: text(a, "exe").into(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let n = apps.len();
        let s = self.s();
        s.set_private_apps(ModelRc::new(VecModel::from(apps)));
        s.set_private_summary(match n {
            0 => "None".into(),
            1 => "1 app".into(),
            n => format!("{n} apps").into(),
        });
        // The app that was in front when the bar last opened, named beside
        // the button that adds it.
        let front = &snap["front"];
        let listed = flag(front, "listed");
        s.set_front_enabled(front.is_object() && !listed);
        s.set_front_note(if !front.is_object() {
            "Open the bar over an app, then come back here.".into()
        } else if listed {
            format!("{} is already on the list.", text(front, "name")).into()
        } else {
            format!("{} ({})", text(front, "name"), text(front, "exe")).into()
        });
    }

    /// An error stays until the next change; anything else clears by
    /// itself.
    fn private_status(&self, t: &str, error: bool) {
        self.s().set_private_error(error);
        self.status("private", t, error);
    }

    /// A change: counted, so a reload that crosses it is dropped.
    fn private_change(
        &self,
        channel: &'static str,
        payload: Value,
        then: impl FnOnce(&SettingsPage, Option<Value>) + 'static,
    ) {
        {
            let mut st = self.st.borrow_mut();
            st.voice.changes += 1;
            st.voice.pending += 1;
        }
        self.call(channel, payload, 8000, move |p, r| {
            {
                let mut st = p.st.borrow_mut();
                st.voice.pending = st.voice.pending.saturating_sub(1);
            }
            let v = r.value().cloned().filter(|v| p.private_take(v));
            then(p, v);
        });
    }

    fn private_add(&self, payload: Value, typed: bool) {
        self.private_change("privacy:add", payload, move |p, v| {
            let Some(v) = v else {
                p.s().set_private_error(false);
                p.unsaved(Some("private"));
                return;
            };
            let err = text(&v, "error");
            if !err.is_empty() {
                p.private_status(&err, true);
                p.announce(&err, "assertive");
                return;
            }
            let a = &v["added"];
            let name = text(a, "name");
            let t = if flag(a, "new") {
                format!("{name} is on the list. Aevaleron will not read it.")
            } else {
                format!("{name} is already on the list.")
            };
            p.private_status(&t, false);
            p.announce(&t, "polite");
            if typed {
                let s = p.s();
                s.set_private_epoch(s.get_private_epoch().wrapping_add(1));
            }
        });
    }

    pub(super) fn private_add_typed(&self, typed: &str) {
        let name = typed.trim();
        if name.is_empty() {
            self.private_status(
                "Type the name of the app's program first, for example notepad.exe.",
                true,
            );
            return;
        }
        self.private_add(json!({ "name": name }), true);
    }

    pub(super) fn private_add_front(&self) {
        self.private_add(json!({ "front": true }), false);
    }

    pub(super) fn private_remove(&self, exe: &str) {
        let name = self
            .st
            .borrow()
            .voice
            .privacy
            .as_ref()
            .and_then(|s| {
                s["apps"]
                    .as_array()
                    .and_then(|a| a.iter().find(|a| text(a, "exe") == exe))
                    .map(|a| text(a, "name"))
            })
            .unwrap_or_else(|| exe.to_string());
        self.private_change("privacy:remove", json!({ "exe": exe }), move |p, v| {
            if v.is_none() {
                p.s().set_private_error(false);
                p.unsaved(Some("private"));
                return;
            }
            let t = format!("{name} was taken off the list.");
            p.private_status(&t, false);
            p.announce(&t, "polite");
        });
    }
}
