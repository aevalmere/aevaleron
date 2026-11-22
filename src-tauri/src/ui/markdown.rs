//! Markdown for model answers, as lines the output page draws with Slint
//! elements (renderer/markdown.js's rules, without the HTML). Headings,
//! paragraphs that join their soft-wrapped lines, lists that survive blank
//! lines and nest, quotes, rules, tables and fenced code.
//!
//! Model output is untrusted, and nothing here makes markup: every line is
//! plain text with its markers dropped. Slint's Text has no inline runs, so
//! emphasis and code spans lose their markers, a link keeps its label inline
//! (with its host when the label does not say it) and is listed in `links`
//! for the page to show as a chip that opens through `open-external`.
//!
//! Everything is a pure function of the whole text, so a streaming answer is
//! parsed again on each update; an open fence shows what has arrived.

use std::sync::OnceLock;

use regex::{Captures, Regex};

macro_rules! re {
    ($src:expr) => {{
        static R: OnceLock<Regex> = OnceLock::new();
        R.get_or_init(|| Regex::new($src).expect("a valid pattern"))
    }};
}

/// Text with its inline markers dropped.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inline {
    pub text: String,
    pub links: Vec<Link>,
    /// The whole text was bold (`**...**`).
    pub strong: bool,
    /// The whole text was one code span.
    pub mono: bool,
}

/// A link: its label as shown, where it goes, and the host shown beside
/// the label ("" when the label already is the address).
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub label: String,
    pub url: String,
    pub host: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

impl Align {
    pub fn as_str(self) -> &'static str {
        match self {
            Align::None => "",
            Align::Left => "left",
            Align::Center => "center",
            Align::Right => "right",
        }
    }
}

/// One drawn block. `depth` is the list nesting a block sits in (0 outside
/// any list).
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    /// Levels 1 to 3 (`#`, `##`, `###` and below).
    Heading {
        level: usize,
        text: Inline,
    },
    Para {
        depth: usize,
        text: Inline,
    },
    /// A list item's first paragraph, with its marker ("•", "3.").
    Item {
        depth: usize,
        marker: String,
        text: Inline,
    },
    Quote(Inline),
    Rule,
    Code {
        depth: usize,
        lang: String,
        text: String,
    },
    Table {
        head: Vec<Inline>,
        aligns: Vec<Align>,
        rows: Vec<Vec<Inline>>,
    },
}

// ------------------------------------------------------------------ inline

/// The host `url` goes to, port included, or "".
fn host_of(url: &str) -> String {
    let Ok(u) = url::Url::parse(url) else {
        return String::new();
    };
    let Some(h) = u.host_str() else {
        return String::new();
    };
    match u.port() {
        Some(p) => format!("{h}:{p}"),
        None => h.to_string(),
    }
}

/// What a label or address says, for telling whether a label is its own
/// address (markdown.js `bare`).
fn bare(s: &str) -> String {
    let cut: String = s.chars().filter(|c| !"*_`~".contains(*c)).collect();
    let low = cut.trim().to_lowercase();
    let low = low
        .strip_prefix("https://")
        .or_else(|| low.strip_prefix("http://"))
        .unwrap_or(&low);
    let low = low.strip_prefix("www.").unwrap_or(low);
    low.trim_end_matches('/').to_string()
}

/// Bold, italic and strike-through markers dropped, the words kept.
fn strip_emphasis(s: &str) -> String {
    let mut out = re!(r"\*\*\*([^*\n]+)\*\*\*")
        .replace_all(s, "${1}")
        .into_owned();
    out = re!(r"\*\*([^*\n]+)\*\*")
        .replace_all(&out, "${1}")
        .into_owned();
    out = re!(r"__([^_\n]+)__").replace_all(&out, "${1}").into_owned();
    out = re!(r"(^|[\s(])\*([^*\n]+)\*")
        .replace_all(&out, "${1}${2}")
        .into_owned();
    // markdown.js looks ahead for the character after `_x_`; this takes it
    // and puts it back, so two spans that share a space take two passes.
    for _ in 0..4 {
        let next = re!(r"(^|[\s(])_([^_\n]+)_($|[\s).,;:!?])")
            .replace_all(&out, "${1}${2}${3}")
            .into_owned();
        if next == out {
            break;
        }
        out = next;
    }
    re!(r"~~([^~\n]+)~~").replace_all(&out, "${1}").into_owned()
}

