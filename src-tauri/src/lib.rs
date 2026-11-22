//! Aevaleron's Rust core. ARCHITECTURE.md is the map: each module below owns
//! one responsibility of the Electron main process, and PARITY.md lists what
//! each must keep doing.
//!
//! The app is one native process: `shell` is the window layer and event
//! loop that replaced Tauri and WebView2, and `ui` holds the pages, drawn
//! by Slint's software renderer (lane native-ui).

// serde_json::json! recurses once per field; the store defaults,
// settings:get and overlay:state are bigger than the default limit allows.
#![recursion_limit = "256"]

mod agent;
// agents lane: watching other coding agents (Claude Code, Codex, ...).
mod agents;
mod ask;
pub mod autostart;
mod backdrop;
mod buddy;
mod capture;
pub mod crash;
mod desktop;
pub mod history;
mod host;
pub mod hotkeys;
pub mod instance;
pub mod ipc;
mod lens;
mod llm;
pub mod notify;
pub mod paths;
// private apps lane: programs Aevaleron never reads.
mod privacy;
mod region;
// region-text lane: copy the text inside a rectangle of the screen.
mod region_text;
mod selftest;
// The app shell that replaced Tauri: native windows and the event loop.
pub mod shell;
mod shots;
mod skills;
pub mod state;
pub mod store;
pub mod tray;
// The pages, native.
pub mod ui;
pub mod updater;
// voice lane
mod voice;
pub mod windows;

use shell::{AppHandle, Manager};

fn quiet_run() -> bool {
    // The CI checks too: a first-run tour would put the buddy's full-screen
    // window over the points the desktop selfcheck probes.
    [
        "AEVAL_SMOKE",
        "AEVAL_SHOT",
        "AEVAL_SHOT_DIR",
        "AEVAL_CAPTURE_CHECK",
        "AEVAL_DESKTOP_CHECK",
    ]
    .iter()
    .any(|k| std::env::var_os(k).is_some())
}

/// main.js presenceToast (LIFE-12): once per fresh install, or whenever the
/// main chord is taken by another app.
fn presence_toast(app: &AppHandle) {
    if std::env::var_os("AEVAL_SMOKE").is_some() {
        return;
    }
    let s = store::get();
    let chord = s.shortcut("captureQuery");
    let failed = hotkeys::main_chord_failed(app);
    let fresh = s.take_presence_notice();
    if !failed && !fresh {
        return;
    }
    let (title, body) = if failed {
        (
            format!("{chord} is taken by another app"),
            "Open Aevaleron from the tray icon by the clock, then pick another shortcut in Settings."
                .to_string(),
        )
    } else {
        (
            "Aevaleron is running".to_string(),
            format!("Press {chord} anywhere to ask or do. It starts with Windows; turn that off in Settings."),
        )
    };
    let a = app.clone();
    notify::show(
        &title,
        &body,
        Some(Box::new(move || {
            let a2 = a.clone();
            let _ = a.run_on_main_thread(move || {
                crash::guard("presence-toast", || {
                    if failed {
                        windows::open_settings(&a2, Some("sec-shortcuts"));
                    } else {
                        windows::open_bar(&a2, None, None);
                    }
                });
            });
        })),
    );
}

/// LIFE-10: a task still running at quit is told why it stopped.
const QUIT_REASON: &str = "Stopped because Aevaleron quit.";

/// Stops a running task with the quit reason. False when none was running.
pub(crate) fn stop_for_exit() -> bool {
    host::try_services().is_some_and(|s| s.agent.stop(Some(QUIT_REASON)))
}

/// LIFE-09, LIFE-10, LIFE-11: what before-quit and will-quit did, on the
/// way out.
fn on_exit(app: &AppHandle) {
    stop_for_exit();
    hotkeys::unregister_all();
    history::get().flush_now();
    desktop::shutdown();
    updater::destroy();
    updater::install_on_quit();
    tray::destroy(app);
}

