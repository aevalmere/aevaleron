//! Turns the pieces of text read inside a screen rectangle into the text the
//! clipboard gets. A piece is a string and the rectangle it was read from: a
//! line or an element from UI Automation, or a word from OCR. Pure functions;
//! `mod.rs` does the reading.
//!
//! The rules, in the order they run:
//! 1. Only pieces that lie inside the rectangle count (half their area).
//! 2. An element and the smaller elements inside it that repeat its text are
//!    one text, not two.
//! 3. Pieces that share a row make one line, left to right; lines run top to
//!    bottom.
//! 4. A step between two lines that is half as large again as the usual step
//!    is kept as one empty line.
//! 5. A word broken over two lines with a hyphen is joined, when the break
//!    is clearly a wrap.
//! 6. No line ends in a space, and no empty line leads or trails.

/// One piece of text and where it was read, in physical pixels. The
/// rectangle the person drew is a piece with no text.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Piece {
    pub fn new(text: &str, x: f64, y: f64, width: f64, height: f64) -> Piece {
        Piece {
            text: text.to_string(),
            x,
            y,
            width,
            height,
        }
    }

    fn area(&self) -> f64 {
        self.width.max(0.0) * self.height.max(0.0)
    }

    fn right(&self) -> f64 {
        self.x + self.width.max(0.0)
    }

    fn bottom(&self) -> f64 {
        self.y + self.height.max(0.0)
    }

    /// The area this piece shares with `o`.
    fn overlap(&self, o: &Piece) -> f64 {
        let w = self.right().min(o.right()) - self.x.max(o.x);
        let h = self.bottom().min(o.bottom()) - self.y.max(o.y);
        if w > 0.0 && h > 0.0 {
            w * h
        } else {
            0.0
        }
    }
}

/// Where the pieces came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// UI Automation: the application's own text, by line or by element.
    /// Elements nest, so a text can come twice (rule 2).
    Exact,
    /// OCR words, each with the box of its letters.
    Ocr,
}

/// A step between two lines this many times the usual step holds an empty
/// line.
const EMPTY_LINE_STEP: f64 = 1.5;
/// Two lines alone have no usual step: they hold an empty line when they
/// are this many line heights apart, middle to middle.
const TWO_LINE_STEP: f64 = 2.5;
/// A hyphenated line must end within this many line heights of the widest
/// line's right edge to count as wrapped.
const WRAP_REACH: f64 = 2.0;
/// Words that follow a hanging hyphen ("pre- and post-"), not a broken word.
const NOT_A_TAIL: [&str; 6] = ["and", "or", "to", "und", "oder", "et"];

/// The text inside `clip`, from `pieces`.
pub fn text_in(pieces: Vec<Piece>, clip: &Piece, source: Source) -> String {
    let inside: Vec<Piece> = pieces
        .into_iter()
        .filter(|p| lies_inside(p, clip))
        .collect();
    text_of(inside, source)
}

/// The text of `pieces`, all of which count.
pub fn text_of(pieces: Vec<Piece>, source: Source) -> String {
    let pieces: Vec<Piece> = pieces.into_iter().map(cleaned).collect();
    let pieces = match source {
        Source::Exact => drop_repeats(pieces),
        Source::Ocr => pieces,
    };
    let mut lines = rows(stacked(pieces));
    let mut gaps = empty_after(&lines);
    join_hyphens(&mut lines, &mut gaps);
    lay_out(&lines, &gaps)
}

/// Rule 1. At least half of the piece is inside `clip`; a piece with no
/// area counts by its top left corner.
pub fn lies_inside(p: &Piece, clip: &Piece) -> bool {
    if p.area() <= 0.0 {
        return p.x >= clip.x && p.x < clip.right() && p.y >= clip.y && p.y < clip.bottom();
    }
    p.overlap(clip) * 2.0 >= p.area()
}

/// Characters no clipboard wants: the object placeholder UI Automation
/// writes for a picture in the text, NUL, and carriage returns (line breaks
/// are `\n` until the text leaves).
fn cleaned(mut p: Piece) -> Piece {
    let junk = |c: char| c == '\u{FFFC}' || c == '\0' || c == '\r';
    if p.text.contains(junk) {
        p.text = p.text.replace("\r\n", "\n").replace(junk, "");
    }
    p
}

