//! Settings' Answers, Tasks, Privacy (history and screen sharing),
//! Shortcuts and General pages (settings.js: skills, the agent, history,
//! shortcuts, text size, startup, the round button, updates, bar position).

use std::time::Duration;

use serde_json::{json, Value};
use slint::{ModelRc, SharedString, TimerMode, VecModel};

use super::{ago, flag, num, said_no, text, SettingsPage};
use crate::shell::platform::KeyPress;
use crate::ui::{SetChoice, SetShortcutRow};

/// The global shortcuts, in the order the page lists them.
const SHORTCUTS: [(&str, &str); 5] = [
    ("captureQuery", "Ask or do"),
    ("captureRegion", "Ask about an area"),
    ("copyRegionText", "Copy text from screen"),
    ("agentTask", "Do a task"),
    ("toggleVisibility", "Hide or show the overlay"),
];

/// What each way of asking does, in the row's summary and under the choice.
fn permission(p: &str) -> (&'static str, &'static str) {
    match p {
        "risky" => (
            "Asks for risky steps",
            "Anything that could send, delete, buy or run something waits for you to allow it.",
        ),
        "every" => (
            "Asks before every action",
            "Every click and every key waits for you to allow it.",
        ),
        _ => (
            "Automatic",
            "A safety check reads each risky step against what you asked for. It runs the step when it is clearly part of your request, blocks it when it works against you, and asks you when it is not sure. Money, passwords, sign-ins, installs and command lines always ask you.",
        ),
    }
}

fn megabytes(n: f64) -> String {
    let mb = n / 1_048_576.0;
    if n > 10_485_760.0 {
        format!("{mb:.0} MB")
    } else {
        format!("{mb:.1} MB")
    }
}

/// A key as an accelerator names it, or None for a bare modifier and keys
/// a shortcut cannot use.
fn key_name(k: &KeyPress) -> Option<String> {
    let named = match k.vk {
        0x20 => "Space",
        0x26 => "Up",
        0x28 => "Down",
        0x25 => "Left",
        0x27 => "Right",
        0x6B => "Plus",
        0x0D => "Enter",
        0x09 => "Tab",
        0x08 => "Backspace",
        0x2E => "Delete",
        0x24 => "Home",
        0x23 => "End",
        0x21 => "PageUp",
        0x22 => "PageDown",
        0x2D => "Insert",
        0x70..=0x87 => return Some(format!("F{}", k.vk - 0x6F)),
        0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0..=0xA5 => return None,
        _ => "",
    };
    if !named.is_empty() {
        return Some(named.to_string());
    }
    k.ch.filter(|c| !c.is_control())
        .map(|c| c.to_uppercase().collect())
}

impl SettingsPage {
    /// Everything that comes from `settings:get` and is not a provider.
    pub(super) fn render_all(&self) {
        let d = self.st.borrow().data.clone();
        let s = self.s();
        s.set_version(text(&d, "version").into());
        self.render_update();
        // Below Windows 10 build 19041 the call that hides these windows
        // from a screen share does nothing. Saying so beats letting someone
        // believe it.
        let can_hide = d["canHide"] != json!(false);
        let build = match &d["winBuild"] {
            Value::Null => "unknown".to_string(),
            Value::String(b) => b.clone(),
            b => b.to_string(),
        };
        s.set_share_summary(if can_hide { "Hidden" } else { "Visible" }.into());
        s.set_share_line(if can_hide {
            format!("Every Aevaleron window, the task frame and the pointer included, is left out of screen shares and recordings (Windows build {build}).")
        } else {
            format!("Windows build {build} cannot hide windows from a screen share; that needs build 19041 or later. People watching your screen can see Aevaleron.")
        }
        .into());
        let agent = if d["agent"].is_object() {
            d["agent"].clone()
        } else {
            json!({ "permission": "auto", "maxSteps": 25, "screenshots": true, "returnCursor": true })
        };
        self.render_agent(&agent);
        s.set_show_paid(flag(&d, "showPaid"));
        s.set_brain_enabled(flag(&d, "brainEnabled"));
        s.set_save_history(d["saveHistory"] != json!(false));
        s.set_launch(flag(&d, "launchAtLogin"));
        s.set_show_button(flag(&d, "showButton"));
        self.render_button();
        self.render_history();
        self.render_startup();
        self.render_text_scale();
        self.render_bar_position();
        self.render_key_storage();
        self.render_skills();
        self.render_shortcuts();
    }

