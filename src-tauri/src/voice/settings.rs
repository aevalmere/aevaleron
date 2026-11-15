//! Voice settings, kept under `voice` in aevaleron.json:
//!
//! ```json
//! "voice": { "dictation": true, "hotkey": "Ctrl+Super", "aiCleanup": false,
//!            "language": "en", "engine": "parakeet-110m",
//!            "enginePicked": false, "device": "" }
//! ```
//!
//! Anything missing or unreadable reads as its default. Settings and the
//! IPC call AI cleanup `cleanup`; it is stored as `aiCleanup` because the
//! `cleanup` earlier builds stored was their default (on), never a choice,
//! and cleanup is now off unless asked for (entries/voice-speed.md).
//!
//! `enginePicked` says the person chose the engine. Builds before it stored
//! their default, `whisper-base`, whether or not it was chosen, so a stored
//! `whisper-base` without the mark reads as no choice and gets today's
//! default; any other engine without the mark was a choice and is kept.

use serde_json::{json, Value};

use super::ptt;

pub const KEY: &str = "voice";

/// Transcription engines, the default first: Parakeet and Whisper on this
/// PC, or Gemini with the person's own key.
pub const ENGINES: [&str; 6] = [
    PARAKEET,
    "whisper-base",
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

/// The default before Parakeet (see the module comment).
const OLD_DEFAULT: &str = "whisper-base";

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
        (Some(e), None) => e != OLD_DEFAULT,
    };
    VoiceSettings {
        dictation: flag("dictation", d.dictation),
        hotkey,
        cleanup: flag("aiCleanup", d.cleanup),
        language,
        engine: engine.filter(|_| picked).unwrap_or(d.engine),
        picked,
        device: text("device").unwrap_or_default(),
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
        assert_eq!((s.engine.as_str(), s.picked), (PARAKEET, false));
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
        assert_eq!(s.whisper_size(), None);
    }

    #[test]
    fn the_old_default_is_not_a_choice_unless_marked() {
        let read = |v: Value| {
            let s = from_value(Some(&v));
            (s.engine, s.picked)
        };
        let fresh = (PARAKEET.to_string(), false);
        // What every earlier build stored, chosen or not.
        assert_eq!(read(json!({ "engine": "whisper-base" })), fresh);
        assert_eq!(read(json!({})), fresh);
        // Any other engine had to be chosen.
        let tiny = ("whisper-tiny".to_string(), true);
        assert_eq!(read(json!({ "engine": "whisper-tiny" })), tiny);
        let gemini = read(json!({ "engine": "gemini" }));
        assert_eq!(gemini, ("gemini".to_string(), true));
        // Chosen in this build, the old default is kept, and so is the mark.
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
        assert_eq!(from_value(Some(&cur.stored())), cur);
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
        assert_eq!(VoiceSettings::default().engine, PARAKEET);
    }
}