fn restore(s: &str, tag: char, parts: &[String]) -> String {
    let pattern = match tag {
        'c' => re!(r"\x00c(\d+)\x00"),
        _ => re!(r"\x00l(\d+)\x00"),
    };
    pattern
        .replace_all(s, |c: &Captures| {
            c[1].parse::<usize>()
                .ok()
                .and_then(|i| parts.get(i))
                .cloned()
                .unwrap_or_default()
        })
        .into_owned()
}

/// The inline rules: code spans first, so nothing inside one is touched,
/// then links, then emphasis.
pub fn inline(text: &str) -> Inline {
    let text = text.replace('\0', "");
    let mono = re!(r"^`[^`\n]+`$").is_match(text.trim());
    let mut codes: Vec<String> = Vec::new();
    let coded = re!(r"`([^`\n]+)`")
        .replace_all(&text, |c: &Captures| {
            codes.push(c[1].to_string());
            format!("\0c{}\0", codes.len() - 1)
        })
        .into_owned();
    let mut links: Vec<Link> = Vec::new();
    let mut shown: Vec<String> = Vec::new();
    let linked = re!(r"\[([^\]\n]+)\]\((https?://[^)\s]+)\)")
        .replace_all(&coded, |c: &Captures| {
            let with_code = restore(&c[1], 'c', &codes);
            let label = strip_emphasis(&with_code);
            let url = c[2].to_string();
            let host = host_of(&url);
            let plain = bare(&with_code);
            let h = host.strip_prefix("www.").unwrap_or(&host).to_lowercase();
            let show = !host.is_empty() && plain != h && plain != bare(&url);
            let host = if show { host } else { String::new() };
            shown.push(if host.is_empty() {
                label.clone()
            } else {
                format!("{label} ({host})")
            });
            links.push(Link { label, url, host });
            format!("\0l{}\0", shown.len() - 1)
        })
        .into_owned();
    let whole = linked.trim();
    let strong = re!(r"^\*\*\*[^*\n]+\*\*\*$").is_match(whole)
        || re!(r"^\*\*[^*\n]+\*\*$").is_match(whole)
        || re!(r"^__[^_\n]+__$").is_match(whole);
    let out = strip_emphasis(&linked);
    let out = restore(&out, 'l', &shown);
    let out = restore(&out, 'c', &codes);
    Inline {
        text: out,
        links,
        strong,
        mono,
    }
}

// ------------------------------------------------------------------ tables

fn is_table_row(t: &str) -> bool {
    t.len() >= 2 && t.starts_with('|') && t.ends_with('|')
}

