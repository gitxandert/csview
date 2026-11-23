// the 'o' view: a cell's full content, wrapped at word boundaries,
// with a command line for finding and replacing text inside the cell

use crate::terminal::is_delimiter;

// a cell's raw content split into the chars it shows (newlines included)
// and the escape sequences that come before each of them
#[derive(Debug, Default)]
pub struct CellText {
    pub chars: Vec<char>,
    pre: Vec<String>, // escapes printed before chars[i]
    tail: String,     // escapes after the last char
}

impl CellText {
    pub fn parse(raw: &str) -> Self {
        let mut text = Self::default();
        let mut pending = String::new();
        let mut it = raw.chars();
        while let Some(c) = it.next() {
            if c != '\x1b' {
                text.chars.push(c);
                text.pre.push(std::mem::take(&mut pending));
                continue;
            }
            // same rule as Cell::new: ESC + one char, or a CSI up to its final byte
            pending.push(c);
            if let Some(next) = it.next() {
                pending.push(next);
                if next == '[' {
                    for n in it.by_ref() {
                        pending.push(n);
                        if ('@'..='~').contains(&n) {
                            break;
                        }
                    }
                }
            }
        }
        text.tail = pending;
        text
    }

    pub fn to_raw(&self) -> String {
        let mut raw = String::new();
        for (c, pre) in self.chars.iter().zip(&self.pre) {
            raw.push_str(pre);
            raw.push(*c);
        }
        raw.push_str(&self.tail);
        raw
    }

    // non-overlapping (start, end) char ranges of term
    pub fn find(&self, term: &[char], insensitive: bool) -> Vec<(usize, usize)> {
        let fold = |c: char| match insensitive {
            true => c.to_lowercase().next().unwrap_or(c),
            false => c,
        };
        let mut found = Vec::new();
        if term.is_empty() {
            return found;
        }
        let mut i = 0usize;
        while i + term.len() <= self.chars.len() {
            if (0..term.len()).all(|k| fold(self.chars[i + k]) == fold(term[k])) {
                found.push((i, i + term.len()));
                i += term.len();
            } else {
                i += 1;
            }
        }
        found
    }

    // replaces every match; escapes inside a match are kept
    // in front of its replacement, so formatting carries over
    pub fn replace(&mut self, term: &[char], new: &[char], insensitive: bool) -> usize {
        let found = self.find(term, insensitive);
        if found.is_empty() {
            return 0;
        }
        let mut chars = Vec::with_capacity(self.chars.len());
        let mut pre = Vec::with_capacity(self.chars.len());
        let mut carry = String::new();
        let mut next = found.iter().peekable();
        let mut i = 0usize;
        while i < self.chars.len() {
            match next.peek() {
                Some(&&(s, e)) if s == i => {
                    next.next();
                    for p in &self.pre[s..e] {
                        carry.push_str(p);
                    }
                    for &c in new {
                        chars.push(c);
                        pre.push(std::mem::take(&mut carry));
                    }
                    i = e;
                }
                _ => {
                    carry.push_str(&self.pre[i]);
                    chars.push(self.chars[i]);
                    pre.push(std::mem::take(&mut carry));
                    i += 1;
                }
            }
        }
        carry.push_str(&self.tail);
        self.chars = chars;
        self.pre = pre;
        self.tail = carry;
        found.len()
    }

    // rows of at most width chars as (start, end) char ranges; lines break at
    // the last whitespace that fits, and only a word longer than a whole row is cut.
    // the newline or whitespace a row breaks on isn't part of either row
    pub fn wrap(&self, width: usize) -> Vec<(usize, usize)> {
        let width = width.max(1);
        let chars = &self.chars;
        let mut rows = Vec::new();
        let mut line_start = 0usize;
        loop {
            let line_end = chars[line_start..]
                .iter()
                .position(|&c| c == '\n')
                .map_or(chars.len(), |p| line_start + p);

            let mut s = line_start;
            loop {
                if line_end - s <= width {
                    rows.push((s, line_end));
                    break;
                }
                let limit = s + width; // first char that doesn't fit
                match (s + 1..=limit).rev().find(|&i| chars[i].is_whitespace()) {
                    Some(brk) => {
                        rows.push((s, brk));
                        s = brk;
                        while s < line_end && chars[s].is_whitespace() {
                            s += 1;
                        }
                        if s == line_end {
                            break;
                        }
                    }
                    None => {
                        rows.push((s, limit));
                        s = limit;
                    }
                }
            }

            if line_end == chars.len() {
                break;
            }
            line_start = line_end + 1;
        }
        rows
    }

