//! ANSI stripping for the searchable text stream. Raw bytes are stored separately for exact replay.

#[derive(Default, Clone, Copy, PartialEq)]
enum St {
    #[default]
    Text,
    Esc,
    Csi,
    Osc,
    OscEsc,
    Str,
    StrEsc,
}

/// Streaming stripper: keeps state across chunks. Drops escape sequences and lone CRs before LF.
#[derive(Default)]
pub struct Stripper {
    st: St,
    cr: bool,
}

impl Stripper {
    pub fn feed(&mut self, input: &[u8], out: &mut Vec<u8>) {
        out.reserve(input.len());
        let mut i = 0;
        while i < input.len() {
            // fast path: copy plain text up to the next escape, CR or backspace in one go
            if self.st == St::Text && !self.cr {
                let rest = &input[i..];
                let n = memchr::memchr3(0x1b, b'\r', 0x08, rest).unwrap_or(rest.len());
                if n > 0 {
                    out.extend(rest[..n].iter().copied().filter(|&c| c != 0x07 && c != 0));
                    i += n;
                    continue;
                }
            }
            let b = input[i];
            i += 1;
            match self.st {
                St::Text => match b {
                    0x1b => self.st = St::Esc,
                    b'\r' => self.cr = true,
                    b'\n' => {
                        self.cr = false;
                        out.push(b'\n');
                    }
                    0x08 => {
                        out.pop();
                    }
                    0x07 | 0x00 | 0x0e | 0x0f => {}
                    _ => {
                        if self.cr {
                            // carriage return without newline: keep progress-bar text readable
                            out.push(b'\n');
                            self.cr = false;
                        }
                        out.push(b)
                    }
                },
                St::Esc => {
                    self.st = match b {
                        b'[' => St::Csi,
                        b']' => St::Osc,
                        b'P' | b'X' | b'^' | b'_' => St::Str,
                        b'(' | b')' | b'*' | b'+' | b'#' | b'%' => St::Esc,
                        _ => St::Text,
                    }
                }
                St::Csi => {
                    if (0x40..=0x7e).contains(&b) {
                        self.st = St::Text
                    }
                }
                St::Osc => match b {
                    0x07 => self.st = St::Text,
                    0x1b => self.st = St::OscEsc,
                    _ => {}
                },
                St::OscEsc => self.st = if b == b'\\' { St::Text } else { St::Osc },
                St::Str => {
                    if b == 0x1b {
                        self.st = St::StrEsc
                    }
                }
                St::StrEsc => self.st = if b == b'\\' { St::Text } else { St::Str },
            }
        }
    }
}

pub fn strip(input: &[u8]) -> Vec<u8> {
    let mut s = Stripper::default();
    let mut out = Vec::new();
    s.feed(input, &mut out);
    out
}

#[cfg(test)]
mod t {
    #[test]
    fn strips() {
        assert_eq!(super::strip(b"\x1b[31mred\x1b[0m\r\nok\x1b]0;title\x07"), b"red\nok");
    }
}
