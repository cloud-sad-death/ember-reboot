//! In-band private channel. Ember's zsh integration writes `ESC ] 7777 ; <kind> ; payload` markers into
//! the PTY stream, so they arrive in exact byte order with the output. They are removed before display.
//! P = preexec (fields split by 0x1f), D = command done (exit code), I = find item, T = a command a loop is about to run (DEBUG trap).
use memchr::memchr;

const PREFIX: &[u8] = b"\x1b]7777;";
const MAX_PENDING: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Preexec { typed: String, full: String, cwd: String, is_loop: bool, body: String },
    Done(i32),
    Item(String),
    Trace(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    Data(Vec<u8>),
    Event(Event),
}

#[derive(Default)]
pub struct OscFilter {
    pending: Vec<u8>,
}

impl OscFilter {
    /// Splits input into display bytes (appended to `display`) and ordered pieces for the recorder.
    pub fn feed(&mut self, input: &[u8], display: &mut Vec<u8>, pieces: &mut Vec<Piece>) {
        let owned;
        let buf: &[u8] = if self.pending.is_empty() {
            input
        } else {
            let mut p = std::mem::take(&mut self.pending);
            p.extend_from_slice(input);
            owned = p;
            &owned
        };
        let mut pos = 0;
        let mut data_start = 0;
        let flush = |from: usize, to: usize, display: &mut Vec<u8>, pieces: &mut Vec<Piece>| {
            if to > from {
                display.extend_from_slice(&buf[from..to]);
                match pieces.last_mut() {
                    Some(Piece::Data(d)) => d.extend_from_slice(&buf[from..to]),
                    _ => pieces.push(Piece::Data(buf[from..to].to_vec())),
                }
            }
        };
        while let Some(off) = memchr(0x1b, &buf[pos..]) {
            let at = pos + off;
            let rest = &buf[at..];
            if rest.len() < PREFIX.len() {
                if PREFIX.starts_with(rest) {
                    flush(data_start, at, display, pieces);
                    self.pending = rest.to_vec();
                    return;
                }
                pos = at + 1;
                continue;
            }
            if !rest.starts_with(PREFIX) {
                pos = at + 1;
                continue;
            }
            let body = &rest[PREFIX.len()..];
            let Some(&kind) = body.first() else {
                flush(data_start, at, display, pieces);
                self.pending = rest.to_vec();
                return;
            };
            let payload_start = 2.min(body.len());
            let term = memchr(0x07, &body[payload_start..]);
            let Some(tlen) = term else {
                if rest.len() > MAX_PENDING {
                    pos = at + 1;
                    continue;
                }
                flush(data_start, at, display, pieces);
                self.pending = rest.to_vec();
                return;
            };
            flush(data_start, at, display, pieces);
            let payload = &body[payload_start..payload_start + tlen];
            if let Some(ev) = parse(kind, payload) {
                pieces.push(Piece::Event(ev));
            }
            pos = at + PREFIX.len() + payload_start + tlen + 1;
            data_start = pos;
        }
        flush(data_start, buf.len(), display, pieces);
    }
}

fn parse(kind: u8, payload: &[u8]) -> Option<Event> {
    let s = String::from_utf8_lossy(payload);
    Some(match kind {
        b'P' => {
            let f: Vec<&str> = s.split('\x1f').collect();
            let g = |i: usize| f.get(i).copied().unwrap_or("").to_string();
            Event::Preexec { typed: g(0), full: g(1), cwd: g(2), is_loop: g(3) == "1", body: g(4) }
        }
        b'D' => Event::Done(s.trim().parse().unwrap_or(0)),
        b'I' => Event::Item(s.to_string()),
        b'T' => Event::Trace(s.trim_end_matches('\r').to_string()),
        _ => return None,
    })
}

#[cfg(test)]
mod t {
    use super::*;
    #[test]
    fn split_across_chunks() {
        let all = b"hi\x1b]7777;I;/a/b\x07out\r\n\x1b]7777;T;cat /x\x07\x1b[31mred\x1b]7777;D;3\x07".to_vec();
        for cut in 0..all.len() {
            let mut f = OscFilter::default();
            let (mut d, mut p) = (vec![], vec![]);
            f.feed(&all[..cut], &mut d, &mut p);
            f.feed(&all[cut..], &mut d, &mut p);
            assert_eq!(d, b"hiout\r\n\x1b[31mred", "cut {cut}");
            let evs: Vec<_> = p.iter().filter_map(|x| if let Piece::Event(e) = x { Some(e.clone()) } else { None }).collect();
            assert_eq!(evs, vec![Event::Item("/a/b".into()), Event::Trace("cat /x".into()), Event::Done(3)], "cut {cut}");
        }
    }
}