    // apply the SGR escapes before chars[from..to] to the active formatting
    fn track_format(&self, active: &mut String, from: usize, to: usize) {
        for p in &self.pre[from..to] {
            push_sgr(active, p);
        }
    }

    // draw rows[first..first + count] on screen lines 1.., highlighting matches
    // (current one in yellow)
    pub fn render(
        &self,
        rows: &[(usize, usize)],
        first: usize,
        count: usize,
        matches: &[(usize, usize)],
        current: Option<usize>,
    ) -> String {
        let mut out = String::new();
        let mut active = String::new();
        let mut seen = 0usize; // chars whose escapes are in `active`
        for line in 0..count {
            out.push_str(&format!("\x1b[{};1H\x1b[2K\x1b[0m", line + 1));
            let Some(&(s, e)) = rows.get(first + line) else { continue };
            self.track_format(&mut active, seen.min(s), s);
            out.push_str(&active);

            let mut hl: Option<&str> = None;
            for k in s..e {
                let want = matches
                    .iter()
                    .position(|&(ms, me)| ms <= k && k < me)
                    .map(|m| match Some(m) == current {
                        true => "\x1b[30;43m",
                        false => "\x1b[7m",
                    });
                let mut sgr = String::new();
                push_sgr(&mut sgr, &self.pre[k]);
                if want != hl || (!sgr.is_empty() && want.is_some()) {
                    push_sgr(&mut active, &self.pre[k]);
                    out.push_str("\x1b[0m");
                    out.push_str(&active);
                    if let Some(h) = want {
                        out.push_str(h);
                    }
                    hl = want;
                } else {
                    push_sgr(&mut active, &self.pre[k]);
                    out.push_str(&sgr);
                }
                out.push(match self.chars[k].is_control() {
                    true => ' ',
                    false => self.chars[k],
                });
            }
            seen = e;
            out.push_str("\x1b[0m");
        }
        out
    }
}

// keep only SGR (formatting) escapes from pre, folded into active;
// other escapes would move the cursor around the view
fn push_sgr(active: &mut String, pre: &str) {
    let mut rest = pre;
    while let Some(i) = rest.find('\x1b') {
        let seq_start = &rest[i..];
        let end = match seq_start.strip_prefix("\x1b[") {
            // CSI: up to and including its final byte
            Some(params) => params
                .char_indices()
                .find(|&(_, c)| ('@'..='~').contains(&c))
                .map_or(seq_start.len(), |(j, c)| 2 + j + c.len_utf8()),
            // ESC + one char
            None => seq_start.chars().take(2).map(char::len_utf8).sum(),
        };
        let seq = &seq_start[..end];
        if seq.starts_with("\x1b[") && seq.ends_with('m') {
            match seq {
                "\x1b[m" | "\x1b[0m" => active.clear(),
                _ => active.push_str(seq),
            }
        }
        rest = &seq_start[end..];
    }
}

// \n, \t and \\ in find/replace arguments, so newlines can be matched and written
pub fn unescape(s: &str) -> Vec<char> {
    let mut out = Vec::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some(o) => { out.push('\\'); out.push(o); }
            None => out.push('\\'),
        }
    }
    out
}

// what the 'o' view shows: the wrapped text, where it's scrolled to,
// the current search, and the command line while one is open
#[derive(Default)]
pub struct CellView {
    pub text: CellText,
    pub rows: Vec<(usize, usize)>,
    pub top: usize, // first row on screen
    pub term: String, // search as typed, for the status line
    term_chars: Vec<char>,
    insensitive: bool,
    pub matches: Vec<(usize, usize)>,
    pub current: Option<usize>,
    pub cmd: Option<LineEdit>,
    pub msg: String,
}

impl CellView {
    pub fn new(raw: &str) -> Self {
        Self { text: CellText::parse(raw), ..Self::default() }
    }

    pub fn relayout(&mut self, width: usize, page: usize) {
        self.rows = self.text.wrap(width);
        self.top = self.top.min(self.max_top(page));
    }

    fn max_top(&self, page: usize) -> usize {
        self.rows.len().saturating_sub(page)
    }

    pub fn scroll(&mut self, by: isize, page: usize) {
        self.top = self.top.saturating_add_signed(by).min(self.max_top(page));
    }

    // the row a char shows on (or ends, for the space or newline a row breaks on)
    fn row_of(&self, idx: usize) -> usize {
        self.rows.iter().rposition(|&(s, _)| s <= idx).unwrap_or(0)
    }