fn dense(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Rule 2. UI Automation names a list row "Inbox 3" and also the two labels
/// inside it. When the smaller pieces inside a piece hold all of its text,
/// the pieces stay (their places are finer); when they hold only part of
/// it, the larger piece stays and they go.
fn drop_repeats(pieces: Vec<Piece>) -> Vec<Piece> {
    let texts: Vec<String> = pieces.iter().map(|p| dense(&p.text)).collect();
    let sizes: Vec<usize> = texts.iter().map(|t| t.chars().count()).collect();
    let mut order: Vec<usize> = (0..pieces.len()).collect();
    order.sort_by(|a, b| pieces[*b].area().total_cmp(&pieces[*a].area()));
    let mut gone = vec![false; pieces.len()];
    for (at, &outer) in order.iter().enumerate() {
        if gone[outer] || sizes[outer] == 0 {
            continue;
        }
        let mut within = Vec::new();
        for &inner in &order[at + 1..] {
            let p = &pieces[inner];
            let inside = p.area() > 0.0 && p.overlap(&pieces[outer]) * 1.25 >= p.area();
            let repeats = sizes[inner] > 0 && texts[outer].contains(texts[inner].as_str());
            if !gone[inner] && inside && repeats {
                within.push(inner);
            }
        }
        if within.is_empty() {
            continue;
        }
        let held: usize = within.iter().map(|&i| sizes[i]).sum();
        if held >= sizes[outer] {
            gone[outer] = true;
        } else {
            for i in within {
                gone[i] = true;
            }
        }
    }
    pieces
        .into_iter()
        .zip(gone)
        .filter(|(_, g)| !g)
        .map(|(p, _)| p)
        .collect()
}

/// A piece whose text has line breaks (an edit box read whole) becomes one
/// piece per line, stacked evenly down its rectangle. An empty line keeps
/// its place, so the gap it leaves is still there to see.
fn stacked(pieces: Vec<Piece>) -> Vec<Piece> {
    let mut out = Vec::with_capacity(pieces.len());
    for p in pieces {
        if !p.text.contains('\n') {
            if !p.text.trim().is_empty() {
                out.push(p);
            }
            continue;
        }
        let body = p.text.trim_matches('\n');
        let count = body.split('\n').count().max(1) as f64;
        let step = p.height.max(0.0) / count;
        for (i, line) in body.split('\n').enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            out.push(Piece {
                text: line.to_string(),
                x: p.x,
                y: p.y + step * i as f64,
                width: p.width,
                height: step,
            });
        }
    }
    out
}

/// One visual line: its text and the band it sits in.
#[derive(Clone, Debug, PartialEq)]
struct Line {
    text: String,
    top: f64,
    bottom: f64,
    right: f64,
}

impl Line {
    fn height(&self) -> f64 {
        (self.bottom - self.top).max(1.0)
    }

    fn middle(&self) -> f64 {
        (self.top + self.bottom) / 2.0
    }
}

struct Row {
    /// The band of the piece that opened the row.
    top: f64,
    bottom: f64,
    pieces: Vec<Piece>,
}

impl Row {
    /// A piece shares the row when half its height, or half the row's, is
    /// inside the row's band.
    fn holds(&self, p: &Piece) -> bool {
        let shared = self.bottom.min(p.bottom()) - self.top.max(p.y);
        let least = (self.bottom - self.top).min(p.height).max(1.0);
        shared * 2.0 >= least
    }
}

/// Rule 3: reading order from rectangles.
fn rows(mut pieces: Vec<Piece>) -> Vec<Line> {
    pieces.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
    let mut rows: Vec<Row> = Vec::new();
    for p in pieces {
        // The lowest row it shares, so a tall piece higher up does not
        // take it.
        match rows.iter().rposition(|r| r.holds(&p)) {
            Some(i) => rows[i].pieces.push(p),
            None => rows.push(Row {
                top: p.y,
                bottom: p.bottom(),
                pieces: vec![p],
            }),
        }
    }
    let mut lines: Vec<Line> = rows.into_iter().map(line_of).collect();
    lines.sort_by(|a, b| a.middle().total_cmp(&b.middle()));
    lines
}

