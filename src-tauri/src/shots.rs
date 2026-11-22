//! `AEVAL_SHOT_DIR=<folder>`: the native screenshots (.github/workflows/
//! ui-shots.yml). Opens the bar, an answer from the fake model, Settings and
//! History, writes each shown page as `<label>-<theme>.png` in the light and
//! the dark theme, then quits: exit code 0 when every capture was written.
//!
//! Run it with `AEVAL_FAKE_LLM=1` and a throwaway `AEVAL_USERDATA`: it saves
//! a stand-in key so the question is asked, and the fixtures answer it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::shell::{platform, AppHandle, Manager};
use crate::{store, windows};

/// How long a closure posted to the UI thread may take to answer.
const UI_WAIT: Duration = Duration::from_secs(10);
/// A window that was asked to open has this long to show.
const OPEN_WAIT: Duration = Duration::from_secs(20);
/// The fake model answers at once; the sheet still has to grow to fit.
const ANSWER_WAIT: Duration = Duration::from_secs(40);
/// Motion and the first paint after a window shows or the theme flips.
const SETTLE: Duration = Duration::from_millis(1500);
const QUESTION: &str = "What does this error mean?";

/// The folder the shots go to, when this is a shots run.
pub fn dir() -> Option<PathBuf> {
    std::env::var_os("AEVAL_SHOT_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
}

/// The whole run, on its own thread; ends the app when it is done.
pub fn run(app: AppHandle) {
    let Some(dir) = dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let mut shots = Shots {
        dir,
        done: BTreeSet::new(),
        failed: Vec::new(),
    };
    shots.scenes(&app);
    let ok = shots.failed.is_empty() && !shots.done.is_empty();
    println!(
        "[SHOT] done written={} failed={} {}",
        shots.done.len(),
        shots.failed.len(),
        shots.failed.join(" ")
    );
    app.exit(if ok { 0 } else { 1 });
}

struct Shots {
    dir: PathBuf,
    /// The labels already written, so the last sweep only adds new pages.
    done: BTreeSet<String>,
    failed: Vec<String>,
}

impl Shots {
    fn scenes(&mut self, app: &AppHandle) {
        let booted = || on_ui(|| crate::ui::page(windows::OVERLAY).is_some()).unwrap_or(false);
        if !wait_for(OPEN_WAIT, booted) {
            self.failed.push("overlay:never-built".into());
            return;
        }
        thread::sleep(SETTLE);

        let a = app.clone();
        let _ = on_ui(move || windows::open_bar(&a, None, None));
        self.opened("bar", windows::OVERLAY);

        // A stand-in key, so the line is asked rather than turned away; the
        // fake model answers it (AEVAL_FAKE_LLM).
        if !store::get().has_api_key() {
            store::get().set_api_key("sk-or-shots-fake");
        }
        let a = app.clone();
        let _ = on_ui(move || windows::submit_text(&a, QUESTION));
        let answered = wait_for(ANSWER_WAIT, || on_ui(|| shown_outputs() > 0).unwrap_or(false));
        if answered {
            thread::sleep(SETTLE * 2);
            self.scene("answer", |l: &str| l.starts_with("output-"));
        } else {
            self.failed.push("answer:no-output-shown".into());
        }

        let a = app.clone();
        let _ = on_ui(move || windows::open_settings(&a, None));
        self.opened("settings", windows::SETTINGS);

        let a = app.clone();
        let _ = on_ui(move || windows::open_history(&a));
        self.opened("history", windows::HISTORY);

        // Whatever else is on screen (cards, the buddy) that no scene took.
        self.scene("rest", |_: &str| true);
    }

    /// Waits for `label` to show, then shoots it.
    fn opened(&mut self, scene: &str, label: &'static str) {
        if wait_for(OPEN_WAIT, || on_ui(move || shown(label)).unwrap_or(false)) {
            thread::sleep(SETTLE);
            self.scene(scene, move |l: &str| l == label);
        } else {
            self.failed.push(format!("{scene}:never-shown"));
        }
    }

    /// Every shown page `want` picks, once per theme. The theme the app
    /// started in comes back afterwards.
    fn scene<W>(&mut self, scene: &str, want: W)
    where
        W: Fn(&str) -> bool + Send + Copy + 'static,
    {
        let started_dark = on_ui(crate::ui::dark).unwrap_or(false);
        let mut taken = BTreeSet::new();
        for dark in [started_dark, !started_dark] {
            if dark != started_dark {
                crate::ui::on_theme_change(dark);
                thread::sleep(SETTLE);
            }
            let theme = if dark { "dark" } else { "light" };
            for (label, w, h, rgba) in grab(want) {
                if self.done.contains(&label) {
                    continue;
                }
                let name = format!("{label}-{theme}.png");
                match write_png(&self.dir.join(&name), w, h, &rgba) {
                    Ok(()) => println!("[SHOT] {scene} {name} {w}x{h}"),
                    Err(e) => {
                        println!("[SHOT] {scene} {name} failed: {e}");
                        self.failed.push(format!("{name}:{e}"));
                    }
                }
                taken.insert(label);
            }
        }
        crate::ui::on_theme_change(started_dark);
        thread::sleep(SETTLE);
        if taken.is_empty() && scene != "rest" {
            self.failed.push(format!("{scene}:nothing-captured"));
        }
        self.done.extend(taken);
    }
}

/// Runs `f` on the UI thread and waits for its answer.
fn on_ui<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> Option<R> {
    let (tx, rx) = mpsc::channel();
    let task = Box::new(move || {
        let _ = tx.send(f());
    });
    platform::post(task).ok()?;
    rx.recv_timeout(UI_WAIT).ok()
}

fn wait_for(limit: Duration, mut ok: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    loop {
        if ok() {
            return true;
        }
        if start.elapsed() >= limit {
            return false;
        }
        thread::sleep(Duration::from_millis(250));
    }
}

/// UI thread: whether the window under `label` is on screen.
fn shown(label: &str) -> bool {
    AppHandle::get()
        .get_webview_window(label)
        .is_some_and(|w| w.is_visible().unwrap_or(false))
}

/// UI thread: how many output windows are on screen (a spare waits hidden).
fn shown_outputs() -> usize {
    crate::ui::labels()
        .iter()
        .filter(|l| l.starts_with("output-") && shown(l))
        .count()
}

/// UI thread, through `on_ui`: a snapshot of every shown page `want` picks.
fn grab<W>(want: W) -> Vec<(String, u32, u32, Vec<u8>)>
where
    W: Fn(&str) -> bool + Send + 'static,
{
    on_ui(move || {
        crate::ui::labels()
            .into_iter()
            .filter(|l| want(l.as_str()) && shown(l))
            .filter_map(|l| {
                let page = crate::ui::page(&l)?;
                let (w, h, rgba) = platform::snapshot(page.hwnd())?;
                Some((l, w, h, rgba))
            })
            .collect()
    })
    .unwrap_or_default()
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut out = enc.write_header().map_err(|e| e.to_string())?;
    out.write_image_data(rgba).map_err(|e| e.to_string())
}