    // scroll the current match onto the screen, a third of the way down
    pub fn show_match(&mut self, page: usize) {
        let Some(c) = self.current else { return };
        let row = self.row_of(self.matches[c].0);
        if row < self.top || row >= self.top + page {
            self.top = row.saturating_sub(page / 3).min(self.max_top(page));
        }
    }

    // search from the top of the screen down; an empty term clears the search
    pub fn search(&mut self, shown: &str, term: Vec<char>, insensitive: bool, page: usize) {
        self.term = shown.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
        self.matches = self.text.find(&term, insensitive);
        self.term_chars = term;
        self.insensitive = insensitive;
        self.current = match self.matches.is_empty() {
            true => None,
            false => Some(
                self.matches
                    .iter()
                    .position(|&(s, _)| self.row_of(s) >= self.top)
                    .unwrap_or(0),
            ),
        };
        self.show_match(page);
    }

    // run the search again after the text changed
    pub fn research(&mut self, page: usize) {
        if !self.term_chars.is_empty() {
            let (shown, term) = (self.term.clone(), self.term_chars.clone());
            self.search(&shown, term, self.insensitive, page);
        }
    }

    pub fn step(&mut self, forward: bool, page: usize) {
        let Some(c) = self.current else { return };
        let n = self.matches.len();
        self.current = Some(match forward {
            true => (c + 1) % n,
            false => (c + n - 1) % n,
        });
        self.show_match(page);
    }

    pub fn status(&self, page: usize, width: usize) -> String {
        let left = match (&self.msg, self.current) {
            (m, _) if !m.is_empty() => m.clone(),
            (_, Some(c)) => format!(
                "{}/{} '{}' · n/b next/prev",
                c + 1, self.matches.len(), self.term
            ),
            _ => ": f/find r/replace · ctrl+↑↓ page · other keys close".to_string(),
        };
        let last = (self.top + page).min(self.rows.len());
        let pos = format!(" {}-{}/{} ", self.top + 1, last, self.rows.len());
        let room = width.saturating_sub(pos.chars().count());
        let left: String = left.chars().take(room).collect();
        let pad = room - left.chars().count();
        format!("\x1b[7m{left}{}{pos}\x1b[0m", " ".repeat(pad))
    }
}

pub enum LineAction {
    Edit,
    Submit(String),
    Cancel,
}

// the 'o' view's command line
#[derive(Default)]
pub struct LineEdit {
    line: Vec<char>,
    pos: usize,
    scroll: usize,
}