/// A row's cells; `\|` is a pipe inside a cell.
fn cells(line: &str) -> Vec<String> {
    let mut s = line.trim();
    s = s.strip_prefix('|').unwrap_or(s);
    if !s.ends_with("\\|") {
        s = s.strip_suffix('|').unwrap_or(s);
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev = '\0';
    for c in s.chars() {
        if c == '|' && prev != '\\' {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
        prev = c;
    }
    out.push(cur);
    out.into_iter()
        .map(|c| c.replace("\\|", "|").trim().to_string())
        .collect()
}

fn align_of(sep: &str) -> Align {
    let c: String = sep.chars().filter(|c| !c.is_whitespace()).collect();
    let dashes = |s: &str| !s.is_empty() && s.chars().all(|c| c == '-');
    if let Some(mid) = c.strip_prefix(':').and_then(|s| s.strip_suffix(':')) {
        if dashes(mid) {
            return Align::Center;
        }
    }
    if c.strip_suffix(':').is_some_and(dashes) {
        return Align::Right;
    }
    if c.strip_prefix(':').is_some_and(dashes) {
        return Align::Left;
    }
    Align::None
}

fn table(header: &str, sep: &str, rows: &[&str]) -> Line {
    let head: Vec<Inline> = cells(header).iter().map(|c| inline(c)).collect();
    let n = head.len();
    let mut aligns: Vec<Align> = cells(sep).iter().map(|c| align_of(c)).collect();
    aligns.resize(n, Align::None);
    let rows = rows
        .iter()
        .map(|r| {
            let cs = cells(r);
            (0..n)
                .map(|i| inline(cs.get(i).map_or("", String::as_str)))
                .collect()
        })
        .collect();
    Line::Table { head, aligns, rows }
}

// ------------------------------------------------------------------ blocks

/// A list as it is built: a blank line, a nested list or an indented code
/// block can still belong to the item above, so it is written out only when
/// it closes.
struct Node {
    ordered: bool,
    start: u64,
    level: usize,
    items: Vec<Vec<Part>>,
}

enum Part {
    Text(Vec<String>),
    List(usize),
    Code(String, String),
}

struct Fence {
    lang: String,
    lines: Vec<String>,
    indent: usize,
    fence: String,
    in_list: bool,
}

fn indent_of(s: &str) -> usize {
    s.chars()
        .take_while(|c| c.is_whitespace())
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

fn bullet(depth: usize) -> &'static str {
    match depth {
        0 => "\u{2022}",
        1 => "\u{25E6}",
        _ => "\u{25AA}",
    }
}

fn emit_list(arena: &[Node], at: usize, depth: usize, out: &mut Vec<Line>) {
    let node = &arena[at];
    for (i, item) in node.items.iter().enumerate() {
        let marker = if node.ordered {
            format!("{}.", node.start + i as u64)
        } else {
            bullet(depth).to_string()
        };
        let mut first = true;
        for part in item {
            match part {
                Part::Text(lines) => {
                    let text = inline(&lines.join(" "));
                    if first {
                        out.push(Line::Item {
                            depth,
                            marker: marker.clone(),
                            text,
                        });
                        first = false;
                    } else {
                        out.push(Line::Para {
                            depth: depth + 1,
                            text,
                        });
                    }
                }
                Part::List(j) => emit_list(arena, *j, depth + 1, out),
                Part::Code(lang, text) => out.push(Line::Code {
                    depth: depth + 1,
                    lang: lang.clone(),
                    text: text.clone(),
                }),
            }
        }
    }
}

/// `^\s{0,3}([-*_])(\s*\1){2,}\s*$`, which the regex crate cannot say.
fn is_rule(line: &str) -> bool {
    let lead = line.chars().take_while(|c| c.is_whitespace()).count();
    if lead > 3 {
        return false;
    }
    let rest = line.trim();
    let Some(mark) = rest.chars().next().filter(|c| "-*_".contains(*c)) else {
        return false;
    };
    rest.chars().all(|c| c == mark || c.is_whitespace())
        && rest.chars().filter(|c| *c == mark).count() >= 3
}

#[derive(Default)]
struct Parser {
    out: Vec<Line>,
    para: Vec<String>,
    arena: Vec<Node>,
    stack: Vec<usize>,
    pending_break: bool,
    code: Option<Fence>,
}

impl Parser {
    fn flush_para(&mut self) {
        if self.para.is_empty() {
            return;
        }
        let text = inline(&self.para.join(" "));
        self.out.push(Line::Para { depth: 0, text });
        self.para.clear();
    }

    fn close_lists(&mut self) {
        if let Some(&root) = self.stack.first() {
            emit_list(&self.arena, root, 0, &mut self.out);
        }
        self.stack.clear();
        self.arena.clear();
        self.pending_break = false;
    }

    fn item(&mut self) -> Option<&mut Vec<Part>> {
        let top = *self.stack.last()?;
        self.arena[top].items.last_mut()
    }

    fn add_text(&mut self, text: String, fresh: bool) {
        let Some(it) = self.item() else {
            return;
        };
        match it.last_mut() {
            Some(Part::Text(lines)) if !fresh => lines.push(text),
            _ => it.push(Part::Text(vec![text])),
        }
    }

    /// A blank line keeps the list open only when what follows still belongs
    /// to it: an item of the same type, a nested item, or indented text.
    fn continues_list(&self, line: &str) -> bool {
        let Some(&root) = self.stack.first() else {
            return false;
        };
        let root = &self.arena[root];
        if let Some(m) = item_re().captures(line) {
            let level = indent_of(&m[1]) / 2;
            if level > root.level {
                return true;
            }
            return m.get(3).is_some() == root.ordered;
        }
        indent_of(line) >= 2
    }

    fn end_code(&mut self) {
        let Some(c) = self.code.take() else {
            return;
        };
        let text = c.lines.join("\n");
        if c.in_list {
            if let Some(it) = self.item() {
                it.push(Part::Code(c.lang, text));
                return;
            }
        }
        self.out.push(Line::Code {
            depth: 0,
            lang: c.lang,
            text,
        });
    }

    fn list_item(&mut self, m: &Captures) {
        self.flush_para();
        let level = indent_of(&m[1]) / 2;
        let ordered = m.get(3).is_some();
        let start = m.get(3).and_then(|n| n.as_str().parse().ok()).unwrap_or(1);
        // Back out of lists nested deeper than this item; a root list
        // indented deeper than it ends.
        while self.stack.len() > 1 && self.top().is_some_and(|t| self.arena[t].level > level) {
            self.stack.pop();
        }
        if self.stack.len() == 1 && self.arena[self.stack[0]].level > level {
            self.close_lists();
        }
        // Same level, other type: that list ends and a new one starts.
        if let Some(t) = self.top() {
            if self.arena[t].level == level && self.arena[t].ordered != ordered {
                if self.stack.len() == 1 {
                    self.close_lists();
                } else {
                    self.stack.pop();
                }
            }
        }
        let node = Node {
            ordered,
            start,
            level,
            items: Vec::new(),
        };
        match self.top() {
            None => {
                self.arena.push(node);
                self.stack = vec![self.arena.len() - 1];
            }
            Some(t) if self.arena[t].level < level => {
                self.arena.push(node);
                let at = self.arena.len() - 1;
                if let Some(it) = self.arena[t].items.last_mut() {
                    it.push(Part::List(at));
                }
                self.stack.push(at);
            }
            Some(_) => {}
        }
        if let Some(t) = self.top() {
            self.arena[t]
                .items
                .push(vec![Part::Text(vec![m[4].to_string()])]);
        }
        self.pending_break = false;
    }

    fn top(&self) -> Option<usize> {
        self.stack.last().copied()
    }
}

fn item_re() -> &'static Regex {
    re!(r"^(\s*)([-*+]|(\d{1,9})[.)])\s+(.*)$")
}

fn fence_re() -> &'static Regex {
    re!(r"^(\s*)(`{3,}|~{3,})\s*([A-Za-z0-9_+#.-]*)")
}

