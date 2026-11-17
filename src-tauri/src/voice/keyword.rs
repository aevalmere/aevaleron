//! The name that starts listening (`voice::wake`): whether what was heard
//! starts with it, and the line that is left once it is taken off.
//!
//! Whisper writes a made-up name in many ways ("Aevaleron", "Avaleron",
//! "Evaleron", "a valeron", "Ava Lauren"), so a name is compared by a sound
//! key, not by its letters: case, punctuation and spaces go, every run of
//! vowels is one vowel, a doubled letter counts once, and c, k and q are one
//! sound, as are z and s, ph and f. Two keys match when they are a short
//! edit apart and have the same consonants, at most one of them different.
//! The consonants are what makes a name: "Avalon" is two edits from
//! "Aevaleron" but has no r, so it does not start listening. That is on
//! purpose; it is a common word in films and songs (lanes/wake.md).
//!
//! The name must come first. Up to two words like "hey" or "okay" may come
//! before it; anywhere else in a sentence it does not count.

/// Words that may come before the name and are not part of the line.
const FILLERS: [&str; 15] = [
    "hey", "hi", "hello", "ok", "okay", "oh", "um", "umm", "uh", "uhm", "erm", "so", "yo",
    "alright", "right",
];

/// At most this many of them before the name.
const MAX_FILLERS: usize = 2;

/// What a check of the start of an utterance found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The name, then `rest` (empty when only the name was heard so far).
    Yes { rest: String },
    /// Words that are not the name: drop the utterance.
    No,
    /// Nothing yet but "hey" or "okay", or nothing at all: listen a little
    /// longer before deciding.
    Unsure,
}

struct Token {
    /// Lower case, letters and digits only.
    word: String,
    /// Where it starts in the text, in bytes.
    start: usize,
}

fn tokens(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let push = |s: usize, e: usize, out: &mut Vec<Token>| {
        let word: String = text[s..e]
            .chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect();
        if !word.is_empty() {
            out.push(Token { word, start: s });
        }
    };
    for (i, c) in text.char_indices() {
        let part = c.is_alphanumeric() || c == '\'' || c == '\u{2019}';
        match (part, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                push(s, i, &mut out);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        push(s, text.len(), &mut out);
    }
    out
}

fn is_filler(word: &str) -> bool {
    FILLERS.contains(&word)
}

/// The sound key of a word (see the module comment).
fn key(word: &str) -> String {
    let chars: Vec<char> = word
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let next = chars.get(i + 1).copied();
        let sound = match chars[i] {
            'a' | 'e' | 'i' | 'o' | 'u' | 'y' => 'a',
            'c' | 'k' | 'q' => 'k',
            'z' => 's',
            'p' if next == Some('h') => {
                i += 1;
                'f'
            }
            other => other,
        };
        if !out.ends_with(sound) {
            out.push(sound);
        }
        i += 1;
    }
    out
}

/// The consonants of a key.
fn skeleton(key: &str) -> Vec<char> {
    key.chars().filter(|c| *c != 'a').collect()
}

/// Edits (insert, delete, change) between two sequences.
fn distance<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut diag = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let up = row[j + 1];
            row[j + 1] = if x == y {
                diag
            } else {
                1 + diag.min(up).min(row[j])
            };
            diag = up;
        }
    }
    row[b.len()]
}

/// The edits a key of this many sounds may be off by.
fn allowed(sounds: usize) -> usize {
    match sounds {
        0..=4 => 0,
        5..=7 => 1,
        _ => 2,
    }
}

/// A name, ready to compare.
struct Name {
    key: Vec<char>,
    consonants: Vec<char>,
    words: usize,
}

impl Name {
    fn new(name: &str) -> Name {
        let k = key(name);
        Name {
            consonants: skeleton(&k),
            key: k.chars().collect(),
            words: tokens(name).len().max(1),
        }
    }

