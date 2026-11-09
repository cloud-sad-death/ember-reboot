//! Session storage: append-only logs of independently compressed zstd frames (seekable through a frame
//! index), and an append-only JSONL catalog of rows. Two streams: `raw` (exact bytes, colours replay) and
//! `text` (readable text for search and the report view). Nothing rotates; a row can be any size.
use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const FRAME_TARGET: usize = 1 << 20;
const IDX_REC: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RowKind {
    Command,
    Item,
    Slice,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Row {
    pub id: u64,
    #[serde(default)]
    pub parent: Option<u64>,
    pub kind: RowKind,
    pub header: String,
    pub ts: DateTime<Local>,
    #[serde(default)]
    pub cwd: String,
    pub mode: String,
    pub raw: (u64, u64),
    pub text: (u64, u64),
    #[serde(default)]
    pub rc: Option<i32>,
    #[serde(default)]
    pub open: bool,
    #[serde(default)]
    pub deleted: bool,
    /// Typed command this row belongs to (loop items and slices keep their command for provenance).
    #[serde(default)]
    pub command: String,
    /// Newline count of the row's text (final once the row is closed).
    #[serde(default)]
    pub lines: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionMeta {
    pub id: String,
    pub host: String,
    pub pills: Vec<(String, String)>,
    pub started: Option<DateTime<Local>>,
    #[serde(default)]
    pub ended: Option<DateTime<Local>>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub archived: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum CatOp {
    Begin { row: Row },
    End { id: u64, raw_end: u64, text_end: u64, #[serde(default)] lines: u64 },
    Rc { ids: Vec<u64>, rc: i32 },
    Delete { id: u64 },
    Header { id: u64, header: String },
}

// ── frame log ───────────────────────────────────────────────────────────

enum Job {
    Frame(u64, Vec<u8>),
    Sync(crossbeam_channel::Sender<()>),
}

/// Appends frames. Compression and disk writes run on a worker thread so recording never waits on them.
struct LogWriter {
    tx: crossbeam_channel::Sender<Job>,
    buf: Vec<u8>,
    committed: u64,
}

impl LogWriter {
    fn open(dir: &Path, name: &str, level: i32) -> Result<Self> {
        let mut data = OpenOptions::new().create(true).append(true).open(dir.join(format!("{name}.zlog")))?;
        let mut idx = OpenOptions::new().create(true).append(true).open(dir.join(format!("{name}.idx")))?;
        let frames = read_index(&dir.join(format!("{name}.idx")))?;
        let (committed, mut comp_off) = frames.last().map(|f| (f.raw + f.raw_len as u64, f.comp + f.comp_len as u64)).unwrap_or((0, 0));
        let (tx, rx) = crossbeam_channel::bounded::<Job>(64);
        std::thread::Builder::new().name(format!("ember-zstd-{name}")).spawn(move || {
            let mut cx = zstd::bulk::Compressor::new(level).expect("zstd");
            for job in rx {
                match job {
                    Job::Frame(raw_off, bytes) => {
                        let Ok(comp) = cx.compress(&bytes) else { continue };
                        if data.write_all(&comp).is_err() {
                            continue;
                        }
                        let mut rec = [0u8; IDX_REC];
                        rec[0..8].copy_from_slice(&raw_off.to_le_bytes());
                        rec[8..16].copy_from_slice(&comp_off.to_le_bytes());
                        rec[16..20].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
                        rec[20..24].copy_from_slice(&(comp.len() as u32).to_le_bytes());
                        let _ = data.flush();
                        let _ = idx.write_all(&rec);
                        comp_off += comp.len() as u64;
                    }
                    Job::Sync(done) => {
                        let _ = data.sync_data();
                        let _ = done.send(());
                    }
                }
            }
        })?;
        Ok(Self { tx, buf: Vec::with_capacity(FRAME_TARGET * 2), committed })
    }
    fn len(&self) -> u64 {
        self.committed + self.buf.len() as u64
    }
    fn append(&mut self, b: &[u8]) -> Result<()> {
        self.buf.extend_from_slice(b);
        if self.buf.len() >= FRAME_TARGET {
            self.flush(false)?;
        }
        Ok(())
    }
    /// Sends buffered bytes as one frame. Non-forced flushes cut at the last newline so lines stay whole.
    fn flush(&mut self, force: bool) -> Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let cut = if force {
            self.buf.len()
        } else {
            match memchr::memrchr(b'\n', &self.buf) {
                Some(i) if i + 1 >= self.buf.len() / 2 => i + 1,
                _ => self.buf.len(),
            }
        };
        let rest = self.buf.split_off(cut);
        let frame = std::mem::replace(&mut self.buf, rest);
        let n = frame.len() as u64;
        self.tx.send(Job::Frame(self.committed, frame)).map_err(|_| anyhow::anyhow!("writer stopped"))?;
        self.committed += n;
        if self.buf.capacity() < FRAME_TARGET {
            self.buf.reserve(FRAME_TARGET * 2);
        }
        Ok(())
    }
    /// Waits until every sent frame is on disk.
    fn sync(&self) -> Result<()> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.tx.send(Job::Sync(tx)).map_err(|_| anyhow::anyhow!("writer stopped"))?;
        rx.recv()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub raw: u64,
    pub comp: u64,
    pub raw_len: u32,
    pub comp_len: u32,
}

fn read_index(p: &Path) -> Result<Vec<Frame>> {
    let b = match fs::read(p) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    Ok(b.chunks_exact(IDX_REC)
        .map(|r| Frame {
            raw: u64::from_le_bytes(r[0..8].try_into().unwrap()),
            comp: u64::from_le_bytes(r[8..16].try_into().unwrap()),
            raw_len: u32::from_le_bytes(r[16..20].try_into().unwrap()),
            comp_len: u32::from_le_bytes(r[20..24].try_into().unwrap()),
        })
        .collect())
}

/// Random access over a frame log. Decodes only the frames a range touches; keeps a small frame cache.
pub struct LogReader {
    path: PathBuf,
    idx_path: PathBuf,
    pub frames: Vec<Frame>,
    cache: Mutex<Vec<(usize, Arc<Vec<u8>>)>>,
}

impl LogReader {
    pub fn open(dir: &Path, name: &str) -> Result<Self> {
        let idx_path = dir.join(format!("{name}.idx"));
        Ok(Self { path: dir.join(format!("{name}.zlog")), frames: read_index(&idx_path)?, idx_path, cache: Mutex::new(vec![]) })
    }
    pub fn refresh(&mut self) -> Result<()> {
        self.frames = read_index(&self.idx_path)?;
        Ok(())
    }
    pub fn len(&self) -> u64 {
        self.frames.last().map(|f| f.raw + f.raw_len as u64).unwrap_or(0)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn data_path(&self) -> PathBuf {
        self.path.clone()
    }
    pub fn frame(&self, i: usize) -> Result<Arc<Vec<u8>>> {
        if let Some((_, d)) = self.cache.lock().iter().find(|(k, _)| *k == i) {
            return Ok(d.clone());
        }
        let f = self.frames[i];
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(f.comp))?;
        let mut comp = vec![0u8; f.comp_len as usize];
        file.read_exact(&mut comp)?;
        let d = Arc::new(zstd::bulk::decompress(&comp, f.raw_len as usize)?);
        let mut c = self.cache.lock();
        if c.len() >= 16 {
            c.remove(0);
        }
        c.push((i, d.clone()));
        Ok(d)
    }
    pub fn frame_at(&self, off: u64) -> usize {
        self.frames.partition_point(|f| f.raw + f.raw_len as u64 <= off)
    }
    pub fn read(&self, start: u64, end: u64) -> Result<Vec<u8>> {
        let end = end.min(self.len());
        let mut out = Vec::with_capacity(end.saturating_sub(start) as usize);
        if start >= end {
            return Ok(out);
        }
        let mut i = self.frame_at(start);
        while i < self.frames.len() && self.frames[i].raw < end {
            let f = self.frames[i];
            let d = self.frame(i)?;
            let a = start.saturating_sub(f.raw) as usize;
            let b = ((end - f.raw) as usize).min(d.len());
            out.extend_from_slice(&d[a..b]);
            i += 1;
        }
        Ok(out)
    }
}

// ── session writer ──────────────────────────────────────────────────────

pub struct SessionWriter {
    pub dir: PathBuf,
    pub meta: SessionMeta,
    raw: LogWriter,
    text: LogWriter,
    cat: File,
    next_id: u64,
    open: Vec<u64>,
    pub rows: Vec<Row>,
    cur: Option<u64>,
    cur_lines: u64,
    marks: File,
}

/// Every LINE_MARK-th line of a row, its absolute text offset is recorded in `lines.bin` for instant seeks.
pub const LINE_MARK: u64 = 4096;

pub type SharedWriter = Arc<Mutex<SessionWriter>>;

impl SessionWriter {
    pub fn create(root: &Path, meta_pills: Vec<(String, String)>, host: &str) -> Result<Self> {
        let now = Local::now();
        let stamp = now.format("%m.%d.%y_%I.%M.%S%p").to_string().to_lowercase();
        let id = format!("ember_{}_{}_{}", host.split('.').next().unwrap_or(host), stamp, std::process::id());
        let dir = root.join("sessions").join(now.format("%Y/%m").to_string()).join(&id);
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let meta = SessionMeta { id, host: host.to_string(), pills: meta_pills, started: Some(now), ..Default::default() };
        fs::write(dir.join("meta.json"), serde_json::to_vec_pretty(&meta)?)?;
        Self::open_dir(dir, meta)
    }
    fn open_dir(dir: PathBuf, meta: SessionMeta) -> Result<Self> {
        let raw = LogWriter::open(&dir, "raw", 1)?;
        let text = LogWriter::open(&dir, "text", 1)?;
        let cat = OpenOptions::new().create(true).append(true).open(dir.join("catalog.jsonl"))?;
        let marks = OpenOptions::new().create(true).append(true).open(dir.join("lines.bin"))?;
        Ok(Self { dir, meta, raw, text, cat, next_id: 1, open: vec![], rows: vec![], cur: None, cur_lines: 0, marks })
    }
    fn op(&mut self, op: &CatOp) -> Result<()> {
        let mut l = serde_json::to_vec(op)?;
        l.push(b'\n');
        self.cat.write_all(&l)?;
        Ok(())
    }
    pub fn begin(&mut self, parent: Option<u64>, kind: RowKind, header: &str, command: &str, cwd: &str, mode: &str) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let (r, t) = (self.raw.len(), self.text.len());
        let row = Row {
            id,
            parent,
            kind,
            header: header.to_string(),
            ts: Local::now(),
            cwd: cwd.to_string(),
            mode: mode.to_string(),
            raw: (r, r),
            text: (t, t),
            rc: None,
            open: true,
            deleted: false,
            command: command.to_string(),
            lines: 0,
        };
        self.cur = Some(id);
        self.cur_lines = 0;
        self.op(&CatOp::Begin { row: row.clone() })?;
        self.rows.push(row);
        self.open.push(id);
        Ok(id)
    }
    pub fn raw(&mut self, b: &[u8]) -> Result<()> {
        self.raw.append(b)
    }
    pub fn text(&mut self, b: &[u8]) -> Result<()> {
        let base = self.text.len();
        if let Some(id) = self.cur {
            for i in memchr::memchr_iter(b'\n', b) {
                self.cur_lines += 1;
                if self.cur_lines % LINE_MARK == 0 {
                    let mut rec = [0u8; 24];
                    rec[0..8].copy_from_slice(&id.to_le_bytes());
                    rec[8..16].copy_from_slice(&self.cur_lines.to_le_bytes());
                    rec[16..24].copy_from_slice(&(base + i as u64 + 1).to_le_bytes());
                    self.marks.write_all(&rec)?;
                }
            }
        }
        self.text.append(b)
    }
    pub fn end(&mut self, id: u64) -> Result<()> {
        let (re, te) = (self.raw.len(), self.text.len());
        let lines = if self.cur == Some(id) { self.cur_lines } else { 0 };
        if let Some(r) = self.rows.iter_mut().rev().find(|r| r.id == id) {
            r.raw.1 = re;
            r.text.1 = te;
            r.open = false;
            r.lines = lines;
        }
        self.open.retain(|x| *x != id);
        if self.cur == Some(id) {
            self.cur = None;
        }
        self.op(&CatOp::End { id, raw_end: re, text_end: te, lines })
    }
    pub fn set_rc(&mut self, ids: &[u64], rc: i32) -> Result<()> {
        for r in self.rows.iter_mut().rev().take(ids.len() + 64) {
            if ids.contains(&r.id) {
                r.rc = Some(rc);
            }
        }
        self.op(&CatOp::Rc { ids: ids.to_vec(), rc })
    }
    pub fn set_header(&mut self, id: u64, header: &str) -> Result<()> {
        self.op(&CatOp::Header { id, header: header.to_string() })
    }
    /// Makes buffered bytes visible to readers (live view). Called on a short timer by the recorder.
    pub fn flush(&mut self) -> Result<()> {
        self.raw.flush(true)?;
        self.text.flush(true)?;
        self.cat.flush()?;
        self.marks.flush()?;
        Ok(())
    }
    pub fn close(&mut self) -> Result<()> {
        for id in self.open.clone() {
            self.end(id)?;
        }
        self.flush()?;
        self.raw.sync()?;
        self.text.sync()?;
        self.meta.ended = Some(Local::now());
        fs::write(self.dir.join("meta.json"), serde_json::to_vec_pretty(&self.meta)?)?;
        Ok(())
    }
}

// ── reading ─────────────────────────────────────────────────────────────

pub struct SessionReader {
    pub dir: PathBuf,
    pub meta: SessionMeta,
    pub rows: Vec<Row>,
    pub raw: LogReader,
    pub text: LogReader,
    cat_off: u64,
    pos: HashMap<u64, usize>,
}

impl SessionReader {
    pub fn open(dir: &Path) -> Result<Self> {
        let meta: SessionMeta = serde_json::from_slice(&fs::read(dir.join("meta.json"))?)?;
        let mut s = Self {
            dir: dir.to_path_buf(),
            meta,
            rows: vec![],
            raw: LogReader::open(dir, "raw")?,
            text: LogReader::open(dir, "text")?,
            cat_off: 0,
            pos: HashMap::new(),
        };
        s.refresh()?;
        Ok(s)
    }
    /// Picks up rows and bytes appended since the last call (live sessions grow in place).
    pub fn refresh(&mut self) -> Result<bool> {
        self.raw.refresh()?;
        self.text.refresh()?;
        let mut f = File::open(self.dir.join("catalog.jsonl"))?;
        f.seek(SeekFrom::Start(self.cat_off))?;
        let mut rd = BufReader::new(f);
        let mut line = String::new();
        let mut changed = false;
        loop {
            line.clear();
            let n = rd.read_line(&mut line)?;
            if n == 0 || !line.ends_with('\n') {
                break;
            }
            self.cat_off += n as u64;
            changed = true;
            let Ok(op) = serde_json::from_str::<CatOp>(&line) else { continue };
            match op {
                CatOp::Begin { row } => {
                    self.pos.insert(row.id, self.rows.len());
                    self.rows.push(row);
                }
                CatOp::End { id, raw_end, text_end, lines } => {
                    if let Some(r) = self.pos.get(&id).map(|&i| &mut self.rows[i]) {
                        r.raw.1 = raw_end;
                        r.text.1 = text_end;
                        r.open = false;
                        r.lines = lines;
                    }
                }
                CatOp::Rc { ids, rc } => {
                    for id in ids {
                        if let Some(&i) = self.pos.get(&id) {
                            self.rows[i].rc = Some(rc);
                        }
                    }
                }
                CatOp::Delete { id } => {
                    if let Some(&i) = self.pos.get(&id) {
                        self.rows[i].deleted = true;
                    }
                }
                CatOp::Header { id, header } => {
                    if let Some(&i) = self.pos.get(&id) {
                        self.rows[i].header = header;
                    }
                }
            }
        }
        Ok(changed)
    }
    /// (line number, absolute text offset) marks of a row, ascending.
    pub fn line_marks(&self, row: u64) -> Vec<(u64, u64)> {
        let b = fs::read(self.dir.join("lines.bin")).unwrap_or_default();
        b.chunks_exact(24)
            .filter(|c| u64::from_le_bytes(c[0..8].try_into().unwrap()) == row)
            .map(|c| (u64::from_le_bytes(c[8..16].try_into().unwrap()), u64::from_le_bytes(c[16..24].try_into().unwrap())))
            .collect()
    }
    /// Lines in a row; for a live row, counted from its last mark to the current end.
    pub fn row_lines(&self, r: &Row, marks: &[(u64, u64)]) -> u64 {
        if !r.open {
            return r.lines;
        }
        let (l0, o0) = marks.last().copied().unwrap_or((0, r.text.0));
        let tail = self.text.read(o0, self.row_end(r).1).unwrap_or_default();
        l0 + memchr::memchr_iter(b'\n', &tail).count() as u64
    }
    /// Absolute text offset where line `k` (0-based) of the row starts.
    pub fn line_offset(&self, r: &Row, marks: &[(u64, u64)], k: u64) -> u64 {
        if k == 0 {
            return r.text.0;
        }
        let i = marks.partition_point(|m| m.0 <= k);
        let (mut l, mut off) = if i == 0 { (0, r.text.0) } else { marks[i - 1] };
        let end = self.row_end(r).1;
        while l < k && off < end {
            let chunk = self.text.read(off, (off + (1 << 20)).min(end)).unwrap_or_default();
            if chunk.is_empty() {
                break;
            }
            let mut advanced = false;
            for p in memchr::memchr_iter(b'\n', &chunk) {
                l += 1;
                if l == k {
                    return off + p as u64 + 1;
                }
                advanced = true;
            }
            let _ = advanced;
            off += chunk.len() as u64;
        }
        off.min(end)
    }
    /// Line number (0-based) containing an offset within the row.
    pub fn line_of(&self, r: &Row, marks: &[(u64, u64)], offset_in_row: u64) -> u64 {
        let abs = r.text.0 + offset_in_row;
        let i = marks.partition_point(|m| m.1 <= abs);
        let (l, off) = if i == 0 { (0, r.text.0) } else { marks[i - 1] };
        let b = self.text.read(off, abs).unwrap_or_default();
        l + memchr::memchr_iter(b'\n', &b).count() as u64
    }
    pub fn row_end(&self, r: &Row) -> (u64, u64) {
        if r.open {
            (self.raw.len().max(r.raw.0), self.text.len().max(r.text.0))
        } else {
            (r.raw.1, r.text.1)
        }
    }
    pub fn row_text(&self, r: &Row) -> Result<Vec<u8>> {
        self.text.read(r.text.0, self.row_end(r).1)
    }
    pub fn row_raw(&self, r: &Row) -> Result<Vec<u8>> {
        self.raw.read(r.raw.0, self.row_end(r).0)
    }
    pub fn visible_rows(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(|r| !r.deleted)
    }
    /// Removes a row from the report (append-only tombstone; stored bytes are untouched).
    pub fn delete_row(&mut self, id: u64) -> Result<()> {
        let mut f = OpenOptions::new().append(true).open(self.dir.join("catalog.jsonl"))?;
        let mut l = serde_json::to_vec(&CatOp::Delete { id })?;
        l.push(b'\n');
        f.write_all(&l)?;
        if let Some(&i) = self.pos.get(&id) {
            self.rows[i].deleted = true;
        }
        Ok(())
    }
    pub fn set_pinned(&mut self, pinned: bool) -> Result<()> {
        self.meta.pinned = pinned;
        fs::write(self.dir.join("meta.json"), serde_json::to_vec_pretty(&self.meta)?)?;
        Ok(())
    }
    pub fn size_on_disk(&self) -> u64 {
        fs::read_dir(&self.dir).map(|d| d.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()).unwrap_or(0)
    }
}

/// All session directories under the data root, oldest first.
pub fn list_sessions(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    fn walk(p: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(rd) = fs::read_dir(p) else { return };
        for e in rd.flatten() {
            let path = e.path();
            if path.join("meta.json").exists() {
                out.push(path);
            } else if depth < 3 && path.is_dir() {
                walk(&path, depth + 1, out);
            }
        }
    }
    walk(&root.join("sessions"), 0, &mut out);
    out.sort_by_key(|p| fs::metadata(p.join("meta.json")).and_then(|m| m.created().or(m.modified())).ok());
    out
}

/// Rewrites a closed session's streams with tighter compression and larger frames. Offsets are unchanged.
pub fn recompress(dir: &Path, level: i32) -> Result<()> {
    for name in ["raw", "text"] {
        let rd = LogReader::open(dir, name)?;
        let tmp = dir.join("recompress.tmp");
        fs::create_dir_all(&tmp)?;
        {
            let mut w = LogWriter::open(&tmp, name, level)?;
            for i in 0..rd.frames.len() {
                w.buf.extend_from_slice(&rd.frame(i)?);
                if w.buf.len() >= 4 * FRAME_TARGET {
                    w.flush(false)?;
                }
            }
            w.flush(true)?;
            w.sync()?;
        }
        fs::rename(tmp.join(format!("{name}.zlog")), dir.join(format!("{name}.zlog")))?;
        fs::rename(tmp.join(format!("{name}.idx")), dir.join(format!("{name}.idx")))?;
        fs::remove_dir_all(&tmp)?;
    }
    Ok(())
}

/// Size and age retention. Pinned sessions are never pruned. Returns removed directories.
pub fn prune(root: &Path, max_bytes: u64, max_age_days: u64) -> Result<Vec<PathBuf>> {
    let mut removed = vec![];
    let sessions = list_sessions(root);
    let mut info: Vec<(PathBuf, SessionMeta, u64)> = sessions
        .into_iter()
        .filter_map(|d| {
            let m: SessionMeta = serde_json::from_slice(&fs::read(d.join("meta.json")).ok()?).ok()?;
            let size = fs::read_dir(&d).ok()?.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum();
            Some((d, m, size))
        })
        .collect();
    let now = Local::now();
    let mut total: u64 = info.iter().map(|x| x.2).sum();
    info.retain(|(d, m, size)| {
        let old = max_age_days > 0 && m.started.map(|s| (now - s).num_days() as u64 > max_age_days).unwrap_or(false);
        if old && !m.pinned && m.ended.is_some() {
            if fs::remove_dir_all(d).is_ok() {
                total -= size;
                removed.push(d.clone());
            }
            return false;
        }
        true
    });
    for (d, m, size) in info {
        if max_bytes == 0 || total <= max_bytes {
            break;
        }
        if m.pinned || m.ended.is_none() {
            continue;
        }
        if fs::remove_dir_all(&d).is_ok() {
            total -= size;
            removed.push(d);
        }
    }
    Ok(removed)
}