fn quote_re() -> &'static Regex {
    re!(r"^\s{0,3}>\s?(.*)$")
}

/// The blocks of `raw`, in order, as the page draws them.
pub fn lines(raw: &str) -> Vec<Line> {
    let src = raw
        .replace('\0', "")
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let lines: Vec<&str> = src.split('\n').collect();
    let mut p = Parser::default();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;

        if let Some(code) = p.code.as_mut() {
            if let Some(close) = fence_re().captures(line) {
                let run = &close[2];
                let rest = line.trim().get(run.len()..).unwrap_or("");
                if run.starts_with(&code.fence[..1])
                    && run.len() >= code.fence.len()
                    && rest.is_empty()
                {
                    p.end_code();
                    continue;
                }
            }
            // Drop the fence's own indent from each line, never more.
            let k = line
                .bytes()
                .take(code.indent)
                .take_while(|b| *b == b' ')
                .count();
            code.lines.push(line[k..].to_string());
            continue;
        }

        if let Some(f) = fence_re().captures(line) {
            p.flush_para();
            let indent = indent_of(&f[1]);
            let in_list = !p.stack.is_empty() && indent >= 2;
            if !in_list {
                p.close_lists();
            }
            p.pending_break = false;
            p.code = Some(Fence {
                lang: f[3].to_string(),
                lines: Vec::new(),
                indent,
                fence: f[2].to_string(),
                in_list,
            });
            continue;
        }

        if line.trim().is_empty() {
            p.flush_para();
            if !p.stack.is_empty() {
                let next = lines[i..].iter().find(|l| !l.trim().is_empty());
                if next.is_some_and(|l| p.continues_list(l)) {
                    p.pending_break = true;
                } else {
                    p.close_lists();
                }
            }
            continue;
        }

        if let Some(h) = re!(r"^\s{0,3}(#{1,6})\s+(.*?)(?:\s+#+)?\s*$").captures(line) {
            p.flush_para();
            p.close_lists();
            p.out.push(Line::Heading {
                level: h[1].len().min(3),
                text: inline(&h[2]),
            });
            continue;
        }

        if is_rule(line) {
            p.flush_para();
            p.close_lists();
            p.out.push(Line::Rule);
            continue;
        }

        let next_sep = lines
            .get(i)
            .is_some_and(|n| re!(r"^\|?\s*:?-{3,}[\s|:-]*$").is_match(n.trim()));
        if is_table_row(line.trim()) && next_sep {
            p.flush_para();
            p.close_lists();
            let sep = lines[i];
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim().starts_with('|') {
                j += 1;
            }
            p.out.push(table(line, sep, &lines[i + 1..j]));
            i = j;
            continue;
        }

        if quote_re().is_match(line) {
            p.flush_para();
            p.close_lists();
            let mut paras: Vec<Vec<String>> = vec![Vec::new()];
            let mut j = i - 1;
            while let Some(m) = lines.get(j).and_then(|l| quote_re().captures(l)) {
                let text = m[1].to_string();
                if text.trim().is_empty() {
                    if paras.last().is_some_and(|p| !p.is_empty()) {
                        paras.push(Vec::new());
                    }
                } else if let Some(last) = paras.last_mut() {
                    last.push(text);
                }
                j += 1;
            }
            for para in paras.iter().filter(|p| !p.is_empty()) {
                p.out.push(Line::Quote(inline(&para.join(" "))));
            }
            i = j;
            continue;
        }

        if let Some(m) = item_re().captures(line) {
            p.list_item(&m);
            continue;
        }

        if !p.stack.is_empty() {
            // Text under an item: a lazy continuation of its line, or after a
            // blank line an indented paragraph of its own.
            if !p.pending_break || indent_of(line) >= 2 {
                let fresh = p.pending_break;
                p.add_text(line.trim().to_string(), fresh);
                p.pending_break = false;
                continue;
            }
            p.close_lists();
        }
        p.para.push(line.trim().to_string());
    }

    // Mid-stream: whatever has arrived so far is shown, open fence included.
    p.end_code();
    p.flush_para();
    p.close_lists();
    p.out
}