    // ---------------------------------------------------------- skills

    fn render_skills(&self) {
        let (list, current) = {
            let st = self.st.borrow();
            (
                st.data["skills"].as_array().cloned().unwrap_or_default(),
                text(&st.data, "skill"),
            )
        };
        let mut choices = vec![SetChoice {
            id: "auto".into(),
            name: "Automatic".into(),
            note: "Picks from what is on screen and what you asked. Recommended.".into(),
            checked: current == "auto" || current.is_empty(),
            enabled: true,
        }];
        let mut active = "Automatic".to_string();
        for sk in &list {
            let id = text(sk, "id");
            let name = text(sk, "name");
            let note = [
                text(sk, "summary"),
                if flag(sk, "truncated") {
                    "Only the first 8 KB is used.".into()
                } else {
                    String::new()
                },
            ]
            .iter()
            .filter(|x| !x.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
            if id == current {
                active.clone_from(&name);
            }
            choices.push(SetChoice {
                checked: id == current,
                id: id.into(),
                name: name.into(),
                note: note.into(),
                enabled: true,
            });
        }
        let s = self.s();
        s.set_skills(ModelRc::new(VecModel::from(choices)));
        s.set_skill_summary(active.into());
    }

    pub(super) fn save_skill(&self, id: &str) {
        self.call(
            "settings:save-skill",
            json!({ "skill": id }),
            0,
            |p, r| {
                if r.failed() {
                    p.unsaved(Some("skills"));
                    return;
                }
                let Some(v) = r.value() else {
                    return;
                };
                p.st.borrow_mut().data["skill"] = v["skill"].clone();
                p.render_skills();
            },
        );
    }

    pub(super) fn open_skills(&self) {
        self.call("skills:open-folder", Value::Null, 0, |p, r| {
            if r.failed() || said_no(r.value()) {
                p.status("skills", "Couldn't open the styles folder.", false);
            }
        });
    }

    // ----------------------------------------------------------- agent

    fn render_agent(&self, a: &Value) {
        self.st.borrow_mut().data["agent"] = a.clone();
        let perm = Some(text(a, "permission"))
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "auto".into());
        let steps = a["maxSteps"].as_f64().unwrap_or(25.0);
        let (summary, note) = permission(&perm);
        let s = self.s();
        s.set_permission(perm.as_str().into());
        s.set_permission_note(note.into());
        s.set_steps(steps as f32);
        s.set_shots(flag(a, "screenshots"));
        s.set_cursor(flag(a, "returnCursor"));
        s.set_revoke_shown(flag(a, "consent"));
        s.set_agent_summary(format!("{summary} · up to {} steps", steps as i64).into());
    }

    /// A change to the agent; a failed save puts the page back as stored.
    /// `done` is said once it saved.
    pub(super) fn set_agent(&self, patch: Value, done: Option<&'static str>) {
        self.call("settings:set-agent", patch, 0, move |p, r| {
            if r.failed() {
                let a = p.st.borrow().data["agent"].clone();
                if a.is_object() {
                    p.render_agent(&a);
                }
                p.unsaved(Some("agent"));
                return;
            }
            if let Some(a) = r.value().map(|v| v["agent"].clone()).filter(Value::is_object) {
                p.render_agent(&a);
            }
            if let Some(t) = done {
                p.status("agent", t, false);
            }
        });
    }

    pub(super) fn revoke_agent(&self) {
        self.set_agent(
            json!({ "consent": false }),
            Some("A task will ask again before it next starts."),
        );
    }

    // --------------------------------------------------------- history

    fn history_count(&self) -> i64 {
        num(&self.st.borrow().data, "historyCount") as i64
    }

    fn render_history(&self) {
        let n = self.history_count();
        let on = self.st.borrow().data["saveHistory"] != json!(false);
        let s = self.s();
        let summary = match (on, n) {
            (false, _) => "Off".to_string(),
            (true, 0) => "On".to_string(),
            (true, n) => format!("{n} saved"),
        };
        s.set_history_summary(summary.into());
        s.set_clear_shown(n > 0);
    }

