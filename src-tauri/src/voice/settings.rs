//! Voice settings, kept under `voice` in aevaleron.json:
//!
//! ```json
//! "voice": { "dictation": true, "hotkey": "Ctrl+Super", "aiCleanup": false,
//!            "language": "en", "engine": "whisper-base",
//!            "enginePicked": false, "device": "",
//!            "wake": false, "wakeWord": "Aevaleron" }
//! ```
//!
//! Anything missing or unreadable reads as its default. Settings and the
//! IPC call AI cleanup `cleanup`; it is stored as `aiCleanup` because the
//! `cleanup` earlier builds stored was their default (on), never a choice,
//! and cleanup is now off unless asked for (entries/voice-speed.md).
//!
//! `enginePicked` says the person chose the engine. Whisper base is the
//! default (lanes/wake.md). Builds before the mark stored their default,
//! `whisper-base`, whether or not it was chosen; builds of the Parakeet
//! default stored `parakeet-110m` with the mark false. So an engine marked
//! false, and either old default without a mark, reads as no choice and gets
//! today's default; an engine marked true, or any other engine without a
//! mark (it was never a default), was a choice and is kept. Nothing flips
//! back: the default is written with the mark false and read as the default.
//!
//! `wake` turns listening for the name on (off until the person turns it
//! on); `wakeWord` is the name (`voice::wake`).

use serde_json::{json, Value};

use super::ptt;

pub const KEY: &str = "voice";

/// Transcription engines, the default first: Whisper and Parakeet on this
/// PC, or Gemini with the person's own key.
pub const ENGINES: [&str; 6] = [
    WHISPER_BASE,
    PARAKEET,
    "whisper-tiny",
    "whisper-small",
    PARAKEET_V3,
    "gemini",
];

/// Parakeet 110M (parakeet-tdt_ctc-110m): several times quicker than
/// Whisper, English only, and its punctuation is plainer.
pub const PARAKEET: &str = "parakeet-110m";

/// Parakeet 0.6B (parakeet-tdt-0.6b-v3): the fullest text of the engines on
/// this PC, in English and 24 other European languages, and the slowest.
pub const PARAKEET_V3: &str = "parakeet-0.6b";

/// Whisper base, the default engine.
pub const WHISPER_BASE: &str = "whisper-base";

/// What earlier builds stored whether or not it was chosen (see the module
/// comment): Whisper base before the mark, Parakeet 110M with it false.
const OLD_DEFAULTS: [&str; 2] = [WHISPER_BASE, PARAKEET];

/// The name that starts listening until the person picks another.
pub const WAKE_WORD: &str = "Aevaleron";