/// The text of a block as the transcript reads it (probes, copy).
pub fn plain(line: &Line) -> String {
    match line {
        Line::Heading { text, .. } | Line::Para { text, .. } | Line::Quote(text) => {
            text.text.clone()
        }
        Line::Item { marker, text, .. } => format!("{marker} {}", text.text),
        Line::Rule => String::new(),
        Line::Code { text, .. } => text.clone(),
        Line::Table { head, rows, .. } => {
            let row = |r: &[Inline]| {
                r.iter()
                    .map(|c| c.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" | ")
            };
            let mut out = vec![row(head)];
            out.extend(rows.iter().map(|r| row(r)));
            out.join("\n")
        }
    }
}

// ------------------------------------------------------------------- reads

/// "Answer ready. <first sentence>." (C-37): the answer's first sentence as
/// plain words, at most 160 characters.
pub fn first_sentence(raw: &str) -> String {
    let s = re!(r"(?s)```.*?(```|$)").replace_all(raw, " ");
    let s = re!(r"`([^`]*)`").replace_all(&s, "${1}");
    let s = re!(r"\[([^\]]*)\]\([^)]*\)").replace_all(&s, "${1}");
    let s = re!(r"(?m)^\s*(#{1,6}|>|\d+[.)]|[-*+])\s+").replace_all(&s, "");
    let s = re!(r"[*_~|#]").replace_all(&s, "");
    let s = re!(r"\s+").replace_all(&s, " ");
    let plain = s.trim();
    let mut out = re!(r"^(.+?[.!?])(\s|$)")
        .captures(plain)
        .map_or_else(|| plain.to_string(), |m| m[1].to_string());
    if out.chars().count() > 160 {
        let cut: String = out.chars().take(159).collect();
        let cut = re!(r"\s+\S*$").replace(&cut, "").into_owned();
        out = format!("{cut}\u{2026}");
    }
    out
}