    pub(super) fn set_save_history(&self, on: bool) {
        self.s().set_save_history(on);
        self.call(
            "settings:set-save-history",
            json!({ "on": on }),
            0,
            |p, r| {
                if r.failed() {
                    let was = p.st.borrow().data["saveHistory"] != json!(false);
                    p.s().set_save_history(was);
                    p.unsaved(Some("history"));
                    return;
                }
                let Some(v) = r.value().cloned() else {
                    return;
                };
                let saved = flag(&v, "saveHistory");
                {
                    let mut st = p.st.borrow_mut();
                    st.data["saveHistory"] = json!(saved);
                    st.data["historyCount"] = v["historyCount"].clone();
                }
                p.s().set_save_history(saved);
                p.render_history();
                p.status(
                    "history",
                    if saved {
                        "New conversations will be kept."
                    } else {
                        "Nothing new will be saved."
                    },
                    false,
                );
            },
        );
    }

    /// Delete all asks inline, with the number.
    pub(super) fn history_confirm_open(&self) {
        let n = self.history_count();
        let t = if n == 1 {
            "Delete the 1 conversation?".to_string()
        } else {
            format!("Delete all {n} conversations?")
        };
        self.announce(&t, "polite");
        self.s().set_history_confirm(t.into());
    }

    pub(super) fn history_confirm(&self, yes: bool) {
        self.s().set_history_confirm(SharedString::new());
        if !yes {
            return;
        }
        let n = self.history_count();
        self.call("settings:clear-history", Value::Null, 0, move |p, r| {
            let Some(v) = r.value().cloned() else {
                p.status("history", "Couldn't delete the history.", true);
                return;
            };
            p.st.borrow_mut().data["historyCount"] = v["historyCount"].clone();
            p.render_history();
            let t = if n == 1 {
                "Deleted 1 conversation.".to_string()
            } else {
                format!("Deleted {n} conversations.")
            };
            p.status("history", &t, false);
            p.announce(&t, "polite");
        });
    }

    // ------------------------------------------------------- shortcuts

    fn render_shortcuts(&self) {
        let (sc, active, capturing) = {
            let st = self.st.borrow();
            (
                st.data["shortcuts"].clone(),
                st.data["active"].clone(),
                st.capturing.clone(),
            )
        };
        let rows: Vec<SetShortcutRow> = SHORTCUTS
            .iter()
            .map(|(action, label)| {
                let now = capturing.as_deref() == Some(*action);
                let set = text(&sc, action);
                let taken = active[*action] == json!(false);
                SetShortcutRow {
                    action: (*action).into(),
                    label: (*label).into(),
                    current: if now {
                        "Press keys".into()
                    } else if set.is_empty() {
                        "Not set".into()
                    } else {
                        set.as_str().into()
                    },
                    empty: !now && set.is_empty(),
                    warn: if taken {
                        "Another app already uses this shortcut. Choose another.".into()
                    } else {
                        SharedString::new()
                    },
                    capturing: now,
                }
            })
            .collect();
        let summary = [text(&sc, "captureQuery"), text(&sc, "agentTask")]
            .iter()
            .filter(|x| !x.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" · ");
        let s = self.s();
        s.set_shortcuts(ModelRc::new(VecModel::from(rows)));
        s.set_shortcut_summary(summary.into());
        self.render_button();
    }

    fn show_free(&self, action: &str, free: &Value) {
        let list: Vec<SharedString> = free
            .as_array()
            .map(|l| {
                l.iter()
                    .filter_map(Value::as_str)
                    .filter(|c| !c.is_empty())
                    .map(SharedString::from)
                    .collect()
            })
            .unwrap_or_default();
        self.st.borrow_mut().free_for = action.to_string();
        self.s().set_free(ModelRc::new(VecModel::from(list)));
    }

    /// Change, or Cancel on the row being recorded: a recording started by
    /// accident is not a trap only Esc gets out of.
    pub(super) fn shortcut_button(&self, action: &str) {
        let now = self.st.borrow().capturing.clone();
        if now.as_deref() == Some(action) {
            self.end_capture();
            return;
        }
        if now.is_some() {
            self.end_capture();
        }
        self.st.borrow_mut().capturing = Some(action.to_string());
        self.status("shortcut", "", false);
        self.show_free(action, &Value::Null);
        self.render_shortcuts();
        self.send("shortcuts:capture-mode", json!({ "on": true }));
        self.announce("Press the new shortcut, or Esc to cancel.", "polite");
    }