/// Languages Settings offers (Whisper codes); `auto` detects.
pub const LANGUAGES: [(&str, &str); 14] = [
    ("en", "English"),
    ("auto", "Detect automatically"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("de", "German"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("tr", "Turkish"),
    ("hi", "Hindi"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("zh", "Chinese"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceSettings {
    /// The hold-to-talk chord works in every app.
    pub dictation: bool,
    pub hotkey: String,
    /// AI cleanup by a free model, which waits for the network. Off, the
    /// words go in as heard with hesitations taken out.
    pub cleanup: bool,
    pub language: String,
    pub engine: String,
    /// The person chose `engine`; otherwise it is the default.
    pub picked: bool,
    /// An input device's name; empty is the Windows default.
    pub device: String,
    /// Listening for the name: the microphone stays open, and saying
    /// `wake_word` starts an input that stops by itself (`voice::wake`).
    pub wake: bool,
    pub wake_word: String,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        VoiceSettings {
            dictation: true,
            hotkey: ptt::DEFAULT.into(),
            cleanup: false,
            language: "en".into(),
            engine: ENGINES[0].into(),
            picked: false,
            device: String::new(),
            wake: false,
            wake_word: WAKE_WORD.into(),
        }
    }
}

impl VoiceSettings {
    /// What is kept in the store (see the module comment).
    pub fn stored(&self) -> Value {
        let mut v = self.to_json();
        if let Some(o) = v.as_object_mut() {
            o.remove("cleanup");
            o.insert("aiCleanup".into(), json!(self.cleanup));
            o.insert("enginePicked".into(), json!(self.picked));
        }
        v
    }

    /// Chooses `engine`, as the person does in Settings.
    pub fn pick(&mut self, engine: &str) {
        self.engine = engine.into();
        self.picked = true;
    }

    /// What Settings is sent.
    pub fn to_json(&self) -> Value {
        json!({
            "dictation": self.dictation,
            "hotkey": self.hotkey,
            "cleanup": self.cleanup,
            "language": self.language,
            "engine": self.engine,
            "device": self.device,
            "wake": self.wake,
            "wakeWord": self.wake_word,
        })
    }

    /// The Whisper size for a `whisper-*` engine.
    pub fn whisper_size(&self) -> Option<&str> {
        self.engine.strip_prefix("whisper-")
    }
}

/// Reads one stored object, field by field; a bad field is its default.
pub fn from_value(v: Option<&Value>) -> VoiceSettings {
    let d = VoiceSettings::default();
    let Some(v) = v.filter(|v| v.is_object()) else {
        return d;
    };
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let flag = |k: &str, def: bool| v.get(k).and_then(Value::as_bool).unwrap_or(def);
    let hotkey = text("hotkey")
        .and_then(|h| ptt::parse(&h).ok().map(|c| ptt::accelerator(&c)))
        .unwrap_or(d.hotkey);
    let language = text("language")
        .filter(|l| LANGUAGES.iter().any(|(id, _)| id == l))
        .unwrap_or(d.language);
    let engine = text("engine").filter(|e| ENGINES.contains(&e.as_str()));
    let marked = v.get("enginePicked").and_then(Value::as_bool);
    let picked = match (&engine, marked) {
        (None, _) => false,
        (Some(_), Some(m)) => m,
        (Some(e), None) => !OLD_DEFAULTS.contains(&e.as_str()),
    };
    let wake_word = text("wakeWord")
        .and_then(|w| super::keyword::check_word(&w).ok())
        .unwrap_or(d.wake_word);
    VoiceSettings {
        dictation: flag("dictation", d.dictation),
        hotkey,
        cleanup: flag("aiCleanup", d.cleanup),
        language,
        engine: engine.filter(|_| picked).unwrap_or(d.engine),
        picked,
        device: text("device").unwrap_or_default(),
        wake: flag("wake", d.wake),
        wake_word,
    }
}

pub fn get() -> VoiceSettings {
    from_value(crate::store::get().raw(KEY).as_ref())
}

/// `cur` with the fields of `patch` applied, or a sentence saying why not.
/// `taken` lists the app's other shortcuts, which the chord may not equal.
pub fn merge(
    cur: &VoiceSettings,
    patch: &Value,
    taken: &[String],
) -> Result<VoiceSettings, String> {
    let mut next = cur.clone();
    if let Some(b) = patch.get("dictation").and_then(Value::as_bool) {
        next.dictation = b;
    }
    if let Some(b) = patch.get("cleanup").and_then(Value::as_bool) {
        next.cleanup = b;
    }
    if let Some(h) = patch.get("hotkey").and_then(Value::as_str) {
        let chord = ptt::parse(h)?;
        let clash = taken
            .iter()
            .any(|t| ptt::parse(t).is_ok_and(|o| o == chord));
        if clash {
            return Err(format!(
                "{} already opens Aevaleron. Choose another.",
                ptt::label(h)
            ));
        }
        next.hotkey = ptt::accelerator(&chord);
    }
    if let Some(l) = patch.get("language").and_then(Value::as_str) {
        if !LANGUAGES.iter().any(|(id, _)| *id == l) {
            return Err(format!("{l} is not a language voice input knows."));
        }
        next.language = l.into();
    }
    if let Some(e) = patch.get("engine").and_then(Value::as_str) {
        if !ENGINES.contains(&e) {
            return Err(format!("{e} is not a transcription engine."));
        }
        next.pick(e);
    }
    if let Some(d) = patch.get("device").and_then(Value::as_str) {
        next.device = d.into();
    }
    if let Some(b) = patch.get("wake").and_then(Value::as_bool) {
        next.wake = b;
    }
    if let Some(w) = patch.get("wakeWord").and_then(Value::as_str) {
        next.wake_word = super::keyword::check_word(w)?;
    }
    // Turning dictation on with a chord an app shortcut took since.
    let turned_on = patch.get("dictation").and_then(Value::as_bool) == Some(true);
    if turned_on && patch.get("hotkey").is_none() {
        if let Ok(chord) = ptt::parse(&next.hotkey) {
            if taken
                .iter()
                .any(|t| ptt::parse(t).is_ok_and(|o| o == chord))
            {
                return Err(format!(
                    "{} already opens Aevaleron. Choose another dictation shortcut.",
                    ptt::label(&next.hotkey)
                ));
            }
        }
    }
    Ok(next)
}

pub fn save(s: &VoiceSettings) {
    crate::store::get().set_raw(KEY, s.stored());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_bad_fields_read_as_defaults() {
        assert_eq!(from_value(None), VoiceSettings::default());
        assert_eq!(from_value(Some(&json!("x"))), VoiceSettings::default());
        let s = from_value(Some(
            &json!({ "hotkey": "win+ctrl", "engine": "nope", "language": "fr", "aiCleanup": 1 }),
        ));
        assert_eq!(s.hotkey, "Ctrl+Super");
        assert_eq!((s.engine.as_str(), s.picked), (WHISPER_BASE, false));
        assert!(!s.wake, "listening for the name is off until turned on");
        assert_eq!(s.wake_word, "Aevaleron");
        assert_eq!(s.language, "fr");
        assert!(!s.cleanup, "a number is not a switch");
        assert_eq!(from_value(Some(&s.stored())), s);
        // An earlier build's stored default is not a choice to clean up.
        assert!(!from_value(Some(&json!({ "cleanup": true }))).cleanup);
        let on = VoiceSettings {
            cleanup: true,
            ..VoiceSettings::default()
        };
        assert!(from_value(Some(&on.stored())).cleanup);
        assert_eq!(on.to_json()["cleanup"], json!(true));
        assert_eq!(s.whisper_size(), Some("base"));
    }

    #[test]
    fn an_old_default_is_not_a_choice_unless_marked() {
        let read = |v: Value| {
            let s = from_value(Some(&v));
            (s.engine, s.picked)
        };
        let fresh = (WHISPER_BASE.to_string(), false);
        // What builds before the mark stored, chosen or not.
        assert_eq!(read(json!({ "engine": "whisper-base" })), fresh);
        assert_eq!(read(json!({})), fresh);
        // What the Parakeet builds stored for the default: it moves.
        let parakeet_default = json!({ "engine": PARAKEET, "enginePicked": false });
        assert_eq!(read(parakeet_default), fresh);
        assert_eq!(read(json!({ "engine": PARAKEET })), fresh);
        // Parakeet chosen in Settings stays.
        let chosen = json!({ "engine": PARAKEET, "enginePicked": true });
        assert_eq!(read(chosen), (PARAKEET.to_string(), true));
        // Any other engine without a mark had to be chosen.
        let tiny = ("whisper-tiny".to_string(), true);
        assert_eq!(read(json!({ "engine": "whisper-tiny" })), tiny);
        let gemini = read(json!({ "engine": "gemini" }));
        assert_eq!(gemini, ("gemini".to_string(), true));
        // Chosen in this build, Whisper base is kept, and so is the mark.
        let taken: Vec<String> = Vec::new();
        let cur = VoiceSettings::default();
        let base = merge(&cur, &json!({ "engine": "whisper-base" }), &taken).unwrap();
        assert!(base.picked);
        let back = from_value(Some(&base.stored()));
        let kept = (back.engine.as_str(), back.picked);
        assert_eq!(kept, ("whisper-base", true));
        assert_eq!(back.whisper_size(), Some("base"));
        // A default that was never chosen follows the default.
        let unmarked = json!({ "engine": "whisper-small", "enginePicked": false });
        assert_eq!(read(unmarked), fresh);
        // Nothing flips back: the default read, written and read again.
        assert_eq!(from_value(Some(&cur.stored())), cur);
        let moved = from_value(Some(&json!({ "engine": PARAKEET, "enginePicked": false })));
        let again = from_value(Some(&moved.stored()));
        assert_eq!((again.engine.as_str(), again.picked), (WHISPER_BASE, false));
    }

    #[test]
    fn the_name_is_checked_before_it_is_kept() {
        let taken: Vec<String> = Vec::new();
        let cur = VoiceSettings::default();
        let on = merge(
            &cur,
            &json!({ "wake": true, "wakeWord": "  Jarvis " }),
            &taken,
        )
        .unwrap();
        assert!(on.wake);
        assert_eq!(on.wake_word, "Jarvis");
        let back = from_value(Some(&on.stored()));
        assert_eq!((back.wake, back.wake_word.as_str()), (true, "Jarvis"));
        assert!(merge(&cur, &json!({ "wakeWord": "x" }), &taken).is_err());
        assert!(merge(&cur, &json!({ "wakeWord": "hey" }), &taken).is_err());
        assert!(merge(&cur, &json!({ "wakeWord": "r2d2!" }), &taken).is_err());
        // A stored name that does not pass reads as the default.
        let bad = from_value(Some(&json!({ "wake": true, "wakeWord": "" })));
        assert_eq!((bad.wake, bad.wake_word.as_str()), (true, WAKE_WORD));
    }

    #[test]
    fn a_patch_is_checked_before_it_is_kept() {
        let cur = VoiceSettings::default();
        let taken = vec!["Ctrl+Alt+Space".to_string()];
        let next = merge(
            &cur,
            &json!({ "hotkey": "alt+shift+d", "cleanup": false }),
            &taken,
        )
        .unwrap();
        assert_eq!((next.hotkey.as_str(), next.cleanup), ("Alt+Shift+D", false));
        assert!(merge(&cur, &json!({ "hotkey": "Control+Alt+Space" }), &taken).is_err());
        assert!(merge(&cur, &json!({ "hotkey": "Ctrl" }), &taken).is_err());
        assert!(merge(&cur, &json!({ "language": "xx" }), &taken).is_err());
        assert!(merge(&cur, &json!({ "engine": "gemini" }), &taken).is_ok());
        let parakeet = merge(&cur, &json!({ "engine": PARAKEET }), &taken).unwrap();
        assert_eq!(parakeet.whisper_size(), None);
        assert_eq!(from_value(Some(&parakeet.stored())).engine, PARAKEET);
        let big = merge(&cur, &json!({ "engine": PARAKEET_V3 }), &taken).unwrap();
        assert_eq!((big.engine.as_str(), big.picked), (PARAKEET_V3, true));
        assert_eq!(VoiceSettings::default().engine, WHISPER_BASE);
        assert_eq!(ENGINES[0], WHISPER_BASE);
    }
}