fn line_of(mut row: Row) -> Line {
    row.pieces.sort_by(|a, b| a.x.total_cmp(&b.x));
    let alone = row.pieces.len() == 1;
    let mut text = String::new();
    for p in &row.pieces {
        // A line that is one piece keeps its indent; pieces side by side are
        // words and labels, joined by one space.
        let part = if alone {
            p.text.trim_end()
        } else {
            p.text.trim()
        };
        if part.is_empty() {
            continue;
        }
        let last = text.chars().next_back();
        let first = part.chars().next();
        let tight = matches!((last, first), (Some(a), Some(b)) if wide(a) && wide(b));
        if !text.is_empty() && !tight {
            text.push(' ');
        }
        text.push_str(part);
    }
    let tops = row.pieces.iter().map(|p| p.y);
    let bottoms = row.pieces.iter().map(Piece::bottom);
    let rights = row.pieces.iter().map(Piece::right);
    Line {
        text,
        top: tops.fold(f64::INFINITY, f64::min),
        bottom: bottoms.fold(f64::NEG_INFINITY, f64::max),
        right: rights.fold(f64::NEG_INFINITY, f64::max),
    }
}

/// Chinese and Japanese characters, which are written without spaces. OCR
/// reads each as a word of its own.
fn wide(c: char) -> bool {
    // Punctuation and kana, the two ideograph blocks, the compatibility
    // ideographs, and the full-width forms.
    const BLOCKS: [(u32, u32); 5] = [
        (0x3000, 0x30FF),
        (0x3400, 0x4DBF),
        (0x4E00, 0x9FFF),
        (0xF900, 0xFAFF),
        (0xFF00, 0xFFEF),
    ];
    let code = c as u32;
    BLOCKS.iter().any(|(lo, hi)| (*lo..=*hi).contains(&code))
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// Rule 4: for each line, whether an empty line follows it. The usual step
/// is the smallest one between two lines here (a step under half a line's
/// height is two rows that should have been one). Evenly spaced rows,
/// however far apart, hold no empty lines.
fn empty_after(lines: &[Line]) -> Vec<bool> {
    let steps: Vec<f64> = lines
        .windows(2)
        .map(|w| w[1].middle() - w[0].middle())
        .collect();
    let height = median(lines.iter().map(Line::height).collect());
    let usual = steps
        .iter()
        .copied()
        .filter(|s| *s >= 0.5 * height)
        .fold(f64::INFINITY, f64::min);
    let mut out = Vec::with_capacity(lines.len());
    for (i, step) in steps.iter().enumerate() {
        let empty = if steps.len() >= 2 && usual.is_finite() {
            *step >= EMPTY_LINE_STEP * usual
        } else {
            let tall = lines[i].height().max(lines[i + 1].height());
            *step >= TWO_LINE_STEP * tall
        };
        out.push(empty);
    }
    if !lines.is_empty() {
        out.push(false);
    }
    out
}

/// The last word of a line when it is the head of a broken word: letters
/// only, a capital first at most, and the hyphen that broke it. The soft
/// hyphen is only ever a break.
fn broken_head(line: &str) -> Option<(usize, &str)> {
    let at = line
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    let word = &line[at..];
    let mark = word.chars().next_back()?;
    if !matches!(mark, '-' | '\u{2010}' | '\u{00AD}') {
        return None;
    }
    let head = &word[..word.len() - mark.len_utf8()];
    let letters = head.chars().count();
    let plain = head.chars().all(char::is_alphabetic);
    let one_capital = head.chars().skip(1).all(|c| !c.is_uppercase());
    (letters >= 2 && plain && one_capital).then_some((at, head))
}

/// The first word of a line when it can be the tail of a broken word: it
/// starts with two or more small letters and is not a joining word.
fn broken_tail(line: &str) -> Option<&str> {
    let word = line.split_whitespace().next()?;
    let letters: String = word.chars().take_while(|c| c.is_alphabetic()).collect();
    let small = letters.chars().count() >= 2 && letters.chars().all(char::is_lowercase);
    (small && !NOT_A_TAIL.contains(&letters.as_str())).then_some(word)
}

/// Rule 5. "exam-" at the end of a line that reaches the right edge, with
/// "ple," opening the line right under it, is "example,": the tail moves up
/// and the rest of the next line stays where it is.
fn join_hyphens(lines: &mut Vec<Line>, gaps: &mut Vec<bool>) {
    let reach = lines.iter().map(|l| l.right).fold(f64::MIN, f64::max);
    let mut i = 0;
    while i + 1 < lines.len() {
        let near_edge = lines[i].right >= reach - WRAP_REACH * lines[i].height();
        let head = broken_head(lines[i].text.trim_end());
        let head = head.map(|(at, h)| (at, h.to_string()));
        let tail = broken_tail(&lines[i + 1].text).map(str::to_string);
        let (Some((at, head)), Some(tail)) = (head, tail) else {
            i += 1;
            continue;
        };
        if !near_edge || gaps[i] {
            i += 1;
            continue;
        }
        let next = lines[i + 1].text.trim_start();
        let rest = next[tail.len()..].trim_start().to_string();
        lines[i].text.truncate(at);
        lines[i].text.push_str(&head);
        lines[i].text.push_str(&tail);
        if rest.is_empty() {
            // The next line was only the tail: it goes, and what came after
            // it now comes after this line.
            lines.remove(i + 1);
            gaps.remove(i);
            continue;
        }
        lines[i + 1].text = rest;
        i += 1;
    }
}

/// Rule 6: the lines with their empty lines, as one string with `\n`
/// between lines.
fn lay_out(lines: &[Line], gaps: &[bool]) -> String {
    let mut out: Vec<&str> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let text = line.text.trim_end();
        if text.trim().is_empty() {
            continue;
        }
        if i > 0 && gaps[i - 1] && !out.is_empty() {
            out.push("");
        }
        out.push(text);
    }
    out.join("\n")
}

