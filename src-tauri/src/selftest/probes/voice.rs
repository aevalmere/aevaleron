//! The voice lane's smoke labels, listed in `tools/smoke-labels/voice.txt`.
//! CI has no microphone, so every recording here reads the dictation
//! fixture (`capture::set_fixture`), a person saying "Um, so I think we
//! should meet at three, no, four pm tomorrow, to go over the budget."

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::desktop;
use crate::selftest::{notepad, Cx, Input};
use crate::voice::{
    self, capture, engine, insert, keyword, model, pill, settings, wake, wav, whisper, Origin,
};
use crate::windows::OVERLAY;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/voice/fixtures/dictation.wav"
);
const CLIP: &str = "voice probe clipboard";

pub fn run(cx: &mut Cx) {
    runtime(cx);
    let before = crate::store::get().raw(settings::KEY);
    // Whisper's small model for the first checks; the default engine is
    // timed in `speed`.
    let mut s = settings::get();
    s.pick("whisper-tiny");
    s.language = "en".into();
    settings::save(&s);
    capture::set_fixture(Some(PathBuf::from(FIXTURE)));

    if transcribes(cx) {
        bar_field(cx);
        dictation(cx);
        speed(cx);
        listen_for_name(cx);
    }

    capture::set_fixture(None);
    crate::store::get().set_raw(settings::KEY, before.unwrap_or(Value::Null));
}

/// transcribe.cpp is linked with the static C runtime, as the rest of the
/// exe, so it starts on a PC without the Visual C++ redistributable.
fn runtime(cx: &Cx) {
    let exe = std::env::current_exe().and_then(std::fs::read);
    let dlls = exe
        .map(|b| imports(&b).unwrap_or_default())
        .unwrap_or_default();
    let bad: Vec<&String> = dlls
        .iter()
        .filter(|d| d.starts_with("msvcp") || d.starts_with("vcomp") || d.starts_with("vcruntime"))
        .collect();
    cx.must(
        "voiceNoCppRuntime",
        !dlls.is_empty() && bad.is_empty(),
        format!("imports {bad:?} of {}", dlls.len()),
    );
}

/// The DLL names in a PE32+ file's import table, lowercased.
fn imports(b: &[u8]) -> Option<Vec<String>> {
    let u16_at = |o: usize| b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]));
    let u32_at = |o: usize| {
        b.get(o..o + 4)
            .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    };
    let pe = u32_at(0x3C)? as usize;
    let sections = u16_at(pe + 6)? as usize;
    let opt = pe + 24;
    let opt_size = u16_at(pe + 20)? as usize;
    let import_rva = u32_at(opt + 112 + 8)?;
    let table = pe + 24 + opt_size;
    let to_file = |rva: u32| -> Option<usize> {
        (0..sections).find_map(|i| {
            let s = table + i * 40;
            let (size, va, raw) = (u32_at(s + 8)?, u32_at(s + 12)?, u32_at(s + 20)?);
            (rva >= va && rva < va + size.max(1)).then(|| (rva - va + raw) as usize)
        })
    };
    let mut at = to_file(import_rva)?;
    let mut out = Vec::new();
    while let Some(name) = u32_at(at + 12).filter(|n| *n != 0) {
        let start = to_file(name)?;
        let end = b[start..].iter().position(|c| *c == 0)? + start;
        out.push(String::from_utf8_lossy(&b[start..end]).to_lowercase());
        at += 20;
    }
    Some(out)
}

/// The model downloads and checks out, and hears the fixture.
fn transcribes(cx: &Cx) -> bool {
    let spec = model::spec_for("tiny", "en");
    let got = cx.block_on(model::ensure(spec, &|_, _| {}));
    let ready = got.is_ok() && model::ready(spec);
    cx.must("voiceModel", ready, format!("{got:?}"));
    if !ready {
        return false;
    }
    let samples = std::fs::read(FIXTURE)
        .map_err(|e| e.to_string())
        .and_then(|b| wav::read(&b))
        .map(|(s, rate)| wav::resample(&s, rate, wav::RATE));
    let text = samples.and_then(|s| whisper::transcribe(&model::path(spec), &s, "en"));
    cx.log("voiceHeard", format!("{text:?}"));
    let heard = text
        .as_ref()
        .is_ok_and(|t| t.to_lowercase().contains("tomorrow"));
    cx.must("voiceTranscribe", heard, format!("{text:?}"));
    heard
}

