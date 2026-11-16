//! Turns a recording into text with the engine Settings chose: Parakeet on
//! this PC (the default, free and offline), Whisper on this PC, or Gemini
//! with the person's key.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::settings::VoiceSettings;
use super::{gemini, model, settings, stream, vad, whisper};

/// What an engine heard; `cleaned` when it already tidied the text. `model`
/// is the file that heard it (`gemini` for Gemini), for the timing log.
pub struct Heard {
    pub text: String,
    pub cleaned: bool,
    pub model: String,
}

/// The languages Settings offers that Parakeet 0.6B hears; `auto` is left
/// to it, and it tells its 25 languages apart by itself.
const V3_HEARS: [&str; 9] = ["en", "auto", "es", "fr", "de", "it", "pt", "nl", "pl"];

/// The model an engine on this PC uses; None for Gemini. Parakeet 110M
/// hears English only and Parakeet 0.6B some languages: asked for one it
/// does not hear, a Parakeet engine's place is taken by Whisper's base
/// model, which keeps the Language setting true.
pub fn spec(s: &VoiceSettings) -> Option<&'static model::Spec> {
    let language = s.language.as_str();
    match s.engine.as_str() {
        settings::PARAKEET if language == "en" => Some(model::parakeet()),
        settings::PARAKEET_V3 if V3_HEARS.contains(&language) => Some(model::parakeet_v3()),
        settings::PARAKEET | settings::PARAKEET_V3 => Some(model::spec_for("base", language)),
        _ => s.whisper_size().map(|w| model::spec_for(w, language)),
    }
}

/// Whether Whisper is standing in for the Parakeet engine chosen.
pub fn stand_in(s: &VoiceSettings) -> bool {
    let own = spec(s).is_some_and(|m| m.id.starts_with("parakeet"));
    s.engine.starts_with("parakeet") && !own
}

/// The models that can hear `language` in place of `own` while it is not on
/// disk, the quickest good one first. English is heard by the English
/// models and by the ones for every language.
fn spares(own: &model::Spec, language: &str) -> Vec<&'static model::Spec> {
    let english = language == "en";
    let mut list: Vec<&'static model::Spec> = Vec::new();
    if english {
        list.push(model::parakeet());
    }
    let sizes = ["base", "tiny", "small"];
    list.extend(sizes.iter().map(|size| model::spec_for(size, language)));
    if english {
        list.extend(sizes.iter().map(|size| model::spec_for(size, "auto")));
    }
    if V3_HEARS.contains(&language) {
        list.push(model::parakeet_v3());
    }
    list.retain(|m| m.id != own.id);
    list
}

/// A model on disk that can hear the words while the chosen one is not
/// there. A dictation never waits for a download when there is one: the
/// first words after the default engine changed took 34 s on the owner's PC,
/// all of it the new model arriving, with last week's model beside it.
fn spare(s: &VoiceSettings) -> Option<PathBuf> {
    let own = spec(s)?;
    let others = spares(own, &s.language);
    others.into_iter().find_map(model::usable)
}

/// The model file to run now, if one is on disk: the chosen engine's (a
/// file an earlier build fetched counts until its replacement arrives), or
/// another that hears the language until the chosen one has arrived.
pub fn usable(s: &VoiceSettings) -> Option<PathBuf> {
    let own = spec(s)?;
    model::usable(own).or_else(|| spare(s))
}

/// Whether the engine can run now without a download or a missing key.
pub fn ready(s: &VoiceSettings) -> bool {
    match spec(s) {
        Some(m) => model::usable(m).is_some(),
        None => gemini::available(),
    }
}

/// Fetches the model in the background when only a file an earlier build
/// fetched is on disk, then deletes the old one.
pub fn upgrade(s: &VoiceSettings) {
    let Some(m) = spec(s) else { return };
    if model::ready(m) {
        model::drop_legacy(m);
        return;
    }
    if model::usable(m).is_none() {
        // Nothing of this model is here. With dictation on, or a model from
        // before on disk (voice is in use), it is fetched now and not at the
        // first words. The smoke run fetches what its probes ask for.
        let wanted = s.dictation || spare(s).is_some();
        if wanted && !crate::selftest::smoke_enabled() {
            fetch(s);
        }
        return;
    }
    tauri::async_runtime::spawn(async move {
        if model::ensure(m, &|_, _| {}).await.is_err() {
            return;
        }
        // The old file cannot be deleted while its model is loaded: the new
        // one takes its place in memory first.
        let old = model::legacy_for(m).any(|l| whisper::loaded(&model::path(l)));
        if old {
            let new = model::path(m);
            let swap = tauri::async_runtime::spawn_blocking(move || {
                whisper::unload();
                whisper::preload(&new)
            });
            let _ = swap.await;
        }
        model::drop_legacy(m);
    });
}

static FETCHING: AtomicBool = AtomicBool::new(false);

/// Downloads the chosen engine's model behind what the person is doing, one
/// at a time, and loads it once it is in if it is still the one chosen and
/// dictation keeps a model loaded. A failed download is tried again by the
/// next call.
pub fn fetch(s: &VoiceSettings) {
    let Some(m) = spec(s) else { return };
    if model::ready(m) || FETCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let got = model::ensure(m, &|_, _| {}).await;
        FETCHING.store(false, Ordering::SeqCst);
        if got.is_err() {
            return;
        }
        let now = settings::get();
        let chosen = spec(&now).is_some_and(|x| x.id == m.id);
        if chosen && now.dictation && whisper::cpu_ok() {
            let new = model::path(m);
            let load = tauri::async_runtime::spawn_blocking(move || {
                if whisper::preload(&new) {
                    tune_soon(new);
                }
            });
            let _ = load.await;
        }
        model::drop_legacy(m);
    });
}