/// How much was copied, for the confirmation: `Copied 42 words`, or
/// characters for text written without spaces.
pub fn count_label(text: &str) -> String {
    let solid: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    let wide_ones = solid.iter().filter(|c| wide(**c)).count();
    let (n, unit) = if !solid.is_empty() && wide_ones * 2 >= solid.len() {
        (solid.len(), "character")
    } else {
        (text.split_whitespace().count(), "word")
    };
    let s = if n == 1 { "" } else { "s" };
    format!("Copied {n} {unit}{s}")
}

/// The first few words, for the confirmation: six words or 40 characters,
/// whichever is less, with `…` when there is more.
pub fn lead(text: &str) -> String {
    const WORDS: usize = 6;
    const CHARS: usize = 40;
    let mut out = String::new();
    let mut taken = 0;
    let mut more = false;
    for word in text.split_whitespace() {
        let longer = out.chars().count() + word.chars().count() + 1;
        if taken == WORDS || (taken > 0 && longer > CHARS) {
            more = true;
            break;
        }
        if taken > 0 {
            out.push(' ');
        }
        out.push_str(word);
        taken += 1;
    }
    if out.chars().count() > CHARS {
        out = out.chars().take(CHARS).collect();
        more = true;
    }
    if more {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str, x: f64, y: f64, w: f64, h: f64) -> Piece {
        Piece::new(text, x, y, w, h)
    }

    /// OCR words on rows 24 px apart, 14 px tall, 10 px a letter.
    fn words(rows: &[&str]) -> Vec<Piece> {
        let mut out = Vec::new();
        for (r, row) in rows.iter().enumerate() {
            let mut x = 20.0;
            for w in row.split(' ') {
                let width = 10.0 * w.chars().count() as f64;
                out.push(p(w, x, 100.0 + 24.0 * r as f64, width, 14.0));
                x += width + 8.0;
            }
        }
        out
    }

    #[test]
    fn reading_order_comes_from_the_rectangles() {
        // Handed over in no order, and with the wobble OCR boxes have.
        let pieces = vec![
            p("due", 80.0, 141.0, 30.0, 12.0),
            p("Invoice", 20.0, 100.0, 70.0, 14.0),
            p("42.00", 118.0, 139.0, 50.0, 14.0),
            p("March", 190.0, 101.0, 50.0, 13.0),
            p("total", 20.0, 140.0, 50.0, 13.0),
            p("2291", 98.0, 102.0, 40.0, 12.0),
            p("for", 146.0, 100.0, 30.0, 14.0),
            p("settle", 88.0, 120.0, 60.0, 14.0),
            p("Please", 20.0, 121.0, 60.0, 14.0),
        ];
        assert_eq!(
            text_of(pieces, Source::Ocr),
            "Invoice 2291 for March\nPlease settle\ntotal due 42.00"
        );
    }

    #[test]
    fn a_small_mark_stays_on_its_row() {
        // A full stop's box is a few pixels at the foot of the line.
        let pieces = vec![
            p("end", 20.0, 100.0, 30.0, 14.0),
            p(".", 52.0, 111.0, 3.0, 3.0),
            p("Next", 20.0, 124.0, 40.0, 14.0),
        ];
        assert_eq!(text_of(pieces, Source::Ocr), "end .\nNext");
    }

    #[test]
    fn only_pieces_inside_the_rectangle_count() {
        let area = p("", 0.0, 0.0, 200.0, 100.0);
        let pieces = vec![
            p("inside", 10.0, 10.0, 60.0, 16.0),
            // More than half inside: counts.
            p("mostly", 150.0, 40.0, 80.0, 16.0),
            // A third inside: does not.
            p("barely", 170.0, 70.0, 90.0, 16.0),
            // The window's own name, far larger than the rectangle.
            p("whole window", -400.0, -300.0, 1600.0, 900.0),
            p("below", 10.0, 140.0, 60.0, 16.0),
        ];
        assert_eq!(text_in(pieces, &area, Source::Exact), "inside\nmostly");
        // A piece with no size counts by where it starts.
        assert!(lies_inside(&p("x", 5.0, 5.0, 0.0, 0.0), &area));
        assert!(!lies_inside(&p("x", 205.0, 5.0, 0.0, 0.0), &area));
    }

    #[test]
    fn lines_keep_their_breaks_and_lose_trailing_spaces() {
        let pieces = vec![
            p("Invoice 2291 for March  ", 20.0, 100.0, 220.0, 18.0),
            p("Please settle by Friday\t", 20.0, 118.0, 220.0, 18.0),
            p("total due 42.00 ", 20.0, 136.0, 150.0, 18.0),
        ];
        assert_eq!(
            text_of(pieces, Source::Exact),
            "Invoice 2291 for March\nPlease settle by Friday\ntotal due 42.00"
        );
        // One piece on its line keeps its indent.
        let code = vec![
            p("if ready {", 20.0, 100.0, 100.0, 18.0),
            p("    go();", 20.0, 118.0, 100.0, 18.0),
            p("}", 20.0, 136.0, 100.0, 18.0),
        ];
        assert_eq!(text_of(code, Source::Exact), "if ready {\n    go();\n}");
    }

    #[test]
    fn an_empty_line_is_kept_as_one() {
        // A skipped line doubles the step to the next line.
        let exact = vec![
            p("Dear Sam,", 20.0, 100.0, 90.0, 18.0),
            p("The report is ready.", 20.0, 136.0, 200.0, 18.0),
            p("It is two pages.", 20.0, 154.0, 160.0, 18.0),
            // Three lines skipped: still one empty line.
            p("Regards", 20.0, 226.0, 70.0, 18.0),
        ];
        assert_eq!(
            text_of(exact, Source::Exact),
            "Dear Sam,\n\nThe report is ready.\nIt is two pages.\n\nRegards"
        );
        // OCR words 14 px tall on rows 24 px apart are single spaced; only
        // the 48 px step holds an empty line.
        let mut spaced = words(&["Dear Sam", "The report", "is ready"]);
        for w in spaced.iter_mut().skip(2) {
            w.y += 24.0;
        }
        assert_eq!(
            text_of(spaced, Source::Ocr),
            "Dear Sam\n\nThe report\nis ready"
        );
        // Rows that are all far apart are a list, not empty lines.
        let list = vec![
            p("Inbox", 20.0, 100.0, 50.0, 16.0),
            p("Sent", 20.0, 140.0, 40.0, 16.0),
            p("Drafts", 20.0, 180.0, 60.0, 16.0),
        ];
        assert_eq!(text_of(list, Source::Exact), "Inbox\nSent\nDrafts");
        // Two lines alone: an empty line only when they are far apart.
        let two = |y: f64| {
            let pair = vec![
                p("Name", 20.0, 100.0, 50.0, 16.0),
                p("Address", 20.0, y, 70.0, 16.0),
            ];
            text_of(pair, Source::Exact)
        };
        assert_eq!(two(130.0), "Name\nAddress");
        assert_eq!(two(150.0), "Name\n\nAddress");
    }

    #[test]
    fn a_word_broken_by_a_wrap_is_joined() {
        let pieces = words(&["this is a clear exam-", "ple, of a wrapped word"]);
        assert_eq!(
            text_of(pieces, Source::Ocr),
            "this is a clear example,\nof a wrapped word"
        );
        // The same from exact lines.
        let lines = vec![
            p("The quarterly num-", 20.0, 100.0, 200.0, 18.0),
            p("bers are in.", 20.0, 118.0, 120.0, 18.0),
        ];
        assert_eq!(
            text_of(lines, Source::Exact),
            "The quarterly numbers\nare in."
        );
        // A soft hyphen is only ever a break.
        let soft = vec![
            p("an unbeliev\u{00AD}", 20.0, 100.0, 200.0, 18.0),
            p("able day", 20.0, 118.0, 80.0, 18.0),
        ];
        assert_eq!(text_of(soft, Source::Exact), "an unbelievable\nday");
        // A line that was only the tail goes; the next line moves up.
        let only_tail = vec![
            p("one long inter-", 20.0, 100.0, 200.0, 18.0),
            p("national", 20.0, 118.0, 80.0, 18.0),
            p("call", 20.0, 136.0, 40.0, 18.0),
        ];
        assert_eq!(
            text_of(only_tail, Source::Exact),
            "one long international\ncall"
        );
    }

    #[test]
    fn a_hyphen_that_is_not_a_wrap_stays() {
        let keep = |rows: &[&str]| text_of(words(rows), Source::Ocr);
        // A range and a pair of names: the next line starts with a digit or
        // a capital.
        assert_eq!(
            keep(&["Pages 10-", "20 are missing"]),
            "Pages 10-\n20 are missing"
        );
        assert_eq!(keep(&["North-", "South divide"]), "North-\nSouth divide");
        // A hanging hyphen before a joining word.
        assert_eq!(
            keep(&["both the pre-", "and post-match talk"]),
            "both the pre-\nand post-match talk"
        );
        // A dash on its own, an option, a word with a hyphen already.
        assert_eq!(
            keep(&["nothing here -", "at all then"]),
            "nothing here -\nat all then"
        );
        assert_eq!(
            keep(&["run with --dry-", "run to check"]),
            "run with --dry-\nrun to check"
        );
        assert_eq!(
            keep(&["a well-known-", "thing indeed"]),
            "a well-known-\nthing indeed"
        );
        // An acronym.
        assert_eq!(
            keep(&["sent over HTTP-", "based links"]),
            "sent over HTTP-\nbased links"
        );
        // The line stops well short of the right edge: a list, not a wrap.
        assert_eq!(
            keep(&["anti-", "social behaviour is a wide heading"]),
            "anti-\nsocial behaviour is a wide heading"
        );
        // An empty line between the two halves.
        let mut apart = words(&["this is a clear exam-", "ple, of a wrapped word"]);
        for w in apart.iter_mut().skip(5) {
            w.y += 48.0;
        }
        assert_eq!(
            text_of(apart, Source::Ocr),
            "this is a clear exam-\n\nple, of a wrapped word"
        );
    }

    #[test]
    fn nothing_readable_is_an_empty_string() {
        assert_eq!(text_of(Vec::new(), Source::Ocr), "");
        assert_eq!(text_of(Vec::new(), Source::Exact), "");
        let blank = vec![
            p("   ", 20.0, 100.0, 40.0, 16.0),
            p("\u{FFFC}", 20.0, 120.0, 40.0, 16.0),
            p("\r\n", 20.0, 140.0, 40.0, 16.0),
        ];
        assert_eq!(text_of(blank, Source::Exact), "");
        // Text there, but none of it inside the rectangle.
        let outside = vec![p("elsewhere", 500.0, 500.0, 90.0, 16.0)];
        let area = p("", 0.0, 0.0, 200.0, 100.0);
        assert_eq!(text_in(outside, &area, Source::Exact), "");
    }

    #[test]
    fn an_edit_box_read_whole_is_split_into_its_lines() {
        let text = "first line\r\nsecond line\r\n\r\nfourth line\r\n";
        let pieces = vec![p(text, 20.0, 100.0, 200.0, 72.0)];
        assert_eq!(
            text_of(pieces, Source::Exact),
            "first line\nsecond line\n\nfourth line"
        );
    }

    #[test]
    fn an_element_and_its_parts_are_read_once() {
        // The row's name repeats its two labels: the labels stay.
        let row = vec![
            p("Inbox 3", 10.0, 100.0, 200.0, 24.0),
            p("Inbox", 14.0, 104.0, 50.0, 16.0),
            p("3", 180.0, 104.0, 10.0, 16.0),
        ];
        assert_eq!(text_of(row, Source::Exact), "Inbox 3");
        // A paragraph with a link in it: the paragraph stays, the link's
        // own copy goes.
        let para = vec![
            p("Read the notes here by Friday.", 10.0, 100.0, 300.0, 18.0),
            p("here", 120.0, 100.0, 34.0, 18.0),
        ];
        assert_eq!(
            text_of(para, Source::Exact),
            "Read the notes here by Friday."
        );
        // The same text twice in the same place is one text.
        let twice = vec![
            p("Save", 10.0, 100.0, 60.0, 24.0),
            p("Save", 10.0, 100.0, 60.0, 24.0),
        ];
        assert_eq!(text_of(twice, Source::Exact), "Save");
        // Two buttons that only share a word are two texts.
        let two = vec![
            p("Save", 10.0, 100.0, 60.0, 24.0),
            p("Save as", 90.0, 100.0, 80.0, 24.0),
        ];
        assert_eq!(text_of(two, Source::Exact), "Save Save as");
    }

    #[test]
    fn chinese_and_japanese_join_without_spaces() {
        let pieces = vec![
            p("日", 20.0, 100.0, 16.0, 16.0),
            p("本", 38.0, 100.0, 16.0, 16.0),
            p("語", 56.0, 100.0, 16.0, 16.0),
            p("OK", 80.0, 100.0, 24.0, 16.0),
        ];
        assert_eq!(text_of(pieces, Source::Ocr), "日本語 OK");
        assert_eq!(count_label("日本語のテキスト"), "Copied 8 characters");
    }

    #[test]
    fn the_confirmation_counts_words_and_shows_the_first_few() {
        assert_eq!(count_label("one"), "Copied 1 word");
        assert_eq!(
            count_label("Invoice 2291 for March\nPlease settle by Friday\ntotal due 42.00"),
            "Copied 11 words"
        );
        assert_eq!(count_label("字"), "Copied 1 character");
        assert_eq!(lead("total due 42.00"), "total due 42.00");
        assert_eq!(
            lead("Invoice 2291 for March\nPlease settle by Friday"),
            "Invoice 2291 for March Please settle…"
        );
        // Long words: 40 characters at most.
        assert_eq!(
            lead("internationalisation considerations notwithstanding everything"),
            "internationalisation considerations…"
        );
        let url = "https://example.com/a/very/long/path/that/keeps/going/on";
        assert_eq!(lead(url).chars().count(), 41);
        assert!(lead(url).ends_with('…'));
        assert_eq!(lead(""), "");
    }
}