/// Everything that was Tauri's `setup`, on the UI thread before the loop.
fn setup(handle: &AppHandle, doorbell: &instance::Doorbell) -> shell::Result<()> {
    if quiet_run() {
        // The checks work the round button: its window is what they
        // sample, click and measure while the bar is closed.
        store::get().set_show_button(true);
        // And the gate that asks: its card is what they answer.
        store::get().set_agent_settings(&serde_json::json!({ "permission": "risky" }));
    }
    windows::boot(handle)?;
    // Desktop helper client and the CI checks (split c).
    desktop::setup(handle);
    // Ask, the agent and the hooks between the splits.
    host::setup(handle);
    // voice lane: the dictation chord and its pill.
    voice::setup(handle);
    // agents lane: the hook pipe, keep-awake and the card.
    agents::setup(handle);
    store::get().decide_presence();
    // Not apply: boot leaves Task Manager's StartupApproved alone.
    autostart::sync();
    // Before hotkeys::apply, which refreshes the tray labels.
    if let Err(e) = tray::init(handle) {
        crash::write("tray", &e.to_string());
    }
    hotkeys::apply(handle);
    hotkeys::report_failures(handle);
    updater::init(handle, &paths::user_data_dir());
    presence_toast(handle);
    let s = store::get();
    if !s.has_api_key() && !s.get_walkthrough_auto_shown() && !quiet_run() {
        s.set_walkthrough_auto_shown();
        let h = handle.clone();
        let tour = async move {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            if state::shared(&h).lock().mode != state::Mode::Hud {
                state::shared(&h).call(&h, |k| k.start_tour.as_ref());
                hotkeys::apply(&h);
            }
        };
        shell::async_runtime::spawn(crash::guard_async("first-tour", tour));
    }
    if std::env::var_os("AEVAL_SMOKE").is_none() {
        state::shared(handle).call(handle, |k| k.warm_catalogue.as_ref());
    }
    if !quiet_run() {
        windows::start_keep_on_top(handle);
    }
    if selftest::smoke_enabled() {
        let h = handle.clone();
        std::thread::spawn(move || selftest::scaffold(h));
    }
    if shots::dir().is_some() {
        let h = handle.clone();
        std::thread::spawn(move || shots::run(h));
    }
    // LIFE-01: a second launch opens the bar; one that came during boot is
    // answered now.
    let h = handle.clone();
    doorbell.answer(move || {
        let h2 = h.clone();
        let _ = h.run_on_main_thread(move || {
            crash::guard("second-instance", || windows::trigger_capture(&h2));
        });
    });
    Ok(())
}

pub fn run() {
    // agents lane: a coding agent's hook call forwards its event to the
    // running app and exits, before anything else starts.
    if agents::hook::run_if_asked() {
        return;
    }
    windows::mark_start();
    notify::set_app_user_model_id();
    // Before anything reads the folder: an AevalBuddy profile becomes ours.
    paths::adopt_old_name();
    let user_data = paths::user_data_dir();
    let _ = std::fs::create_dir_all(&user_data);
    crash::install(&user_data);
    // LIFE-01: a second launch opens the first copy's bar and exits.
    let Some(lock) = instance::acquire(&user_data) else {
        return;
    };
    // Before the windows exist, so a launch during a slow boot is heard; its
    // `open` is answered when setup is done.
    let doorbell = instance::serve(lock.name.clone());
    store::init(&user_data);
    history::init(&user_data).set_enabled(store::get().get_save_history());

    // This thread is the UI thread: it owns every window and runs the loop.
    shell::platform::init();
    let handle = AppHandle::get();
    handle.manage(ipc::Seen::default());
    handle.manage(state::Shared::default());
    shell::set_event_sink(ui::deliver);
    shell::set_window_event_handler(|window, event| {
        crash::guard("window-event", || windows::on_window_event(window, event));
    });
    shell::set_page_loaded_handler(|window| {
        crash::guard("page-load", || {
            windows::on_page_loaded(window.app_handle(), window.label())
        });
    });
    shell::sys::on_theme_change(ui::on_theme_change);
    hotkeys::init(&handle);

    if let Err(e) = setup(&handle, &doorbell) {
        crash::write("setup", &e.to_string());
        std::process::exit(1);
    }
    let code = shell::platform::run_loop();
    // LIFE-09: the way out, as Tauri's RunEvent::Exit was.
    crash::guard("exit", || on_exit(&handle));
    drop(doorbell);
    drop(lock);
    std::process::exit(code);
}
