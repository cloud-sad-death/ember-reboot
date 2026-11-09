//! Streaming mode (stream2, looser): one command header, output cut into timestamped time slices.
//! Screen-redrawing tools (top, nettop) are sliced at full refreshes and stored as raw bytes plus the
//! readable text of each screen; line streams (tcpdump, log stream) are cut on line boundaries.
use crate::ansi::Stripper;
use crate::slicer::Op;
use crate::store::RowKind;
use chrono::Local;
use std::time::{Duration, Instant};

const REFRESH: [&[u8]; 3] = [b"\x1b[2J", b"\x1b[H\x1b[J", b"\x1b[?1049h"];

pub struct StreamSlicer {
    cmd: String,
    interval: Duration,
    on_refresh: bool,
    min_refresh: Duration,
    slice_start: Instant,
    slice_open: bool,
    screen_mode: bool,
    screen: vt100::Parser,
    strip: Stripper,
    pub ops: Vec<Op>,
}

fn find_refresh(b: &[u8]) -> Option<usize> {
    REFRESH.iter().filter_map(|p| memchr::memmem::find(b, p)).min()
}

impl StreamSlicer {
    pub fn new(cmd: &str, interval: Duration, on_refresh: bool, rows: u16, cols: u16) -> Self {
        let mut s = Self {
            cmd: cmd.to_string(),
            interval,
            on_refresh,
            min_refresh: Duration::from_millis(900),
            slice_start: Instant::now(),
            slice_open: false,
            screen_mode: false,
            screen: vt100::Parser::new(rows.max(2), cols.max(2), 0),
            strip: Stripper::default(),
            ops: vec![],
        };
        s.ops.push(Op::Begin { kind: RowKind::Command, header: cmd.to_string() });
        s.ops.push(Op::End);
        s
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.screen.set_size(rows.max(2), cols.max(2));
    }

    fn open(&mut self, now: Instant) {
        let header = format!("{} · {}", self.cmd, Local::now().format("%I:%M:%S %p"));
        self.ops.push(Op::Begin { kind: RowKind::Slice, header });
        self.slice_open = true;
        self.slice_start = now;
        self.strip = Stripper::default();
    }

    fn close(&mut self) {
        if !self.slice_open {
            return;
        }
        if self.screen_mode {
            let mut t = self.screen.screen().contents().into_bytes();
            t.push(b'\n');
            self.ops.push(Op::Text(t));
        }
        self.ops.push(Op::End);
        self.slice_open = false;
    }

    fn push(&mut self, b: &[u8], now: Instant) {
        if b.is_empty() {
            return;
        }
        if !self.slice_open {
            self.open(now);
        }
        self.ops.push(Op::Raw(b.to_vec()));
        self.screen.process(b);
        if !self.screen_mode {
            let mut t = Vec::new();
            self.strip.feed(b, &mut t);
            if !t.is_empty() {
                self.ops.push(Op::Text(t));
            }
        }
    }

    pub fn data(&mut self, mut b: &[u8], now: Instant) {
        if let Some(i) = find_refresh(b) {
            if !self.screen_mode {
                self.screen_mode = true;
                // earlier text of this slice was already kept as lines; the screen text follows it
            }
            if self.on_refresh && self.slice_open && now.duration_since(self.slice_start) >= self.min_refresh {
                let (a, rest) = b.split_at(i);
                self.push(a, now);
                self.close();
                b = rest;
            }
        }
        if !self.screen_mode && self.slice_open && now.duration_since(self.slice_start) >= self.interval {
            if let Some(nl) = memchr::memchr(b'\n', b) {
                let (a, rest) = b.split_at(nl + 1);
                self.push(a, now);
                self.close();
                b = rest;
            }
        }
        if self.screen_mode && !self.on_refresh && self.slice_open && now.duration_since(self.slice_start) >= self.interval {
            self.close();
        }
        self.push(b, now);
    }

    pub fn tick(&mut self, now: Instant) {
        if self.slice_open && now.duration_since(self.slice_start) >= self.interval && !(self.screen_mode && self.on_refresh) {
            self.close();
        }
    }

    pub fn finish(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod t {
    use super::*;
    fn n(s: &StreamSlicer) -> usize {
        s.ops.iter().filter(|o| matches!(o, Op::Begin { kind: RowKind::Slice, .. })).count()
    }
    #[test]
    fn line_stream_slices_on_interval() {
        let mut s = StreamSlicer::new("tcpdump", Duration::from_millis(100), true, 24, 80);
        let t0 = Instant::now();
        s.data(b"pkt1\n", t0);
        s.data(b"pkt2\npkt3\n", t0 + Duration::from_millis(150));
        s.tick(t0 + Duration::from_millis(400));
        s.data(b"pkt4\n", t0 + Duration::from_millis(500));
        s.finish();
        assert_eq!(n(&s), 3);
    }
    #[test]
    fn screen_refresh_slices_and_text() {
        let mut s = StreamSlicer::new("top", Duration::from_secs(5), true, 5, 20);
        let t0 = Instant::now();
        s.data(b"\x1b[H\x1b[2Jload 1.0\r\n", t0);
        s.data(b"\x1b[H\x1b[2Jload 2.0\r\n", t0 + Duration::from_secs(1));
        s.finish();
        assert_eq!(n(&s), 2);
        let texts: Vec<String> = s.ops.iter().filter_map(|o| if let Op::Text(t) = o { Some(String::from_utf8_lossy(t).to_string()) } else { None }).collect();
        assert!(texts[0].contains("load 1.0") && texts[1].contains("load 2.0"), "{texts:?}");
    }
}