static TUNE_ASKED: AtomicBool = AtomicBool::new(false);
/// After the model is loaded the app is still starting; the measuring waits
/// for that to pass, and tries again this often while someone dictates.
const TUNE_WAIT: Duration = Duration::from_secs(8);
const TUNE_TRIES: u32 = 6;

/// Measures, a little later and on a thread of its own, how many threads
/// run the model at `path` quickest on this PC ([`whisper::tune`]). Once in
/// a run of the app, and never in the smoke run, whose probes time the
/// engine themselves and call the measuring by hand.
pub fn tune_soon(path: PathBuf) {
    let asked = TUNE_ASKED.swap(true, Ordering::SeqCst);
    if asked || crate::selftest::smoke_enabled() {
        return;
    }
    std::thread::spawn(move || {
        let busy = || super::recording(None);
        for _ in 0..TUNE_TRIES {
            std::thread::sleep(TUNE_WAIT);
            if busy() {
                continue;
            }
            if let Some(line) = whisper::tune(&path, &busy) {
                super::log_line(&serde_json::json!({ "threads": line }));
                return;
            }
        }
    });
}

/// `samples` with the silence before and after the speech cut off; None
/// when nothing was said.
pub fn trim(samples: &[f32]) -> Option<&[f32]> {
    vad::speech(samples).map(|r| &samples[r])
}

/// What the model at `path` is handed of `samples`; None when nothing was
/// said. Whisper gets the speech with the silence cut off. Parakeet gets all
/// of it: cut that close, the 110M model wrote the fixture's "tomorrow" as
/// "to morrow" and left out every comma, and its time grows with the clip by
/// little.
pub fn piece<'a>(path: &Path, samples: &'a [f32]) -> Option<&'a [f32]> {
    let speech = trim(samples)?;
    let whole = [model::parakeet(), model::parakeet_v3()]
        .iter()
        .any(|m| path == model::path(m));
    Some(if whole { samples } else { speech })
}

pub async fn transcribe(
    s: &VoiceSettings,
    samples: Vec<f32>,
    early: stream::Done,
    on_progress: &(dyn Fn(u64, u64) + Send + Sync),
) -> Result<Heard, String> {
    let Some(m) = spec(s) else {
        let text = gemini::transcribe(&samples, &s.language, s.cleanup).await?;
        return Ok(Heard {
            text,
            cleaned: s.cleanup,
            model: "gemini".into(),
        });
    };
    if !whisper::cpu_ok() {
        return Err(whisper::OLD_CPU.into());
    }
    let mut path = match model::usable(m).or_else(|| spare(s)) {
        Some(p) => p,
        None => model::ensure(m, on_progress).await?,
    };
    if !model::ready(m) && model::usable(m).is_none() {
        // Heard with another model that is on disk; the chosen one arrives
        // behind it.
        fetch(s);
    }
    if path != model::path(m) {
        // An earlier build's file. Should this engine not read it, its
        // replacement is fetched now rather than in the background.
        let old = path.clone();
        let reads = tauri::async_runtime::spawn_blocking(move || whisper::preload(&old));
        if !reads.await.unwrap_or(false) {
            path = model::ensure(m, on_progress).await?;
        }
    }
    let language = s.language.clone();
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
    let text = tauri::async_runtime::spawn_blocking(move || {
        let rest = samples.get(early.upto..).unwrap_or_default();
        let tail = match piece(&path, rest) {
            Some(speech) => whisper::transcribe_after(&path, speech, &language, &early.text)?,
            None => String::new(),
        };
        Ok::<String, String>(stream::join(&early.text, &tail))
    })
    .await
    .map_err(|_| "Transcription stopped unexpectedly.".to_string())??;
    Ok(Heard {
        text,
        cleaned: false,
        model: name.unwrap_or_default(),
    })
}

/// Loads the model if it is on disk and not in memory, on a thread of its
/// own. A missing model waits for the first release, which shows the
/// download.
pub fn warm(s: &VoiceSettings) {
    if !whisper::cpu_ok() {
        return;
    }
    if let Some(path) = usable(s).filter(|p| !whisper::loaded(p)) {
        std::thread::spawn(move || {
            if whisper::preload(&path) {
                tune_soon(path);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(own: &model::Spec, language: &str) -> Vec<&'static str> {
        spares(own, language).iter().map(|m| m.id).collect()
    }

    #[test]
    fn english_is_heard_by_what_is_on_disk_until_the_chosen_model_arrives() {
        // The default engine is not here yet: Whisper's base model from an
        // earlier build is the first one asked for.
        let for_parakeet = ids(model::parakeet(), "en");
        assert_eq!(for_parakeet[0], "base.en");
        assert!(!for_parakeet.contains(&"parakeet-110m"));
        assert!(for_parakeet.contains(&"base"), "{for_parakeet:?}");
        // The other way round, Parakeet stands in for a Whisper model.
        let for_small = ids(model::spec_for("small", "en"), "en");
        assert_eq!(for_small[0], "parakeet-110m");
        assert!(!for_small.contains(&"small.en"));
    }

    #[test]
    fn another_language_is_never_handed_to_an_english_model() {
        let chinese = ids(model::spec_for("base", "zh"), "zh");
        assert_eq!(chinese, ["tiny", "small"]);
        let french = ids(model::spec_for("base", "fr"), "fr");
        assert_eq!(french, ["tiny", "small", "parakeet-0.6b-v3"]);
    }
}
