//! Per-pane recorder. Runs on its own thread, fed by the PTY reader, so typing never waits on disk.
use crate::loopdetect::{is_per_item_loop, resolve_loop_verb};
use crate::osc::{Event, Piece};
use crate::slicer::{LoopSlicer, Op};
use crate::store::{RowKind, SessionWriter, SharedWriter};
use crate::stream::StreamSlicer;
use anyhow::Result;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Off,
    Standard,
    Streaming,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Standard => "standard",
            Mode::Streaming => "streaming",
        }
    }
}

#[derive(Clone)]
pub struct RecorderConfig {
    pub root: PathBuf,
    pub host: String,
    pub pills: Vec<(String, String)>,
    pub slice_interval: Duration,
    pub slice_on_refresh: bool,
    /// Bytes per streaming row that go into the keyword index (0 = everything).
    pub index_max: u64,
    /// Background recompression level once a session closes (0 = off).
    pub recompress_level: i32,
}

enum Active {
    Std(LoopSlicer),
    Stream(StreamSlicer),
}

struct Cmd {
    active: Active,
    typed: String,
    cwd: String,
    mode: Mode,
    ids: Vec<u64>,
    header_id: Option<u64>,
    current: Option<u64>,
}

pub struct Recorder {
    cfg: RecorderConfig,
    pub mode: Mode,
    pub writer: Option<SharedWriter>,
    cur: Option<Cmd>,
    size: (u16, u16),
    last_flush: Instant,
    pub commands: u64,
}

impl Recorder {
    pub fn new(cfg: RecorderConfig, mode: Mode) -> Self {
        Self { cfg, mode, writer: None, cur: None, size: (24, 80), last_flush: Instant::now(), commands: 0 }
    }

    /// Takes effect at the next command, so a row is never half recorded.
    pub fn set_mode(&mut self, m: Mode) {
        self.mode = m;
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.size = (rows, cols);
        if let Some(Cmd { active: Active::Stream(s), .. }) = &mut self.cur {
            s.resize(rows, cols);
        }
    }

    fn writer(&mut self) -> Result<SharedWriter> {
        if self.writer.is_none() {
            let w = SessionWriter::create(&self.cfg.root, self.cfg.pills.clone(), &self.cfg.host)?;
            self.writer = Some(Arc::new(Mutex::new(w)));
        }
        Ok(self.writer.clone().unwrap())
    }

    pub fn session_dir(&self) -> Option<PathBuf> {
        self.writer.as_ref().map(|w| w.lock().dir.clone())
    }

    pub fn handle(&mut self, piece: Piece) -> Result<()> {
        match piece {
            Piece::Data(d) => {
                if let Some(c) = &mut self.cur {
                    match &mut c.active {
                        Active::Std(s) => s.data(&d),
                        Active::Stream(s) => s.data(&d, Instant::now()),
                    }
                }
            }
            Piece::Event(Event::Preexec { typed, full, cwd, is_loop, body }) => {
                if self.cur.is_some() {
                    self.finish(None)?;
                }
                if self.mode == Mode::Off || matches!(typed.trim(), "exit" | "logout") {
                    return Ok(());
                }
                self.writer()?;
                let detect = if body.trim().is_empty() { full.clone() } else { body.clone() };
                let looped = is_loop || is_per_item_loop(&detect);
                let active = match self.mode {
                    Mode::Streaming => Active::Stream(StreamSlicer::new(&typed, self.cfg.slice_interval, self.cfg.slice_on_refresh, self.size.0, self.size.1)),
                    _ => Active::Std(LoopSlicer::new(&typed, looped, &if looped { resolve_loop_verb(&detect) } else { String::new() })),
                };
                self.commands += 1;
                self.cur = Some(Cmd { active, typed, cwd, mode: self.mode, ids: vec![], header_id: None, current: None });
            }
            Piece::Event(Event::Trace(t)) => {
                if let Some(Cmd { active: Active::Std(s), .. }) = &mut self.cur {
                    s.trace(&t)
                }
            }
            Piece::Event(Event::Item(p)) => {
                if let Some(Cmd { active: Active::Std(s), .. }) = &mut self.cur {
                    s.item(&p)
                }
            }
            Piece::Event(Event::Done(rc)) => {
                if self.cur.is_some() {
                    self.finish(Some(rc))?;
                }
            }
        }
        self.apply()
    }

    fn apply(&mut self) -> Result<()> {
        let Some(c) = &mut self.cur else { return Ok(()) };
        let ops = match &mut c.active {
            Active::Std(s) => std::mem::take(&mut s.ops),
            Active::Stream(s) => std::mem::take(&mut s.ops),
        };
        if ops.is_empty() {
            return Ok(());
        }
        let w = self.writer.clone().unwrap();
        let mut w = w.lock();
        for op in ops {
            match op {
                Op::Begin { kind, header } => {
                    let parent = if kind == RowKind::Slice { c.header_id } else { None };
                    let id = w.begin(parent, kind, &header, &c.typed, &c.cwd, c.mode.as_str())?;
                    if kind == RowKind::Command && matches!(c.active, Active::Stream(_)) {
                        c.header_id = Some(id);
                    }
                    c.ids.push(id);
                    c.current = Some(id);
                }
                Op::Raw(b) => w.raw(&b)?,
                Op::Text(b) => w.text(&b)?,
                Op::End => {
                    if let Some(id) = c.current.take() {
                        w.end(id)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn finish(&mut self, rc: Option<i32>) -> Result<()> {
        if let Some(c) = &mut self.cur {
            match &mut c.active {
                Active::Std(s) => s.finish(),
                Active::Stream(s) => s.finish(),
            }
        }
        self.apply()?;
        if let Some(c) = self.cur.take() {
            if let (Some(rc), Some(w)) = (rc, &self.writer) {
                w.lock().set_rc(&c.ids, rc)?;
            }
        }
        Ok(())
    }

    /// Time slicing and live flush; call every ~100–250 ms.
    pub fn tick(&mut self) -> Result<()> {
        if let Some(Cmd { active: Active::Stream(s), .. }) = &mut self.cur {
            s.tick(Instant::now());
        }
        self.apply()?;
        if self.last_flush.elapsed() >= Duration::from_millis(250) {
            if let Some(w) = &self.writer {
                w.lock().flush()?;
            }
            self.last_flush = Instant::now();
        }
        Ok(())
    }

    pub fn close(&mut self) -> Result<()> {
        if self.cur.is_some() {
            self.finish(None)?;
        }
        if let Some(w) = self.writer.take() {
            let dir = {
                let mut w = w.lock();
                w.close()?;
                w.dir.clone()
            };
            let (lvl, max) = (self.cfg.recompress_level, self.cfg.index_max);
            // background tiers: tighter compression, then the keyword index
            std::thread::spawn(move || {
                if lvl > 0 {
                    let _ = crate::store::recompress(&dir, lvl);
                }
                let _ = crate::index::build(&dir, max);
            });
        }
        Ok(())
    }
}