/// An answer that reads as steps to take: at least two list lines that
/// start with an action (U39). Those get Do it.
pub fn reads_as_steps(raw: &str) -> bool {
    re!(r"(?im)^\s*(?:\d+[.)]|[-*])\s+(?:\*\*|__)?(?:click|double-click|right-click|press|open|type|select|choose|go to|drag|enter)\b")
        .find_iter(raw)
        .count()
        >= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(l: &Line) -> &str {
        match l {
            Line::Heading { text, .. }
            | Line::Para { text, .. }
            | Line::Item { text, .. }
            | Line::Quote(text) => &text.text,
            Line::Code { text, .. } => text,
            _ => "",
        }
    }

    #[test]
    fn headings_and_paragraphs() {
        let l = lines("# One\n## Two\n#### Four\n\nsoft\nwrapped line");
        assert_eq!(l.len(), 4);
        assert!(matches!(&l[0], Line::Heading { level: 1, .. }));
        assert!(matches!(&l[1], Line::Heading { level: 2, .. }));
        assert!(matches!(&l[2], Line::Heading { level: 3, .. }));
        assert_eq!(text(&l[3]), "soft wrapped line");
        assert_eq!(text(&lines("## Closed ##")[0]), "Closed");
        // No space after the marks: a paragraph.
        assert!(matches!(&lines("#tag")[0], Line::Para { .. }));
    }

    /// The smoke's markdownStructure: one ordered list across a blank line,
    /// the third item holding a nested list.
    #[test]
    fn lists_survive_blank_lines_and_nest() {
        let src = "1. first\n2. second\n\n3. third\n   - nested\n4. fourth";
        let l = lines(src);
        let items: Vec<(usize, String, String)> = l
            .iter()
            .filter_map(|x| match x {
                Line::Item {
                    depth,
                    marker,
                    text,
                } => Some((*depth, marker.clone(), text.text.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(l.len(), 5);
        assert_eq!(items[0], (0, "1.".into(), "first".into()));
        assert_eq!(items[2], (0, "3.".into(), "third".into()));
        assert_eq!(items[3], (1, bullet(1).to_string(), "nested".into()));
        assert_eq!(items[4], (0, "4.".into(), "fourth".into()));
    }

    #[test]
    fn lists_end_and_change_type() {
        let l = lines("- a\n- b\n\nafter");
        assert_eq!(l.len(), 3);
        assert!(matches!(&l[2], Line::Para { depth: 0, .. }));
        let l = lines("- a\n1. b");
        assert!(matches!(&l[1], Line::Item { marker, .. } if marker == "1."));
        // A numbered list keeps its start.
        let l = lines("3. c\n4. d");
        assert!(matches!(&l[0], Line::Item { marker, .. } if marker == "3."));
        assert!(matches!(&l[1], Line::Item { marker, .. } if marker == "4."));
        // Lazy continuation joins the item's line; an indented paragraph
        // after a blank line is its own.
        let l = lines("- one\ncontinued\n\n  more");
        assert_eq!(text(&l[0]), "one continued");
        assert!(matches!(&l[1], Line::Para { depth: 1, .. }));
    }

    #[test]
    fn code_fences() {
        let l = lines("Before\n```rust\nfn main() {}\n  x\n```\nafter");
        assert_eq!(l.len(), 3);
        assert_eq!(
            l[1],
            Line::Code {
                depth: 0,
                lang: "rust".into(),
                text: "fn main() {}\n  x".into(),
            }
        );
        // Mid-stream, an open fence shows what arrived.
        let l = lines("```\nhalf");
        assert_eq!(text(&l[0]), "half");
        // Nothing inside a fence is markdown.
        let l = lines("```\n# not a heading\n- nor a list\n```");
        assert_eq!(l.len(), 1);
        // A fence inside an item belongs to it.
        let l = lines("- step\n  ```\n  code\n  ```");
        assert!(matches!(&l[1], Line::Code { depth: 1, text, .. } if text == "code"));
    }

    #[test]
    fn tables_keep_their_alignment() {
        let l = lines("| A | B | C |\n|:---|---:|:-:|\n| x | y |\n\ntail");
        let Line::Table { head, aligns, rows } = &l[0] else {
            panic!("not a table: {l:?}");
        };
        assert_eq!(head.len(), 3);
        assert_eq!(
            aligns.iter().map(|a| a.as_str()).collect::<Vec<_>>(),
            ["left", "right", "center"]
        );
        assert_eq!(rows[0].len(), 3);
        assert_eq!(rows[0][2].text, "");
        assert_eq!(cells("| a \\| b | c |"), ["a | b", "c"]);
    }

    #[test]
    fn quotes_and_rules() {
        let l = lines("> one\n> two\n>\n> three\n\n---\n* * *");
        assert_eq!(
            l,
            vec![
                Line::Quote(inline("one two")),
                Line::Quote(inline("three")),
                Line::Rule,
                Line::Rule,
            ]
        );
        assert!(!is_rule("--"));
        assert!(!is_rule("    ---"));
    }

    #[test]
    fn inline_markers_drop_and_links_stay() {
        let i = inline("**bold** and *it* and `code` and ~~gone~~ and _em_.");
        assert_eq!(i.text, "bold and it and code and gone and em.");
        assert!(!i.strong);
        assert!(inline("**all bold**").strong);
        assert!(inline("`only code`").mono);
        // Code spans are left alone.
        assert_eq!(inline("`**x**`").text, "**x**");
        let i = inline("See [the book](https://doc.rust-lang.org/book/).");
        assert_eq!(i.text, "See the book (doc.rust-lang.org).");
        assert_eq!(i.links[0].url, "https://doc.rust-lang.org/book/");
        assert_eq!(i.links[0].host, "doc.rust-lang.org");
        // A label that is the address says no host.
        let i = inline("[example.com](https://www.example.com/)");
        assert_eq!(i.text, "example.com");
        assert_eq!(i.links[0].host, "");
        // Only http and https are links.
        let i = inline("[x](javascript:alert(1))");
        assert!(i.links.is_empty());
        // Two underscored spans sharing a space.
        assert_eq!(inline("_a_ _b_").text, "a b");
        // Snake case is not emphasis.
        assert_eq!(inline("snake_case_name").text, "snake_case_name");
    }

    #[test]
    fn markup_is_never_made() {
        let l = lines("<img src=x onerror=\"alert(1)\">");
        assert_eq!(text(&l[0]), "<img src=x onerror=\"alert(1)\">");
        assert_eq!(inline("a\0c0\0b").text, "ac0b");
    }

    #[test]
    fn sentences_and_steps() {
        assert_eq!(
            first_sentence("## Fix\n\nRun `cargo build`. Then wait."),
            "Fix Run cargo build."
        );
        let long = "word ".repeat(60);
        let s = first_sentence(&long);
        assert!(s.ends_with('\u{2026}') && s.chars().count() <= 160);
        assert!(reads_as_steps("1. Click Start\n2. Type notepad\n"));
        assert!(!reads_as_steps("1. Click Start\nThen wait."));
    }
}