    /// Whether `heard`, one or more words run together, sounds like it.
    /// `loose` allows one more edit and one consonant more or less: for a
    /// line already known to start with the name.
    fn heard_in(&self, heard: &str, loose: bool) -> bool {
        let k = key(heard);
        let consonants = skeleton(&k);
        let k: Vec<char> = k.chars().collect();
        let slack = usize::from(loose);
        if distance(&k, &self.key) > allowed(self.key.len()) + slack {
            return false;
        }
        let own = self.consonants.len();
        let off = distance(&consonants, &self.consonants);
        let may = usize::from(own > 2);
        if loose {
            consonants.len().abs_diff(own) <= 1 && off <= may + 1
        } else {
            consonants.len() == own && off <= may
        }
    }

    /// The index of the first token after the name, when the name opens
    /// `toks` (after at most [`MAX_FILLERS`] fillers).
    fn ends_at(&self, toks: &[Token], loose: bool) -> Option<usize> {
        for skip in 0..=MAX_FILLERS {
            if skip > 0 && !toks.get(skip - 1).is_some_and(|t| is_filler(&t.word)) {
                break;
            }
            let mut joined = String::new();
            for n in 0..self.words + 2 {
                let Some(t) = toks.get(skip + n) else { break };
                joined.push_str(&t.word);
                if joined.chars().count() > 2 * self.key.len() + 4 {
                    break;
                }
                if self.heard_in(&joined, loose) {
                    return Some(skip + n + 1);
                }
            }
        }
        None
    }
}

/// Whether `text`, the start of an utterance, opens with `name`.
pub fn verdict(text: &str, name: &str) -> Verdict {
    let toks = tokens(text);
    let n = Name::new(name);
    if let Some(end) = n.ends_at(&toks, false) {
        let rest = toks.get(end).map_or("", |t| &text[t.start..]);
        return Verdict::Yes {
            rest: rest.trim().to_string(),
        };
    }
    if toks.iter().all(|t| is_filler(&t.word)) {
        return Verdict::Unsure;
    }
    Verdict::No
}