    fn end_capture(&self) {
        self.st.borrow_mut().capturing = None;
        self.render_shortcuts();
        self.send("shortcuts:capture-mode", json!({ "on": false }));
    }

    /// A key while a shortcut is recorded.
    pub(super) fn capture_key(&self, k: &KeyPress) {
        if k.repeat {
            return;
        }
        if k.vk == 0x1B {
            self.status("shortcut", "", false);
            self.end_capture();
            return;
        }
        // A bare modifier press: keep listening.
        let Some(key) = key_name(k) else {
            return;
        };
        if !k.ctrl && !k.alt && !k.meta {
            self.status("shortcut", "Add Ctrl, Alt or Win to the shortcut.", true);
            return;
        }
        let mut parts: Vec<String> = Vec::new();
        for (on, name) in [
            (k.ctrl, "Ctrl"),
            (k.alt, "Alt"),
            (k.meta, "Super"),
            (k.shift, "Shift"),
        ] {
            if on {
                parts.push(name.to_string());
            }
        }
        parts.push(key);
        let Some(action) = self.st.borrow().capturing.clone() else {
            return;
        };
        // A failed set stays in capture mode, so another combo can be tried
        // at once or a free one picked below.
        self.apply_shortcut(&action, &parts.join("+"), false);
    }

    fn apply_shortcut(&self, action: &str, acc: &str, said: bool) {
        let action = action.to_string();
        let acc = acc.to_string();
        self.call(
            "shortcuts:set",
            json!({ "action": action, "accelerator": acc }),
            0,
            move |p, r| {
                if r.failed() {
                    p.status("shortcut", "Couldn't change the shortcut. Try again.", true);
                    return;
                }
                let Some(v) = r.value().cloned() else {
                    return;
                };
                if !flag(&v, "ok") {
                    let err = text(&v, "error");
                    p.status("shortcut", &err, true);
                    p.announce(&err, "assertive");
                    p.show_free(&action, &v["free"]);
                    return;
                }
                {
                    let mut st = p.st.borrow_mut();
                    st.data["shortcuts"] = v["shortcuts"].clone();
                    st.data["active"] = v["active"].clone();
                }
                p.status("shortcut", "", false);
                p.show_free(&action, &Value::Null);
                if p.st.borrow().capturing.is_some() {
                    p.end_capture();
                } else {
                    p.render_shortcuts();
                }
                if said {
                    p.announce(&format!("Shortcut set to {acc}."), "polite");
                }
            },
        );
    }

    pub(super) fn use_free(&self, chord: &str) {
        let action = self.st.borrow().free_for.clone();
        if !action.is_empty() {
            self.apply_shortcut(&action, chord, true);
        }
    }

    pub(super) fn reset_shortcuts(&self) {
        self.call("shortcuts:reset", Value::Null, 0, |p, r| {
            if r.failed() {
                p.status("shortcut", "Couldn't reset the shortcuts. Try again.", true);
                return;
            }
            let Some(v) = r.value().cloned() else {
                return;
            };
            {
                let mut st = p.st.borrow_mut();
                st.data["shortcuts"] = v["shortcuts"].clone();
                st.data["active"] = v["active"].clone();
            }
            p.status("shortcut", "", false);
            p.show_free("", &Value::Null);
            p.render_shortcuts();
        });
    }

    // ------------------------------------------------------- text size

    /// Text size: the person's own factor on top of Windows' text size.
    fn render_text_scale(&self) {
        let d = self.st.borrow().data.clone();
        let k = d["textScale"].as_f64().filter(|k| *k > 0.0).unwrap_or(1.0);
        let pct = (k * 100.0).round();
        let os = d["osTextScale"].as_f64().filter(|k| *k > 0.0).unwrap_or(1.0);
        let s = self.s();
        s.set_text_pct(pct as f32);
        s.set_text_note(if (os - 1.0).abs() > 0.001 {
            format!(
                "Starts from your Windows text size. Windows is set to {}%.",
                (os * 100.0).round() as i64
            )
        } else {
            "Starts from your Windows text size.".to_string()
        }
        .into());
    }