impl LineEdit {
    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars().filter(|c| !c.is_control()) {
            self.line.insert(self.pos, c);
            self.pos += 1;
        }
    }

    pub fn key(&mut self, key: &[u8]) -> LineAction {
        match key {
            [13] | [10] | [13, 10] => return LineAction::Submit(self.line.iter().collect()),
            [17] | [27] => return LineAction::Cancel, // ctrl + q / escape
            [27, 91, 67] => self.pos = (self.pos + 1).min(self.line.len()),
            [27, 91, 68] => self.pos = self.pos.saturating_sub(1),
            [27, 91, 72] | [27, 79, 72] | [27, 91, 49, 126] | [27, 91, 55, 126] => self.pos = 0,
            [27, 91, 70] | [27, 79, 70] | [27, 91, 52, 126] | [27, 91, 56, 126] => {
                self.pos = self.line.len()
            }
            [27, 91, 49, 59, 53, 67] => {
                while self.pos < self.line.len() && !is_delimiter(self.line[self.pos]) { self.pos += 1; }
                while self.pos < self.line.len() && is_delimiter(self.line[self.pos]) { self.pos += 1; }
            }
            [27, 91, 49, 59, 53, 68] => {
                while self.pos > 0 && is_delimiter(self.line[self.pos - 1]) { self.pos -= 1; }
                while self.pos > 0 && !is_delimiter(self.line[self.pos - 1]) { self.pos -= 1; }
            }
            [127] | [8] => {
                if self.pos > 0 {
                    self.pos -= 1;
                    self.line.remove(self.pos);
                }
            }
            [27, 91, 51, 126] => {
                if self.pos < self.line.len() {
                    self.line.remove(self.pos);
                }
            }
            [27, ..] | [0..=31] => (),
            bytes => {
                if let Ok(s) = std::str::from_utf8(bytes) {
                    self.insert_str(s);
                }
            }
        }
        LineAction::Edit
    }

    // the line drawn on screen row `row`, ending with the cursor shown at pos
    pub fn render(&mut self, row: usize, width: usize) -> String {
        let avail = width.saturating_sub(2).max(1);
        if self.pos < self.scroll {
            self.scroll = self.pos;
        } else if self.pos >= self.scroll + avail {
            self.scroll = self.pos + 1 - avail;
        }
        let shown: String = self.line.iter().skip(self.scroll).take(avail).collect();
        let col = self.pos - self.scroll + 2;
        format!("\x1b[{row};1H\x1b[2K\x1b[0m:{shown}\x1b[{row};{col}H\x1b[?25h")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows_of(text: &str, width: usize) -> Vec<String> {
        let t = CellText::parse(text);
        t.wrap(width)
            .iter()
            .map(|&(s, e)| t.chars[s..e].iter().collect())
            .collect()
    }

    #[test]
    fn wrap_breaks_between_words() {
        assert_eq!(rows_of("the quick brown fox", 10), vec!["the quick", "brown fox"]);
        assert_eq!(rows_of("aaaa bbbb", 4), vec!["aaaa", "bbbb"]);
        assert_eq!(rows_of("ab cd", 5), vec!["ab cd"]);
    }

    #[test]
    fn wrap_cuts_only_words_longer_than_a_row() {
        assert_eq!(rows_of("abcdefghij xy", 4), vec!["abcd", "efgh", "ij", "xy"]);
    }

    #[test]
    fn wrap_keeps_newlines_and_empty_lines() {
        assert_eq!(rows_of("one\n\ntwo three", 5), vec!["one", "", "two", "three"]);
        assert_eq!(rows_of("", 5), vec![""]);
        assert_eq!(rows_of("end\n", 5), vec!["end", ""]);
    }

    #[test]
    fn parse_round_trips_escapes() {
        let raw = "a\x1b[31mb\nc\x1b[0m";
        let t = CellText::parse(raw);
        assert_eq!(t.chars.iter().collect::<String>(), "ab\nc");
        assert_eq!(t.to_raw(), raw);
    }

    #[test]
    fn find_is_non_overlapping_and_can_ignore_case() {
        let t = CellText::parse("aaa Foo foo");
        assert_eq!(t.find(&['a', 'a'], false), vec![(0, 2)]);
        assert_eq!(t.find(&['f', 'o', 'o'], false), vec![(8, 11)]);
        assert_eq!(t.find(&['f', 'o', 'o'], true), vec![(4, 7), (8, 11)]);
    }

    #[test]
    fn replace_keeps_formatting_and_handles_newlines() {
        let mut t = CellText::parse("x \x1b[31mred\x1b[0m y");
        assert_eq!(t.replace(&['r', 'e', 'd'], &['b', 'l', 'u', 'e'], false), 1);
        assert_eq!(t.to_raw(), "x \x1b[31mblue\x1b[0m y");

        let mut t = CellText::parse("a\nb");
        assert_eq!(t.replace(&unescape("\\n"), &[' '], false), 1);
        assert_eq!(t.to_raw(), "a b");

        let mut t = CellText::parse("a\x1b[1mb");
        assert_eq!(t.replace(&['b'], &[], false), 1);
        assert_eq!(t.to_raw(), "a\x1b[1m");
    }

    #[test]
    fn render_highlights_matches_and_drops_cursor_moves() {
        let t = CellText::parse("ab\x1b[2Jcd");
        let rows = t.wrap(10);
        let out = t.render(&rows, 0, 1, &[(1, 3)], Some(0));
        assert!(!out.contains("\x1b[2J"));
        assert!(out.contains("a\x1b[0m\x1b[30;43mbc\x1b[0md"));
    }

    #[test]
    fn sgr_tracking_reads_back_to_back_escapes() {
        let mut active = String::new();
        push_sgr(&mut active, "\x1b(\x1b[31m\x1b[1m");
        assert_eq!(active, "\x1b[31m\x1b[1m");
        push_sgr(&mut active, "\x1b[0m");
        assert_eq!(active, "");
    }

    #[test]
    fn line_edit_types_moves_and_submits() {
        let mut l = LineEdit::default();
        for k in [&b"f"[..], b" ", b"x", b"\x1b[D", b"\x7f", b"\x1b[H", b"r"] {
            l.key(k);
        }
        match l.key(b"\r") {
            LineAction::Submit(s) => assert_eq!(s, "rfx"),
            _ => panic!("expected submit"),
        }
    }
}
