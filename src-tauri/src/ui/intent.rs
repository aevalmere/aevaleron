//! What the person means by what they typed: Ask or Do. A port of
//! renderer/intent.js, rule for rule and score for score.
//!
//! A clear order is a task and Enter starts it, through the same path a
//! leading `>` takes, so the executor's safety gate decides what needs a
//! yes. A clear question is an answer. Anything in between is asked, and
//! the answer offers "Do this instead" (`offer_do`).

use std::sync::OnceLock;

use regex::Regex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    Ask,
    Do,
}

impl Intent {
    pub fn name(self) -> &'static str {
        match self {
            Intent::Ask => "Ask",
            Intent::Do => "Do",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Intent::Ask => "ask",
            Intent::Do => "do",
        }
    }

    pub fn prefix(self) -> char {
        match self {
            Intent::Ask => '?',
            Intent::Do => '>',
        }
    }

    pub fn other(self) -> Intent {
        match self {
            Intent::Ask => Intent::Do,
            Intent::Do => Intent::Ask,
        }
    }
}

const DO_VERBS: &str = "open|launch|start|close|quit|click|double-click|right-click|press|type|rename|move|paste|save|send|reply|forward|delete|remove|erase|create|make a new|new folder|fill in|fill out|fill|download|upload|install|uninstall|scroll|select all|select|highlight|check|uncheck|tick|untick|enable|disable|turn on|turn off|switch to|switch|go to|jump to|navigate to|search for|search|log in|login|sign in|sign out|log out|attach|book|schedule|sort|drag|minimi[sz]e|maximi[sz]e|resize|set|change|add|insert|empty|archive|submit|post|print|run|play|pause|mute|unmute|zip|unzip|extract|convert|export|import|share|pin|unpin|bookmark|refresh|reload|maximise|restart|shut down|lock|order|buy|pay|transfer|accept|decline|approve|merge|deploy|format|reset|sign";

struct Rules {
    ask_start: Regex,
    how_to: Regex,
    do_start: Regex,
    do_follow: Regex,
    polite: Regex,
    for_me: Regex,
    chain: Regex,
    text_only: Regex,
    risky: Regex,
    visual: Regex,
    asks: Regex,
}

fn rules() -> &'static Rules {
    static R: OnceLock<Rules> = OnceLock::new();
    R.get_or_init(|| {
        let re = |s: &str| Regex::new(s).expect("intent rule");
        Rules {
            ask_start: re(r"(?i)^(what|what's|whats|why|how|who|whom|whose|when|which|where|where's|wheres|is|are|was|were|does|did|can|could|should|would|will|has|have|explain|summari[sz]e|translate|tell me|meaning|define|describe|compare|any idea|i don'?t|i'?m|im|is there|are there|whats? up with|find|locate|look for|show me where)\b"),
            how_to: re(r"(?i)^(how (do|can|should|would) (i|you|we)|how to|is it possible to|what'?s the (best )?way to|can i|should i)\b"),
            do_start: re(&format!(
                r"(?i)^(please |pls |can you |could you |would you |go ahead and |now |then )?({DO_VERBS})\b"
            )),
            do_follow: re(r"(?i)^(do it|do that|do this|go ahead|yes,? do (it|that)|carry (it|that) out|make it so)\b"),
            polite: re(r"(?i)^(can|could|would) you\b"),
            for_me: re(r"(?i)\b(for me|on my computer|on my behalf)\b"),
            chain: re(r"(?i)\b(and then|then)\b|->"),
            text_only: re(r"(?i)^(write|draft|compose|rewrite|reword|proofread|translate|summari[sz]e|explain|list|give me|suggest|generate)\b"),
            risky: re(r"(?i)\b(send|delete|remove|erase|pay|buy|order|purchase|submit|post|share|install|uninstall|format|reset|transfer|sign|reply|forward|merge|deploy|empty|wipe|publish)\b"),
            visual: re(r"(?i)\b(which (button|icon|one|tab|option)|where (is|are|do)|colou?r|icon|button|look(s)? like|see|picture|image|photo|chart|graph|layout|design|logo|font|screenshot|highlighted|red|green|blue)\b"),
            asks: re(r"\?\s*$"),
        }
    })
}

/// A task scores at least this, and beats the question by `DO_MARGIN`,
/// before Enter starts it.
const DO_FLOOR: f64 = 0.6;
const DO_MARGIN: f64 = 0.15;
/// Below that, a line with this much of a task in it is asked, and the
/// answer offers to do it instead.
const OFFER_DO: f64 = 0.2;

#[derive(Clone, Debug, PartialEq)]
pub struct Score {
    pub intent: Intent,
    pub ask: f64,
    pub run: f64,
    pub offer_do: bool,
    pub risky: bool,
    pub visual: bool,
}

/// A typed line's prefix lock and the text after it: `?` asks, `>` does.
pub fn parse(raw: &str) -> (Option<Intent>, &str) {
    if let Some(rest) = raw.strip_prefix('?') {
        return (Some(Intent::Ask), rest.trim_start());
    }
    if raw.starts_with('>') {
        return (Some(Intent::Do), raw.trim_start_matches('>').trim_start());
    }
    (None, raw)
}

