//! Search across all history. Plain text and regex scan the compressed text streams in parallel across all
//! cores (every saved byte). The keyword index (index.rs) answers whole-word queries instantly.
use crate::store::{list_sessions, LogReader, Row, SessionReader};
use anyhow::Result;
use chrono::{DateTime, Local};
use rayon::prelude::*;
use regex::bytes::{Regex, RegexBuilder};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct Filters {
    pub since: Option<DateTime<Local>>,
    pub until: Option<DateTime<Local>>,
    pub host: Option<String>,
    pub cwd: Option<String>,
    pub mode: Option<String>,
    pub command: Option<String>,
    /// Some(true): only non-zero exit; Some(false): only zero.
    pub failed: Option<bool>,
}

impl Filters {
    pub fn row_ok(&self, r: &Row) -> bool {
        !r.deleted
            && self.since.map_or(true, |s| r.ts >= s)
            && self.until.map_or(true, |u| r.ts <= u)
            && self.cwd.as_ref().map_or(true, |c| r.cwd.contains(c.as_str()))
            && self.mode.as_ref().map_or(true, |m| &r.mode == m)
            && self.command.as_ref().map_or(true, |c| r.header.contains(c.as_str()) || r.command.contains(c.as_str()))
            && self.failed.map_or(true, |f| f == r.rc.map_or(false, |rc| rc != 0))
    }
}

#[derive(Debug, Clone)]
pub struct Query {
    pub pattern: String,
    pub regex: bool,
    pub ignore_case: bool,
    pub words: bool,
    pub filters: Filters,
    pub sessions: Option<Vec<PathBuf>>,
    pub limit: usize,
}

impl Query {
    pub fn new(p: &str) -> Self {
        Self { pattern: p.into(), regex: false, ignore_case: true, words: false, filters: Filters::default(), sessions: None, limit: 10_000 }
    }
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub session: PathBuf,
    pub session_id: String,
    pub host: String,
    pub row_id: u64,
    pub header: String,
    pub ts: DateTime<Local>,
    pub offset_in_row: u64,
    pub line: String,
    pub col: (usize, usize),
}

#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub sessions: usize,
    pub frames: usize,
    pub bytes: u64,
    pub millis: u128,
    pub used_index: bool,
}

pub fn build_regex(q: &Query) -> Result<Regex> {
    let mut p = if q.regex { q.pattern.clone() } else { regex::escape(&q.pattern) };
    if q.words {
        p = format!(r"\b(?:{p})\b");
    }
    Ok(RegexBuilder::new(&p).case_insensitive(q.ignore_case).multi_line(true).size_limit(1 << 26).build()?)
}

struct Sess {
    reader: SessionReader,
    // rows with non-empty text, sorted by text start
    spans: Vec<usize>,
}