    /// The slider moved: shown at once, saved once it rests for 150 ms.
    pub(super) fn slide_text_scale(&self, pct: f32) {
        let k = f64::from(pct) / 100.0;
        let before = self.st.borrow().data["textScale"].as_f64();
        if before.is_some_and(|b| (b - k).abs() < 0.0001) {
            return;
        }
        self.st.borrow_mut().data["textScale"] = json!(k);
        self.render_text_scale();
        let label = self.label.clone();
        self.text_timer
            .start(TimerMode::SingleShot, Duration::from_millis(150), move || {
                super::bridge::send(&label, "settings:set-text-scale", json!({ "scale": k }));
            });
    }

    pub(super) fn on_text_scale(&self, s: &Value) {
        if let Some(k) = s["textScale"].as_f64() {
            self.st.borrow_mut().data["textScale"] = json!(k);
            self.render_text_scale();
            return;
        }
        // Ctrl+= or Ctrl+- in some window: read the stored factor back.
        self.call("settings:get", Value::Null, 0, |p, r| {
            let Some(d) = r.value() else {
                return;
            };
            let Some(k) = d["textScale"].as_f64() else {
                return;
            };
            {
                let mut st = p.st.borrow_mut();
                st.data["textScale"] = json!(k);
                st.data["osTextScale"] = d["osTextScale"].clone();
            }
            p.render_text_scale();
        });
    }

    // --------------------------------------------------------- startup

    fn render_startup(&self) {
        let d = self.st.borrow().data.clone();
        let s = self.s();
        s.set_startup_summary(if flag(&d, "launchAtLogin") { "On" } else { "Off" }.into());
        // Unpackaged, the Run key would point at a development binary
        // rather than at Aevaleron, so the preference is remembered and not
        // applied. Saying so beats a switch that silently does nothing.
        s.set_startup_note(if flag(&d, "launchAtLoginLive") {
            SharedString::new()
        } else {
            "Remembered, but only applied in an installed build.".into()
        });
    }

    pub(super) fn set_launch_at_login(&self, on: bool) {
        self.s().set_launch(on);
        self.call(
            "settings:set-launch-at-login",
            json!({ "on": on }),
            0,
            |p, r| {
                if r.failed() {
                    let was = flag(&p.st.borrow().data, "launchAtLogin");
                    p.s().set_launch(was);
                    p.unsaved(None);
                    return;
                }
                let Some(v) = r.value() else {
                    return;
                };
                let now = flag(v, "launchAtLogin");
                p.st.borrow_mut().data["launchAtLogin"] = json!(now);
                p.s().set_launch(now);
                p.render_startup();
            },
        );
    }

    // ------------------------------------------------- the round button

    /// Off, the bar is on screen only while it is open, and the shortcut or
    /// the tray icon opens it.
    fn render_button(&self) {
        let d = self.st.borrow().data.clone();
        let chord = text(&d["shortcuts"], "captureQuery");
        let on = flag(&d, "showButton");
        let s = self.s();
        s.set_button_summary(if on { "On" } else { "Off" }.into());
        s.set_button_note(if on {
            "The bar opens out of the button. Move it with Bar position.".to_string()
        } else {
            let chord = if chord.is_empty() {
                "the shortcut".to_string()
            } else {
                chord
            };
            format!("Nothing is on screen until you press {chord}. The tray icon opens the bar too.")
        }
        .into());
    }

    pub(super) fn set_show_button(&self, on: bool) {
        self.s().set_show_button(on);
        self.call(
            "settings:set-show-button",
            json!({ "on": on }),
            0,
            |p, r| {
                let Some(v) = r.value().cloned() else {
                    let was = flag(&p.st.borrow().data, "showButton");
                    p.s().set_show_button(was);
                    return;
                };
                let now = flag(&v, "showButton");
                {
                    let mut st = p.st.borrow_mut();
                    st.data["showButton"] = json!(now);
                    if v["overlayPos"].is_object() {
                        st.data["overlayPos"] = v["overlayPos"].clone();
                    }
                }
                p.s().set_show_button(now);
                p.render_button();
                p.render_bar_position();
            },
        );
    }