/// The line with the name taken off the front, with any "hey" before it:
/// "Hey Aevaleron, what time is it?" gives "What time is it?". The check
/// already heard the name, so here it is looked for loosely too. Text that
/// does not open with it comes back as it was.
pub fn strip(text: &str, name: &str) -> String {
    let toks = tokens(text);
    let n = Name::new(name);
    let end = n.ends_at(&toks, false).or_else(|| n.ends_at(&toks, true));
    let rest = match end {
        Some(e) => toks.get(e).map_or("", |t| &text[t.start..]),
        None => text,
    };
    let rest = rest.trim();
    let mut chars = rest.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// A name the person typed, tidied, or why it cannot be used.
pub fn check_word(word: &str) -> Result<String, String> {
    let w = word.split_whitespace().collect::<Vec<_>>().join(" ");
    let len = w.chars().count();
    if !(2..=32).contains(&len) {
        return Err("Use a name of 2 to 32 letters.".into());
    }
    let letters = |c: char| c.is_alphabetic() || c == ' ' || c == '-' || c == '\'';
    if !w.chars().all(letters) {
        return Err("Use letters only, for example Aevaleron.".into());
    }
    let toks = tokens(&w);
    if toks.len() > 3 {
        return Err("Use a name of one to three words.".into());
    }
    if toks.iter().all(|t| is_filler(&t.word)) {
        return Err("Pick a name, not a word like hey or okay.".into());
    }
    if key(&w).chars().count() < 3 {
        return Err("Use a longer name. A short one is said by accident too often.".into());
    }
    Ok(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAME: &str = "Aevaleron";

    fn yes(text: &str) -> bool {
        matches!(verdict(text, NAME), Verdict::Yes { .. })
    }

    #[test]
    fn the_ways_whisper_writes_the_name_all_count() {
        for heard in [
            "Aevaleron, what is on my screen?",
            "AEVALERON!",
            "Avaleron what time is it",
            "Evaleron.",
            "Aveleron, open my mail.",
            "Avaleren, hi",
            "Evaluron, what's this?",
            "Ivaleron.",
            "A valeron, open my mail.",
            "Ava Lauren, what's the weather?",
            "Ava-leron.",
            "Hey Aevaleron, hi.",
            "Okay, Avaleron, read this.",
            "Hey, okay, Aevaleron.",
            "Valeron, what is this?",
        ] {
            assert!(yes(heard), "{heard:?} should start listening");
        }
    }

    #[test]
    fn near_words_and_the_name_later_on_do_not() {
        for heard in [
            // Avalon has no r: a real word, kept out on purpose.
            "Avalon.",
            "Avalon is an island in the old stories.",
            "Everyone, sit down.",
            "Valerie, come here.",
            "Aileron trim is set.",
            "The elevator is broken.",
            "I said Aevaleron yesterday.",
            "Hey hey hey Aevaleron.",
            "So I think we should meet at three.",
            "Thank you.",
        ] {
            assert_eq!(verdict(heard, NAME), Verdict::No, "{heard:?}");
        }
    }

    #[test]
    fn only_a_hey_so_far_waits_for_more() {
        assert_eq!(verdict("", NAME), Verdict::Unsure);
        assert_eq!(verdict("Hey.", NAME), Verdict::Unsure);
        assert_eq!(verdict("Um, okay.", NAME), Verdict::Unsure);
    }

    #[test]
    fn what_follows_the_name_is_the_rest() {
        let rest = |t: &str| match verdict(t, NAME) {
            Verdict::Yes { rest } => rest,
            v => panic!("{t:?}: {v:?}"),
        };
        assert_eq!(rest("Aevaleron, what is on my screen?"), "what is on my screen?");
        assert_eq!(rest("Hey Avaleron."), "");
        assert_eq!(rest("A valeron open Notepad"), "open Notepad");
    }

    #[test]
    fn another_name_works_the_same_way() {
        assert!(matches!(verdict("Jarvis, lights.", "Jarvis"), Verdict::Yes { .. }));
        assert!(matches!(verdict("Hey Jarvas.", "Jarvis"), Verdict::Yes { .. }));
        assert_eq!(verdict("Travis, lights.", "Jarvis"), Verdict::No);
        // A short name must be heard as it is, but for its vowels.
        assert!(matches!(verdict("Max, hi.", "Max"), Verdict::Yes { .. }));
        assert_eq!(verdict("Mat, hi.", "Max"), Verdict::No);
        // Two words, the first a filler of its own.
        assert!(matches!(verdict("Hey buddy, hi.", "Hey Buddy"), Verdict::Yes { .. }));
    }

    #[test]
    fn the_name_comes_off_the_line() {
        let s = |t: &str| strip(t, NAME);
        assert_eq!(s("Aevaleron, what is on my screen?"), "What is on my screen?");
        assert_eq!(s("Hey Avaleron. What time is it?"), "What time is it?");
        // Loosely: the check heard the name, the final pass wrote it so.
        assert_eq!(s("Avalon, what time is it"), "What time is it");
        assert_eq!(s("Aevaleron."), "");
        assert_eq!(s("Nothing like it here."), "Nothing like it here.");
    }

    #[test]
    fn a_typed_name_is_checked() {
        assert_eq!(check_word("  Aevaleron "), Ok("Aevaleron".to_string()));
        assert_eq!(check_word("Hey  Buddy"), Ok("Hey Buddy".to_string()));
        assert!(check_word("x").is_err());
        assert!(check_word("hey").is_err());
        assert!(check_word("okay hey").is_err());
        assert!(check_word("r2d2!").is_err());
        assert!(check_word("one two three four").is_err());
        assert!(check_word("Al").is_err());
        assert!(check_word(&"a".repeat(40)).is_err());
    }

    #[test]
    fn keys_sound_alike() {
        assert_eq!(key("Aevaleron"), "avalaran");
        assert_eq!(key("Avellaron"), "avalaran");
        assert_eq!(key("Phil"), "fal");
        assert_eq!(distance(&[1, 2, 3], &[1, 3]), 1);
        assert_eq!(distance::<u8>(&[], &[1, 2]), 2);
    }
}