/// The bar's mic: start, stop, and the words land in the field.
fn bar_field(cx: &Cx) {
    cx.on_main(|app| crate::windows::open_bar(app, None, None));
    cx.sleep(400);
    let field = || bar_text(cx).to_lowercase();
    // The mic button's own path: start, then stop.
    let started = cx.on_main(|app| voice::start(app, Origin::Bar));
    let listening = matches!(started, Some(Ok(())));
    cx.sleep(600);
    let _ = cx.on_main(|app| voice::finish(app, Origin::Bar));
    let landed = cx.wait_for(Duration::from_secs(40), || field().contains("tomorrow"));
    cx.must(
        "voiceBarField",
        listening && landed,
        format!("listening={started:?} field={:?}", field()),
    );
    clear_bar(cx);
    cx.on_main(crate::windows::collapse_panel);
    cx.sleep(300);
}

fn notepad_text(cx: &Cx, handle: &str) -> String {
    let h = json!(handle);
    let got = cx.block_on(desktop::client().get_text(Some(&h), &Value::Null, 50_000, false));
    got.ok()
        .and_then(|v| v["text"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// A key down or up as a person's keyboard sends it (not marked as ours,
/// so the dictation hook sees it).
fn key(vk: u16, up: bool) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse as km;
    let mut flags = if up { km::KEYEVENTF_KEYUP } else { 0 };
    if vk == 0x5B {
        flags |= km::KEYEVENTF_EXTENDEDKEY;
    }
    // SAFETY: a plain lookup, then one initialised INPUT.
    unsafe {
        let scan = km::MapVirtualKeyW(vk as u32, km::MAPVK_VK_TO_VSC) as u16;
        let i = km::INPUT {
            r#type: km::INPUT_KEYBOARD,
            Anonymous: km::INPUT_0 {
                ki: km::KEYBDINPUT {
                    wVk: vk,
                    wScan: scan,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        km::SendInput(1, &i, std::mem::size_of::<km::INPUT>() as i32);
    }
}

/// Hold Ctrl+Win in Notepad: the pill shows, the words are typed at the
/// caret, and the clipboard is as it was.
fn dictation(cx: &mut Cx) {
    let Some(handle) = cx.notepad().map(|n| n.handle.clone()) else {
        return;
    };
    let _ = cx.block_on(desktop::client().focus_window(&json!(handle)));
    cx.sleep(300);
    let _ = cx.input(Input::Key("Ctrl+End"));
    let _ = insert::set_clipboard_text(CLIP);

    const CTRL: u16 = 0xA2;
    const WIN: u16 = 0x5B;
    key(CTRL, false);
    key(WIN, false);
    let shown = cx.wait_for(Duration::from_secs(3), || {
        let up = cx.on_main(pill::visible).unwrap_or(false);
        let state = cx.probe(pill::LABEL, "state");
        up && state == Ok(json!("listening"))
    });
    cx.sleep(900);
    key(WIN, true);
    key(CTRL, true);
    cx.must("voicePill", shown, "the pill did not show listening");

    let typed = cx.wait_for(Duration::from_secs(40), || {
        notepad_text(cx, &handle)
            .to_lowercase()
            .contains("tomorrow")
    });
    cx.must(
        "voiceDictation",
        typed,
        format!("{:?}", notepad_text(cx, &handle)),
    );
    cx.sleep(800);
    let clip = insert::clipboard_text();
    cx.must(
        "voiceClipboard",
        clip.as_deref() == Some(CLIP),
        format!("{clip:?}"),
    );

    // Put the file's text back for the lanes after.
    let _ = insert::set_clipboard_text(notepad::TEXT);
    let _ = cx.block_on(desktop::client().focus_window(&json!(handle)));
    let _ = cx.input(Input::Key("Ctrl+A"));
    let _ = cx.input(Input::Key("Ctrl+V"));
    cx.sleep(300);
    let _ = cx.on_main(|app| voice::cancel(app, true));
}

/// What the speed gates allow on the runner (4 vCPUs, no GPU), in ms
/// (measurements in lanes/voice-transcribe.md). The runner is one of two
/// processors, an EPYC 9V74 or the slower EPYC 7763, and the times below are
/// in that order. Parakeet 110M, kept loaded: the fixture (7.9 s), the
/// better of two passes, 217 to 227 and 274 to 277. Whisper base on the
/// fixture: 807 to 836 and 978 to 980. The two fixture gates allow a little
/// over 1.4 times what the slower processor took. The default engine,
/// Whisper base (wake lane), on the whole release-to-text of a 17 s
/// utterance with a pause: about 1100 on a Xeon 8573C; it is allowed 1.6
/// times that, as Parakeet's was.
///
/// A runner is at times busy with something else for several seconds (run
/// 43655509204: every pass two to five times its usual for about ten
/// seconds, then as before), so a time that comes out over its budget is
/// taken again after a wait, `RETRIES` times at most, and the best counts. A
/// build that is slow stays slow through all of them.
const SPEED_BUDGET_MS: u64 = 400;
const E2E_BUDGET_MS: u64 = 1800;
const RETRIES: usize = 2;
const QUIET_MS: u64 = 5000;
const WHISPER_BUDGET_MS: u64 = 1400;

fn fixture_16k() -> Result<Vec<f32>, String> {
    std::fs::read(FIXTURE)
        .map_err(|e| e.to_string())
        .and_then(|b| wav::read(&b))
        .map(|(s, rate)| wav::resample(&s, rate, wav::RATE))
}

/// A fixed piece of arithmetic on one core (about 0.1 s on an idle modern
/// core): how much of the processor the probe gets right now, so a slow
/// Whisper time can be told apart from a busy runner.
fn calibrate() -> u64 {
    let t = Instant::now();
    let mut acc = [1.0f32; 8];
    for i in 0..40_000_000u32 {
        let x = std::hint::black_box(i as f32 * 1e-9);
        for a in acc.iter_mut() {
            *a = *a * 0.999_999 + x;
        }
    }
    std::hint::black_box(acc);
    ms(t)
}

fn ms(since: Instant) -> u64 {
    since.elapsed().as_millis() as u64
}

/// Whether a pass heard the fixture's "tomorrow".
fn tomorrow(text: &Result<String, String>) -> bool {
    match text {
        Ok(t) => t.to_lowercase().contains("tomorrow"),
        Err(_) => false,
    }
}

/// Seconds of 16 kHz audio, to one place.
fn secs(samples: &[f32]) -> String {
    format!("{:.1}s", samples.len() as f32 / wav::RATE as f32)
}

/// One pass with the model at `path`: how long it took and what it heard.
fn pass(path: &Path, samples: &[f32]) -> (u64, Result<String, String>) {
    let t = Instant::now();
    let text = whisper::transcribe(path, samples, "en");
    (ms(t), text)
}

/// `first`, or the best of it and the takes of `again` made while the best
/// is over `budget`.
fn settled(cx: &Cx, first: u64, budget: u64, mut again: impl FnMut() -> u64) -> u64 {
    let mut best = first;
    for _ in 0..RETRIES {
        if best <= budget {
            break;
        }
        cx.sleep(QUIET_MS);
        best = best.min(again());
    }
    best
}

/// Whether a 17 s utterance came out inside the default engine's budget.
fn in_time(timings: &Option<voice::Timings>) -> bool {
    let took = timings.as_ref().map(|t| t.total_ms);
    took.is_some_and(|t| t <= E2E_BUDGET_MS)
}

/// The pass just done, as a log line: its time, where the engine spent it
/// and the text.
fn said(took: u64, text: &Result<String, String>) -> String {
    let stages = whisper::stages();
    format!("transcribe={took}ms {stages} text={text:?}")
}

/// What the person is given of a pass: its text after the local tidy.
fn tidied(text: &Result<String, String>) -> String {
    match text {
        Ok(t) => {
            let words = voice::cleanup::strip_noise(t);
            format!("{:?}", voice::cleanup::tidy(&words))
        }
        Err(e) => format!("failed: {e}"),
    }
}

/// Whisper base on the fixture, kept loaded and with the silence cut; then a
/// three-second phrase, the smallest model, Parakeet on the same two clips
/// and a file an earlier build fetched. Last, the release-to-text time of a
/// long utterance through the real pipeline, which transcribes its first
/// half while it is held, with the default engine (Whisper base) and with
/// Parakeet.
fn speed(cx: &Cx) {
    let Ok(clip) = fixture_16k() else {
        cx.must("voiceSpeedWhisper", false, "the fixture did not read");
        return;
    };

    // Whisper base.en, loaded once (at app start, in the app).
    let base = model::spec_for("base", "en");
    let path = match cx.block_on(model::ensure(base, &|_, _| {})) {
        Ok(p) => p,
        Err(e) => {
            cx.must("voiceSpeedWhisper", false, e);
            return;
        }
    };
    let t0 = Instant::now();
    whisper::preload(&path);
    let load_ms = ms(t0);
    let t1 = Instant::now();
    let speech = engine::trim(&clip).unwrap_or(&clip);
    let trim_ms = ms(t1);
    let (run_ms, text) = pass(&path, speech);
    let total = trim_ms + run_ms;
    let kept = format!("{} of {} kept", secs(speech), secs(&clip));
    let heard = said(run_ms, &text);
    let line = format!("load={load_ms}ms (once) trim={trim_ms}ms ({kept}) {heard}");
    cx.log("voiceSpeedAfter", line);
    cx.log("voiceSpeedWhisperTidy", tidied(&text));
    // Taken again if the runner was busy; the gate is further down.
    let total = settled(cx, total, WHISPER_BUDGET_MS, || pass(&path, speech).0);
    // A three-second phrase is most of dictation.
    let short = &clip[..clip.len().min(3 * wav::RATE as usize)];
    let (short_ms, phrase) = pass(&path, short);
    cx.log("voiceSpeedShort", said(short_ms, &phrase));

    let tiny = model::usable(model::spec_for("tiny", "en"));
    if let Some(tp) = tiny {
        whisper::preload(&tp);
        let (tiny_ms, tt) = pass(&tp, speech);
        cx.log("voiceSpeedTiny", said(tiny_ms, &tt));
        // Thread counts, on tiny to keep the run short.
        let mut sweep = Vec::new();
        for n in [1, 2, 4] {
            whisper::set_threads(n);
            let (took, _) = pass(&tp, speech);
            sweep.push(format!("{n}={took}ms"));
        }
        whisper::set_threads(0);
        cx.log("voiceSpeedThreads", sweep.join(" "));
    }
    parakeet(cx, &clip, short);
    legacy(cx, speech);
    cx.log("voiceCalibrate", format!("{}ms", calibrate()));
    let threads = whisper::threads();
    let system = whisper::system_info();
    cx.log("voiceSystem", format!("threads={threads} {system}"));
    cx.must(
        "voiceSpeedWhisper",
        tomorrow(&text) && total <= WHISPER_BUDGET_MS,
        format!("{total}ms of {WHISPER_BUDGET_MS}ms"),
    );

    let id = settings::ENGINES[0];
    let mut run = e2e(cx, &clip, id);
    let mut tries = 1;
    while !in_time(&run.1) && tries <= RETRIES {
        cx.sleep(QUIET_MS);
        run = e2e(cx, &clip, id);
        tries += 1;
    }
    let (started, timings, field) = run;
    let json = serde_json::to_string(&timings).unwrap_or_default();
    cx.log("voiceSpeedTimings", &json);
    cx.log("voiceSpeedHeard", format!("{field:?}"));
    cx.must(
        "voiceSpeedE2e",
        started && in_time(&timings),
        format!("started={started} {json} budget={E2E_BUDGET_MS}ms tries={tries}"),
    );

    let (_, timings, field) = e2e(cx, &clip, settings::PARAKEET);
    let json = serde_json::to_string(&timings).unwrap_or_default();
    cx.log("voiceSpeedParakeetTimings", &json);
    cx.log("voiceSpeedParakeetHeard", format!("{field:?}"));

    spare(cx, &clip);
    tuned(cx, &clip);
}

/// An engine whose model is not on disk (Whisper small, never fetched in
/// this run) is chosen while Parakeet is: the words come at once from
/// Parakeet, with no download in the way, and the chosen model arrives
/// behind them. This is what the first dictation after a change of default
/// engine meets.
fn spare(cx: &Cx, clip: &[f32]) {
    let small = model::spec_for("small", "en");
    if model::usable(small).is_some() {
        cx.must("voiceSpare", false, "the small model is already here");
        cx.must("voiceSpareFetch", false, "the small model is already here");
        return;
    }
    let (started, timings, field) = e2e(cx, clip, "whisper-small");
    let json = serde_json::to_string(&timings).unwrap_or_default();
    cx.log("voiceSpareTimings", &json);
    let by = timings.as_ref().map(|t| t.model.clone());
    let by = by.unwrap_or_default();
    let stood_in = by.starts_with("parakeet");
    let heard = field.to_lowercase().contains("tomorrow");
    cx.must(
        "voiceSpare",
        started && stood_in && heard,
        format!("started={started} model={by:?} field={field:?}"),
    );
    let began = Instant::now();
    let arrived = cx.wait_for(Duration::from_secs(240), || model::ready(small));
    let waited = format!("after {}ms", ms(began));
    cx.must("voiceSpareFetch", arrived, waited);
}

/// The thread measuring ([`whisper::tune`]) on the default engine, Whisper
/// base: it times each count the runner has, keeps one, and the engine
/// still hears the fixture with it. Last of the speed checks, so no gate
/// before it runs on a count it chose.
fn tuned(cx: &Cx, clip: &[f32]) {
    let Some(path) = model::usable(model::spec_for("base", "en")) else {
        cx.must("voiceTuned", false, "Whisper base is not on disk");
        return;
    };
    whisper::preload(&path);
    let line = whisper::tune(&path, &|| false);
    let threads = whisper::threads();
    let whole = engine::piece(&path, clip).unwrap_or(clip);
    let (took, text) = pass(&path, whole);
    cx.log("voiceTunedPass", said(took, &text));
    cx.must(
        "voiceTuned",
        line.is_some() && tomorrow(&text),
        format!("{line:?} threads={threads}"),
    );
}

/// Parakeet 110M on the fixture and on its first three
/// seconds, kept loaded as the app keeps it. It is handed the clip as the app
/// hands it, silence left on; the same clip cut to the speech, and cut then
/// given half a second of quiet either side, are logged beside it. The gate
/// takes the better of two passes: the first after a load is uneven. Over
/// its budget, it is taken again (`settled`).
fn parakeet(cx: &Cx, clip: &[f32], short: &[f32]) {
    let spec = model::parakeet();
    let path = match cx.block_on(model::ensure(spec, &|_, _| {})) {
        Ok(p) => p,
        Err(e) => {
            cx.must("voiceSpeed", false, &e);
            cx.must("voiceParakeet", false, e);
            return;
        }
    };
    let t0 = Instant::now();
    whisper::preload(&path);
    let load_ms = ms(t0);
    let whole = engine::piece(&path, clip).unwrap_or(clip);
    let (run_ms, text) = pass(&path, whole);
    let heard = said(run_ms, &text);
    // Once more: a slow first pass is not the model's speed.
    let (again_ms, _) = pass(&path, whole);
    let line = format!("load={load_ms}ms {heard} again={again_ms}ms");
    cx.log("voiceSpeedParakeet", line);
    cx.log("voiceSpeedParakeetTidy", tidied(&text));
    let first = run_ms.min(again_ms);
    let best = settled(cx, first, SPEED_BUDGET_MS, || pass(&path, whole).0);
    cx.must(
        "voiceSpeed",
        tomorrow(&text) && best <= SPEED_BUDGET_MS,
        format!("{best}ms of {SPEED_BUDGET_MS}ms, first {first}ms"),
    );
    let (short_ms, phrase) = pass(&path, short);
    cx.log("voiceSpeedParakeetShort", said(short_ms, &phrase));
    if let Some(speech) = engine::trim(clip) {
        let (cut_ms, cut) = pass(&path, speech);
        cx.log("voiceSpeedParakeetTrimmed", said(cut_ms, &cut));
        let quiet = vec![0.0; wav::RATE as usize / 2];
        let padded = [&quiet[..], speech, &quiet[..]].concat();
        let (pad_ms, pad) = pass(&path, &padded);
        cx.log("voiceSpeedParakeetPadded", said(pad_ms, &pad));
    }
    cx.must("voiceParakeet", tomorrow(&text), format!("{text:?}"));
}

/// A file an earlier build fetched for whisper.cpp: it is what runs until
/// its GGUF replacement has arrived, so it has to load and hear the fixture.
fn legacy(cx: &Cx, speech: &[f32]) {
    let Some(old) = model::by_id("tiny.en-q5_1") else {
        return;
    };
    let path = match cx.block_on(model::ensure(old, &|_, _| {})) {
        Ok(p) => p,
        Err(e) => {
            cx.log("voiceLegacyBin", format!("download: {e}"));
            return;
        }
    };
    let t0 = Instant::now();
    let engine = whisper::cold(&path);
    let load_ms = ms(t0);
    let t1 = Instant::now();
    let text = engine.and_then(|e| whisper::run(&e, speech, "en", ""));
    let run_ms = ms(t1);
    let line = format!("load={load_ms}ms transcribe={run_ms}ms text={text:?}");
    cx.log("voiceLegacyBin", line);
}

/// What the bar's field holds.
fn bar_text(cx: &Cx) -> String {
    cx.probe(OVERLAY, "input")
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Empties the bar's field the way Rust fills it: `bar:prefill`.
fn clear_bar(cx: &Cx) {
    let empty = json!({ "text": "", "mode": "ask" });
    cx.on_main(move |app| crate::windows::emit(app, OVERLAY, "bar:prefill", empty));
}

/// The fixture, a second of silence and the fixture again, recorded through
/// the bar's mic path with the engine `id`, its model loaded beforehand as
/// the app keeps it: whether it started, its timings and the text that
/// reached the bar's field.
fn e2e(cx: &Cx, clip: &[f32], id: &str) -> (bool, Option<voice::Timings>, String) {
    let mut long = clip.to_vec();
    long.resize(long.len() + wav::RATE as usize, 0.0);
    long.extend_from_slice(clip);
    let file = std::env::temp_dir().join(format!("aeval-voice-long-{}.wav", std::process::id()));
    if std::fs::write(&file, wav::encode(&long, wav::RATE)).is_err() {
        return (false, None, "the long clip did not write".into());
    }
    let mut s = settings::get();
    let was = s.clone();
    s.pick(id);
    s.cleanup = false;
    settings::save(&s);
    if let Some(path) = engine::usable(&s) {
        whisper::preload(&path);
    }
    capture::set_fixture(Some(file.clone()));
    let _ = voice::take_timings();
    let started = cx.on_main(|app| voice::start(app, Origin::Bar));
    let held = long.len() as u64 * 1000 / wav::RATE as u64 + 300;
    cx.sleep(held);
    let _ = cx.on_main(|app| voice::finish(app, Origin::Bar));
    let mut timings = None;
    let _ = cx.wait_for(Duration::from_secs(30), || {
        timings = voice::take_timings();
        timings.is_some()
    });
    capture::set_fixture(Some(PathBuf::from(FIXTURE)));
    settings::save(&was);
    let _ = std::fs::remove_file(&file);
    cx.sleep(300);
    let field = bar_text(cx);
    clear_bar(cx);
    (matches!(started, Some(Ok(()))), timings, field)
}

// ---------------------------------------------------- listening for the name

/// Speech from Windows' own voice (SAPI), 16 kHz mono: CI has no person to
/// say the name, and no recording of one is kept in the repo. The file is
/// in the temp folder only while it is read back.
fn synth(text: &str) -> Result<Vec<f32>, String> {
    use std::os::windows::process::CommandExt;
    const NO_WINDOW: u32 = 0x0800_0000;
    let file = std::env::temp_dir().join(format!("aeval-wake-{}.wav", std::process::id()));
    let script = "Add-Type -AssemblyName System.Speech; \
        $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
        $f = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, \
        [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, \
        [System.Speech.AudioFormat.AudioChannel]::Mono); \
        $s.SetOutputToWaveFile($env:AEVAL_WAKE_OUT, $f); \
        $s.Speak($env:AEVAL_WAKE_TEXT); $s.Dispose()";
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("AEVAL_WAKE_OUT", &file)
        .env("AEVAL_WAKE_TEXT", text)
        .creation_flags(NO_WINDOW)
        .output()
        .map_err(|e| e.to_string())?;
    let read = std::fs::read(&file);
    let _ = std::fs::remove_file(&file);
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let (samples, rate) = wav::read(&read.map_err(|e| e.to_string())?)?;
    let samples = wav::resample(&samples, rate, wav::RATE);
    // Only the speech, so the pauses below are the ones the scene asks for.
    let speech = engine::trim(&samples).unwrap_or(&samples[..]);
    Ok(speech.to_vec())
}

/// `secs` of silence at 16 kHz.
fn quiet(secs: f32) -> Vec<f32> {
    vec![0.0; (secs * wav::RATE as f32) as usize]
}

/// The clip's length in ms.
fn clip_ms(samples: &[f32]) -> u64 {
    samples.len() as u64 * 1000 / wav::RATE as u64
}

/// Turns listening on or off as Settings does.
fn set_wake(on: bool) {
    let mut s = settings::get();
    s.wake = on;
    s.wake_word = settings::WAKE_WORD.into();
    settings::save(&s);
    voice::refresh(&s);
}

/// Plays `clip` into the listening and waits for what it sends, at most
/// `wait` ms after the clip ends.
fn hear(
    cx: &Cx,
    rx: &std::sync::mpsc::Receiver<String>,
    clip: &[f32],
    wait: u64,
) -> Option<String> {
    while rx.try_recv().is_ok() {}
    wake::play(clip);
    let deadline = clip_ms(clip) + wait;
    let got = rx.recv_timeout(Duration::from_millis(deadline)).ok();
    // Let the machine go back to waiting before the next scene.
    cx.sleep(1500);
    got
}

/// Each model that could check for the name, on the name alone and on the
/// first 1.8 s of a sentence that starts with it: time, text and verdict.
/// The checker is picked from these (`wake::checker_model`).
fn bench(cx: &Cx, name: &[f32], breath: &[f32]) {
    let word = settings::WAKE_WORD;
    let head = &breath[..breath.len().min(wav::RATE as usize * 18 / 10)];
    let models = [
        ("Base", model::spec_for("base", "en"), word),
        ("Tiny", model::spec_for("tiny", "en"), word),
        ("Parakeet", model::parakeet(), ""),
    ];
    for (tag, spec, prompt) in models {
        let label = format!("voiceWakeBench{tag}");
        let Some(path) = model::usable(spec) else {
            cx.log(&label, "not on disk");
            continue;
        };
        let t0 = Instant::now();
        let engine = match whisper::cold(&path) {
            Ok(e) => e,
            Err(e) => {
                cx.log(&label, e);
                continue;
            }
        };
        let load_ms = ms(t0);
        // The first pass after a load is uneven.
        let _ = whisper::run(&engine, name, "en", prompt);
        let mut line = format!("load={load_ms}ms");
        for (what, clip) in [("name", name), ("head", head)] {
            let t = Instant::now();
            let text = whisper::run(&engine, clip, "en", prompt);
            let took = ms(t);
            let verdict = text.as_deref().map(|t| keyword::verdict(t, word));
            line.push_str(&format!(" {what}={took}ms {text:?} {verdict:?}"));
        }
        cx.log(&label, line);
    }
}

/// Listening for the name, on Windows' own voice played into the listening
/// in real time (`wake::use_test_source`), with the default engine: what it
/// costs while nobody speaks, one breath, a pause after the name, a
/// sentence without it, the bare name, the bar's mic first, and off.
fn listen_for_name(cx: &Cx) {
    let word = settings::WAKE_WORD;
    let clips = (|| -> Result<_, String> {
        Ok((
            synth(&format!("{word}, what is the weather tomorrow?"))?,
            synth(word)?,
            synth("open my calendar for Friday")?,
            synth("I think we should meet tomorrow to go over the budget.")?,
        ))
    })();
    let (breath, name, ask, other) = match clips {
        Ok(c) => c,
        Err(e) => {
            cx.must("voiceWakeListens", false, format!("no speech to play: {e}"));
            return;
        }
    };
    bench(cx, &name, &breath);

    let was = settings::get();
    let mut s = was.clone();
    s.pick(settings::WHISPER_BASE);
    s.cleanup = false;
    settings::save(&s);
    let (tx, rx) = std::sync::mpsc::channel();
    wake::set_sink(Some(tx));
    wake::use_test_source(true);
    wake::reset_stats();
    set_wake(true);
    let up = cx.wait_for(Duration::from_secs(60), wake::listening);
    let status = wake::status_json();
    cx.must("voiceWakeListens", up, format!("{status}"));
    if up {
        // The checker's model loads on its own thread first.
        cx.sleep(3000);
        scenes(cx, &rx, &breath, &name, &ask, &other);
    }
    set_wake(false);
    let off = cx.wait_for(Duration::from_secs(5), || !wake::listening());
    let status = wake::status_json();
    cx.must(
        "voiceWakeOff",
        off && status["state"] == "off",
        format!("{status}"),
    );
    wake::use_test_source(false);
    wake::set_sink(None);
    settings::save(&was);
    voice::refresh(&was);
}

/// The listening's processor time, as its thread counts it (ms).
fn cpu_now(cx: &Cx) -> u64 {
    // Updated once a second.
    cx.sleep(1100);
    wake::stats().cpu_ms
}

fn scenes(
    cx: &Cx,
    rx: &std::sync::mpsc::Receiver<String>,
    breath: &[f32],
    name: &[f32],
    ask: &[f32],
    other: &[f32],
) {
    // Nobody speaks: under 1% of one core.
    const IDLE_S: u64 = 10;
    let c0 = cpu_now(cx);
    let st0 = wake::stats();
    cx.sleep(IDLE_S * 1000);
    let c1 = cpu_now(cx);
    let st1 = wake::stats();
    let per_cent = c1.saturating_sub(c0) as f64 / ((IDLE_S * 1000 + 1100) as f64) * 100.0;
    let loops = st1.loops.saturating_sub(st0.loops).max(1);
    let per_loop = st1.work_us.saturating_sub(st0.work_us) / loops;
    let checks = st1.checks.saturating_sub(st0.checks);
    let idle = format!("cpu={per_cent:.2}% of one core, {per_loop}us a pass, checks={checks}");
    cx.log("voiceWakeIdleCost", &idle);
    cx.must("voiceWakeIdle", per_cent < 1.0 && checks == 0, idle);

    // The name and the question in one breath.
    let one = [breath, &quiet(2.0)].concat();
    let got = hear(cx, rx, &one, 15_000);
    let st = wake::stats();
    let timing = format!(
        "nameToPill={}ms check={}ms ({}) transcribe={}ms speechEndToSent={}ms heard={:?}",
        st.heard_ms, st.check_ms, st.check_model, st.transcribe_ms, st.sent_ms, st.last_text
    );
    cx.log("voiceWakeTimings", &timing);
    let ok = got.as_deref().is_some_and(|l| {
        let l = l.to_lowercase();
        l.contains("weather") && !l.starts_with("aeval")
    });
    cx.must("voiceWakeOneBreath", ok, format!("sent={got:?} {timing}"));

    // The name, a pause of 1.2 s, then the question.
    let paused = [name, &quiet(1.2), ask, &quiet(2.0)].concat();
    let got = hear(cx, rx, &paused, 15_000);
    let ok = got
        .as_deref()
        .is_some_and(|l| l.to_lowercase().contains("calendar"));
    cx.must("voiceWakePause", ok, format!("sent={got:?}"));

    // A sentence without the name sends nothing.
    let plain = [other, &quiet(2.0)].concat();
    let got = hear(cx, rx, &plain, 6_000);
    let st = wake::stats();
    cx.must(
        "voiceWakeIgnores",
        got.is_none(),
        format!("sent={got:?} heard={:?}", st.last_text),
    );
    cx.sleep(1000);

    // The bare name and nothing after: nothing is sent and the pill goes.
    let bare = [name, &quiet(4.0)].concat();
    let got = hear(cx, rx, &bare, 6_000);
    let shown = cx.on_main(pill::visible).unwrap_or(false);
    cx.must(
        "voiceWakeBare",
        got.is_none() && !shown,
        format!("sent={got:?} pill={shown}"),
    );

    // The bar's mic is recording: the name is not listened for.
    let started = cx.on_main(|app| voice::start(app, Origin::Bar));
    let busy = matches!(started, Some(Ok(())));
    let got = hear(cx, rx, &one, 3_000);
    let _ = cx.on_main(|app| voice::cancel(app, true));
    cx.sleep(1000);
    cx.must(
        "voiceWakeYields",
        busy && got.is_none(),
        format!("bar recording={busy} sent={got:?}"),
    );
}