    /// Where the bar sits.
    fn render_bar_position(&self) {
        let d = self.st.borrow().data.clone();
        let pos = if d["overlayPos"].is_object() {
            &d["overlayPos"]
        } else {
            &d["inputAnchor"]
        };
        let (v, h) = (text(pos, "v"), text(pos, "h"));
        let side = match h.as_str() {
            "center" => "centre".to_string(),
            other => other.to_string(),
        };
        let t = if v.is_empty() || h.is_empty() {
            String::new()
        } else if v == "center" {
            if h == "center" {
                "Middle of the screen".into()
            } else {
                format!("Middle {side}")
            }
        } else {
            format!("{} {side}", if v == "bottom" { "Bottom" } else { "Top" })
        };
        self.s().set_layout_summary(t.into());
    }

    // --------------------------------------------------------- updates

    /// A failed check never reads as an error here; it says when the next
    /// try is. The footer only speaks up when a restart is waiting.
    pub(super) fn render_update(&self) {
        let (u, version) = {
            let st = self.st.borrow();
            (st.data["update"].clone(), text(&st.data, "version"))
        };
        let cur = text(&u, "current");
        let v = if cur.is_empty() { version } else { cur };
        let status = text(&u, "status");
        let to = text(&u, "version");
        let last = num(&u, "lastChecked");
        let checked = if last > 0.0 {
            format!("Checked {}.", ago(last))
        } else {
            String::new()
        };
        let pending = flag(&u, "restartPending");
        let progress = num(&u, "progress");
        let mut summary = v.clone();
        let mut line = match status.as_str() {
            "dev" => {
                summary = format!("{v} · development build");
                "Updates are checked by the installed app, not by a development build.".to_string()
            }
            "checking" => {
                summary = format!("{v} · checking");
                "Checking for a newer version…".into()
            }
            "downloading" => {
                summary = format!("{v} · downloading {to}");
                let total = num(&u, "total");
                if total > 0.0 {
                    format!(
                        "Downloading {to}: {} of {}, {}%.",
                        megabytes(num(&u, "transferred")),
                        megabytes(total),
                        (progress * 100.0).floor() as i64
                    )
                } else {
                    format!("Downloading {to}…")
                }
            }
            "ready" => {
                summary = format!("{v} · {to} ready");
                if pending {
                    format!("{to} installs as soon as the current answer finishes.")
                } else {
                    format!("{to} is downloaded and verified. It installs when you quit, or now.")
                }
            }
            "offline" | "retrying" => format!("{} {checked}", text(&u, "problem"))
                .trim()
                .to_string(),
            "current" => {
                summary = format!("{v} · up to date");
                format!("You have the latest version. {checked}")
                    .trim()
                    .to_string()
            }
            _ if checked.is_empty() => "Checks for updates in the background.".into(),
            _ => checked,
        };
        let from = text(&u, "updatedFrom");
        let moving = status == "ready" || status == "downloading";
        if !from.is_empty() && !moving {
            line = format!("Updated from {from}. {line}");
        }
        let notes = text(&u, "notes");
        let release = text(&u, "releaseUrl");
        let restart = status == "ready" && !pending;
        let s = self.s();
        s.set_about_summary(summary.into());
        s.set_update_line(line.into());
        s.set_update_progress_shown(status == "downloading");
        s.set_update_progress(progress as f32);
        s.set_update_notes(if !notes.is_empty() && moving {
            format!("New in {to}: {notes}").into()
        } else {
            SharedString::new()
        });
        s.set_release_url(release.into());
        s.set_release_label(if !from.is_empty() && status != "ready" {
            "What's new"
        } else {
            "Release notes"
        }
        .into());
        s.set_restart_shown(restart);
        s.set_check_shown(!matches!(status.as_str(), "dev" | "ready" | "downloading"));
        s.set_check_enabled(status != "checking");
        s.set_update_btn_shown(restart);
    }

    pub(super) fn update_check(&self) {
        let before = self.st.borrow().data["update"].clone();
        let mut checking = if before.is_object() {
            before.clone()
        } else {
            json!({})
        };
        checking["status"] = json!("checking");
        self.st.borrow_mut().data["update"] = checking;
        self.render_update();
        self.call("update:check", Value::Null, 0, move |p, r| {
            if let Some(v) = r.value().filter(|v| !text(v, "status").is_empty()) {
                p.st.borrow_mut().data["update"] = v.clone();
                p.render_update();
                return;
            }
            // Never left at "checking" with the button disabled.
            p.st.borrow_mut().data["update"] = before;
            p.render_update();
            p.s()
                .set_update_line("Couldn't check for updates. Try again in a moment.".into());
        });
    }
}