fn round(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// What an unlocked line most likely means.
pub fn score(text: &str) -> Score {
    let r = rules();
    let t = text.trim();
    let mut ask = 0.3;
    let mut run = 0.0;
    if !t.is_empty() {
        let n = t.split_whitespace().count();
        let asks = r.asks.is_match(t);
        let do_start = r.do_start.is_match(t);
        let how_to = r.how_to.is_match(t);
        if asks {
            ask += 0.6;
        }
        if r.ask_start.is_match(t) && !(r.polite.is_match(t) && do_start) {
            ask += 0.5;
        }
        if how_to {
            ask += 0.3;
        }
        let follow = r.do_follow.is_match(t);
        if do_start && !how_to {
            // A lone verb ("save") is ambiguous; an order has an object.
            run += if n >= 2 { 0.65 } else { 0.2 };
            if asks {
                run -= 0.3;
            }
        }
        if follow {
            run += 0.75;
        }
        if run > 0.0 && r.for_me.is_match(t) {
            run += 0.15;
        }
        if run > 0.0 && r.chain.is_match(t) {
            run += 0.1;
        }
        if r.text_only.is_match(t) && !do_start {
            ask += 0.3;
        }
        if n >= 6 && !do_start && !follow {
            ask += 0.15;
        }
    }
    let (ask, run) = (round(ask), round(run));
    // Unrounded, as intent.js compares it.
    let clear_do = run >= DO_FLOOR && run - ask >= DO_MARGIN;
    Score {
        intent: if clear_do { Intent::Do } else { Intent::Ask },
        ask,
        run,
        offer_do: !clear_do && run >= OFFER_DO,
        risky: r.risky.is_match(t),
        visual: r.visual.is_match(t),
    }
}

/// parse + score: the whole decision for one typed line.
pub fn classify(raw: &str) -> Score {
    let (lock, body) = parse(raw);
    let mut s = score(body);
    if let Some(l) = lock {
        s.intent = l;
        s.offer_do = false;
    }
    s
}

/// Starting points under an empty bar, picked from the app in front.
pub fn examples(process: &str, title: &str) -> [&'static str; 3] {
    match app_kind(process, title) {
        "browser" => [
            "Summarize this page",
            "Where is the sign in button?",
            "Open a new tab and search for the weather",
        ],
        "excel" => [
            "What is the total of this column?",
            "Explain this formula",
            "Sort this sheet by date",
        ],
        "mail" => [
            "Draft a reply to this email",
            "What does this email want from me?",
            "Archive this email",
        ],
        "editor" => [
            "What does this error mean?",
            "Explain this function",
            "Save all open files",
        ],
        "terminal" => [
            "Why did this command fail?",
            "What does this output mean?",
            "Open a new terminal tab",
        ],
        "explorer" => [
            "Which of these files is the newest?",
            "Rename these files by date",
            "Make a new folder called old",
        ],
        "document" => [
            "Summarize this document",
            "Fix the grammar in this paragraph",
            "Save this as a PDF",
        ],
        _ => [
            "What's on my screen?",
            "Where is the settings button?",
            "Open Notepad and write a haiku",
        ],
    }
}

pub fn app_kind(process: &str, title: &str) -> &'static str {
    let p = process.to_lowercase();
    let p = p.strip_suffix(".exe").unwrap_or(&p);
    let t = title.to_lowercase();
    let mail_title = t
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| w == "inbox" || w == "mail");
    match p {
        "chrome" | "msedge" | "firefox" | "brave" | "opera" | "vivaldi" | "arc" => "browser",
        "excel" => "excel",
        "outlook" | "olk" | "thunderbird" | "hxoutlook" => "mail",
        _ if mail_title => "mail",
        "code" | "devenv" | "idea64" | "pycharm64" | "sublime_text" | "notepad++" | "cursor"
        | "rider64" | "webstorm64" => "editor",
        "windowsterminal" | "cmd" | "powershell" | "pwsh" | "conhost" | "wt" | "mintty"
        | "alacritty" => "terminal",
        "explorer" => "explorer",
        "winword" | "notepad" | "wordpad" | "acrord32" | "acrobat" | "sumatrapdf" | "onenote"
        | "obsidian" | "notion" => "document",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn questions_ask_and_orders_do() {
        assert_eq!(classify("what is this error?").intent, Intent::Ask);
        assert_eq!(classify("how do I save this").intent, Intent::Ask);
        assert_eq!(classify("open notepad and type hello").intent, Intent::Do);
        assert_eq!(classify("rename these files by date").intent, Intent::Do);
        assert_eq!(classify("write me a haiku").intent, Intent::Ask);
        assert_eq!(classify("").intent, Intent::Ask);
    }

    #[test]
    fn a_lone_verb_is_asked_and_offered() {
        let s = classify("save");
        assert_eq!(s.intent, Intent::Ask);
        assert!(s.offer_do);
    }

    #[test]
    fn prefixes_lock() {
        assert_eq!(parse("?open it"), (Some(Intent::Ask), "open it"));
        assert_eq!(parse(">> what is it"), (Some(Intent::Do), "what is it"));
        assert_eq!(classify(">what is it").intent, Intent::Do);
        assert!(!classify("?save").offer_do);
    }

    #[test]
    fn examples_follow_the_app() {
        assert_eq!(app_kind("chrome.exe", ""), "browser");
        assert_eq!(app_kind("foo", "Inbox - Mail"), "mail");
        assert_eq!(examples("EXCEL.EXE", "")[2], "Sort this sheet by date");
        assert_eq!(app_kind("", ""), "other");
    }
}
