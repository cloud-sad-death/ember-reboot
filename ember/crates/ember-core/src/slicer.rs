//! Standard-mode slicing (blast30 rules). One row per command; loops become one row per item.
//! Item boundaries, first source wins per command: an xtrace line whose command matches the loop verb,
//! a find item marker, or blast30's rule (a bare absolute path alone on its line at bracket depth 0).
use crate::ansi::{strip, Stripper};
use crate::loopdetect::first_word;
use crate::store::RowKind;

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Begin { kind: RowKind, header: String },
    Raw(Vec<u8>),
    Text(Vec<u8>),
    End,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Source {
    None,
    Trace,
    Item,
    Path,
}

pub struct LoopSlicer {
    cmd: String,
    is_loop: bool,
    verb: String,
    verb_word: String,
    source: Source,
    row_open: bool,
    any_row: bool,
    partial: Vec<u8>,
    depth: i32,
    strip: Stripper,
    pub ops: Vec<Op>,
}

impl LoopSlicer {
    pub fn new(cmd: &str, is_loop: bool, verb: &str) -> Self {
        Self {
            cmd: cmd.to_string(),
            is_loop,
            verb: verb.to_string(),
            verb_word: first_word(verb).to_string(),
            source: Source::None,
            row_open: false,
            any_row: false,
            partial: vec![],
            depth: 0,
            strip: Stripper::default(),
            ops: vec![],
        }
    }

    fn begin(&mut self, kind: RowKind, header: String) {
        if self.row_open {
            self.ops.push(Op::End);
        }
        self.strip = Stripper::default();
        self.ops.push(Op::Begin { kind, header });
        self.row_open = true;
        self.any_row = true;
    }

    fn ensure_row(&mut self) {
        if !self.row_open {
            let h = self.cmd.clone();
            self.begin(RowKind::Command, h);
        }
    }

    fn emit(&mut self, b: &[u8]) {
        if b.is_empty() {
            return;
        }
        self.ensure_row();
        let mut t = Vec::new();
        self.strip.feed(b, &mut t);
        self.ops.push(Op::Raw(b.to_vec()));
        if !t.is_empty() {
            self.ops.push(Op::Text(t));
        }
    }

    fn label(&self, target: &str) -> String {
        if self.verb.is_empty() {
            target.to_string()
        } else {
            format!("{} {}", self.verb, target)
        }
    }

    fn flush_partial(&mut self) {
        let p = std::mem::take(&mut self.partial);
        self.emit(&p);
    }

    pub fn data(&mut self, b: &[u8]) {
        if !self.is_loop || matches!(self.source, Source::Trace | Source::Item) {
            self.emit(b);
            return;
        }
        self.partial.extend_from_slice(b);
        let Some(last_nl) = memchr::memrchr(b'\n', &self.partial) else {
            if self.partial.len() > 1 << 20 {
                self.flush_partial();
            }
            return;
        };
        let rest = self.partial.split_off(last_nl + 1);
        let done = std::mem::replace(&mut self.partial, rest);
        for line in done.split_inclusive(|&c| c == b'\n') {
            self.path_line(line);
        }
    }

    fn path_line(&mut self, line: &[u8]) {
        let clean = strip(line);
        let label = String::from_utf8_lossy(&clean).trim_matches(|c| c == ' ' || c == '\t' || c == '\r' || c == '\n').to_string();
        if self.depth == 0 && label.starts_with('/') && !label.contains('\t') {
            self.source = Source::Path;
            let h = self.label(&label);
            self.begin(RowKind::Item, h);
        }
        self.emit(line);
        for c in clean.iter() {
            match c {
                b'{' | b'[' => self.depth += 1,
                b'}' | b']' => self.depth = (self.depth - 1).max(0),
                _ => {}
            }
        }
    }

    pub fn trace(&mut self, text: &str) {
        if !self.is_loop || self.verb_word.is_empty() || !matches!(self.source, Source::None | Source::Trace) {
            return;
        }
        if first_word(text) == self.verb_word {
            self.flush_partial();
            self.source = Source::Trace;
            self.begin(RowKind::Item, text.trim().to_string());
        }
    }

    pub fn item(&mut self, path: &str) {
        if !self.is_loop || !matches!(self.source, Source::None | Source::Item) {
            return;
        }
        self.flush_partial();
        self.source = Source::Item;
        let h = self.label(path);
        self.begin(RowKind::Item, h);
    }

    pub fn finish(&mut self) {
        self.flush_partial();
        if !self.any_row {
            self.ensure_row();
        }
        if self.row_open {
            self.ops.push(Op::End);
            self.row_open = false;
        }
    }
}

#[cfg(test)]
mod t {
    use super::*;
    fn headers(s: &LoopSlicer) -> Vec<String> {
        s.ops.iter().filter_map(|o| if let Op::Begin { header, .. } = o { Some(header.clone()) } else { None }).collect()
    }
    #[test]
    fn plain_command_one_row() {
        let mut s = LoopSlicer::new("ls", false, "");
        s.data(b"a\nb\n");
        s.finish();
        assert_eq!(headers(&s), vec!["ls"]);
    }
    #[test]
    fn bare_path_with_depth() {
        let mut s = LoopSlicer::new("plup", true, "plutil -p");
        s.data(b"/x/a.plist\n{\n  \"k\" => [\n/nested/path\n  ]\n}\n/x/b.plist\n{}\n");
        s.finish();
        assert_eq!(headers(&s), vec!["plutil -p /x/a.plist", "plutil -p /x/b.plist"]);
    }
    #[test]
    fn find_items_and_preamble() {
        let mut s = LoopSlicer::new("find /t -exec cat {} \\;", true, "cat");
        s.data(b"warming up\n");
        s.item("/t/a.query");
        s.data(b"AAA\n");
        s.item("/t/b.query");
        s.data(b"BBB\n");
        s.finish();
        assert_eq!(headers(&s), vec!["find /t -exec cat {} \\;", "cat /t/a.query", "cat /t/b.query"]);
    }
    #[test]
    fn trace_items() {
        let mut s = LoopSlicer::new("for f in *; do cat $f; done", true, "cat");
        s.trace("f=a");
        s.trace("cat a");
        s.data(b"A\n");
        s.trace("cat b");
        s.data(b"/abs/looking/line\n");
        s.finish();
        assert_eq!(headers(&s), vec!["cat a", "cat b"]);
    }
}