pub fn search(root: &Path, q: &Query) -> Result<(Vec<Hit>, Stats)> {
    let t0 = std::time::Instant::now();
    let re = build_regex(q)?;
    let dirs = q.sessions.clone().unwrap_or_else(|| list_sessions(root));
    let mut used_index = false;
    let sessions: Vec<(Sess, Option<Vec<u64>>)> = dirs
        .iter()
        .filter_map(|d| SessionReader::open(d).ok())
        .filter(|r| q.filters.host.as_ref().map_or(true, |h| r.meta.host.contains(h.as_str())))
        .map(|reader| {
            let mut spans: Vec<usize> = (0..reader.rows.len()).filter(|&i| q.filters.row_ok(&reader.rows[i])).collect();
            spans.retain(|&i| {
                let r = &reader.rows[i];
                reader.row_end(r).1 > r.text.0
            });
            spans.sort_by_key(|&i| reader.rows[i].text.0);
            let cand = if q.words && !q.regex { crate::index::candidates(&reader.dir, &q.pattern) } else { None };
            (Sess { reader, spans }, cand)
        })
        .collect();
    // (session, frame) work items, only frames that overlap a qualifying row
    let mut work = vec![];
    for (si, (s, cand)) in sessions.iter().enumerate() {
        let mut frames: Vec<usize> = vec![];
        for &ri in &s.spans {
            let r = &s.reader.rows[ri];
            if let Some(c) = cand {
                used_index = true;
                if c.binary_search(&r.id).is_err() {
                    continue;
                }
            }
            let (a, b) = (r.text.0, s.reader.row_end(r).1);
            let (fa, fb) = (s.reader.text.frame_at(a), s.reader.text.frame_at(b.saturating_sub(1)));
            if frames.last().map_or(true, |&l| l < fa) {
                frames.extend(fa..=fb.min(s.reader.text.frames.len().saturating_sub(1)));
            } else {
                let l = *frames.last().unwrap();
                frames.extend((l + 1)..=fb.min(s.reader.text.frames.len().saturating_sub(1)));
            }
        }
        frames.dedup();
        work.extend(frames.into_iter().map(|f| (si, f)));
    }
    let limit = q.limit;
    let results: Vec<(Vec<Hit>, u64)> = work
        .par_iter()
        .map(|&(si, fi)| {
            let s = &sessions[si].0;
            let data = match frame_uncached(&s.reader.text, fi) {
                Ok(d) => d,
                Err(_) => return (vec![], 0),
            };
            let base = s.reader.text.frames[fi].raw;
            let mut hits = vec![];
            let mut last_line = usize::MAX;
            for m in re.find_iter(&data) {
                let ls = memchr::memrchr(b'\n', &data[..m.start()]).map_or(0, |i| i + 1);
                if ls == last_line {
                    continue;
                }
                let abs = base + m.start() as u64;
                let Some(ri) = row_at(s, abs) else { continue };
                let r = &s.reader.rows[ri];
                last_line = ls;
                let le = memchr::memchr(b'\n', &data[m.end()..]).map_or(data.len(), |i| m.end() + i);
                let (ls2, le2) = clamp_line(ls, le, m.start(), m.end());
                hits.push(Hit {
                    session: s.reader.dir.clone(),
                    session_id: s.reader.meta.id.clone(),
                    host: s.reader.meta.host.clone(),
                    row_id: r.id,
                    header: r.header.clone(),
                    ts: r.ts,
                    offset_in_row: abs - r.text.0,
                    line: String::from_utf8_lossy(&data[ls2..le2]).to_string(),
                    col: (m.start() - ls2, m.end() - ls2),
                });
                if hits.len() >= limit {
                    break;
                }
            }
            (hits, data.len() as u64)
        })
        .collect();
    let bytes = results.iter().map(|r| r.1).sum();
    let mut hits: Vec<Hit> = results.into_iter().flat_map(|r| r.0).collect();
    hits.truncate(limit);
    // header hits: commands and targets themselves are searchable
    for (s, _) in &sessions {
        for &ri in &s.spans {
            let r = &s.reader.rows[ri];
            if let Some(m) = re.find(r.header.as_bytes()) {
                if hits.len() < limit && !hits.iter().any(|h| h.row_id == r.id && h.session == s.reader.dir && h.offset_in_row == 0) {
                    hits.push(Hit { session: s.reader.dir.clone(), session_id: s.reader.meta.id.clone(), host: s.reader.meta.host.clone(), row_id: r.id, header: r.header.clone(), ts: r.ts, offset_in_row: 0, line: r.header.clone(), col: (m.start(), m.end()) });
                }
            }
        }
    }
    let stats = Stats { sessions: sessions.len(), frames: work.len(), bytes, millis: t0.elapsed().as_millis(), used_index };
    Ok((hits, stats))
}

fn clamp_line(ls: usize, le: usize, ms: usize, me: usize) -> (usize, usize) {
    const W: usize = 400;
    if le - ls <= W {
        return (ls, le);
    }
    let a = ms.saturating_sub(W / 2).max(ls);
    (a, (me + W / 2).min(le))
}

fn row_at(s: &Sess, abs: u64) -> Option<usize> {
    let rows = &s.reader.rows;
    let k = s.spans.partition_point(|&i| rows[i].text.0 <= abs);
    if k == 0 {
        return None;
    }
    let ri = s.spans[k - 1];
    (abs < s.reader.row_end(&rows[ri]).1).then_some(ri)
}

fn frame_uncached(r: &LogReader, i: usize) -> Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let f = r.frames[i];
    let mut file = std::fs::File::open(r_path(r))?;
    file.seek(SeekFrom::Start(f.comp))?;
    let mut comp = vec![0u8; f.comp_len as usize];
    file.read_exact(&mut comp)?;
    Ok(zstd::bulk::decompress(&comp, f.raw_len as usize)?)
}

fn r_path(r: &LogReader) -> PathBuf {
    r.data_path()
}
