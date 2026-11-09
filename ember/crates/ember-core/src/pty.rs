//! PTY ownership. The reader thread strips Ember markers, hands display bytes to the screen at once,
//! and passes ordered pieces to the recorder thread over a channel; the recorder never blocks typing.
use crate::osc::{OscFilter, Piece};
use crate::recorder::{Mode, Recorder, RecorderConfig};
use anyhow::{anyhow, Result};
use crossbeam_channel::{unbounded, Receiver, Sender};
use parking_lot::Mutex;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub enum RecMsg {
    Pieces(Vec<Piece>),
    Mode(Mode),
    Resize(u16, u16),
    Close,
}

pub struct Pane {
    pub master: Box<dyn MasterPty + Send>,
    pub writer: Box<dyn Write + Send>,
    pub child: Box<dyn Child + Send + Sync>,
    pub rec_tx: Sender<RecMsg>,
    pub recorder: Arc<Mutex<Recorder>>,
    /// Commands finished (D markers seen), recorded or not.
    pub dones: Arc<AtomicU64>,
}

pub struct SpawnOpts<'a> {
    pub shell: &'a str,
    pub cwd: Option<PathBuf>,
    pub shell_dir: &'a Path,
    pub rows: u16,
    pub cols: u16,
    pub extra_env: Vec<(String, String)>,
}

impl Pane {
    /// `on_output` receives display bytes on the PTY reader thread (feed them to the terminal engine).
    pub fn spawn(opts: SpawnOpts, rec_cfg: RecorderConfig, mode: Mode, mut on_output: impl FnMut(&[u8]) + Send + 'static, on_exit: impl FnOnce() + Send + 'static) -> Result<Self> {
        let pty = native_pty_system();
        let pair = pty.openpty(PtySize { rows: opts.rows, cols: opts.cols, pixel_width: 0, pixel_height: 0 }).map_err(|e| anyhow!("{e}"))?;
        let mut cmd = CommandBuilder::new(opts.shell);
        cmd.arg("-l");
        cmd.arg("-i");
        for (k, v) in crate::shell::env(opts.shell_dir).into_iter().chain(opts.extra_env) {
            cmd.env(k, v);
        }
        cmd.cwd(opts.cwd.or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("/")));
        let child = pair.slave.spawn_command(cmd).map_err(|e| anyhow!("{e}"))?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(|e| anyhow!("{e}"))?;
        let writer = pair.master.take_writer().map_err(|e| anyhow!("{e}"))?;
        let recorder = Arc::new(Mutex::new(Recorder::new(rec_cfg, mode)));
        recorder.lock().resize(opts.rows, opts.cols);
        let (tx, rx) = unbounded::<RecMsg>();
        let rtx = tx.clone();
        let dones = Arc::new(AtomicU64::new(0));
        let d2 = dones.clone();
        std::thread::Builder::new().name("ember-pty-read".into()).spawn(move || {
            let mut filter = OscFilter::default();
            let mut buf = vec![0u8; 1 << 16];
            let mut display = Vec::with_capacity(1 << 16);
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        display.clear();
                        let mut pieces = Vec::new();
                        filter.feed(&buf[..n], &mut display, &mut pieces);
                        if !display.is_empty() {
                            on_output(&display);
                        }
                        let n_done = pieces.iter().filter(|p| matches!(p, Piece::Event(crate::osc::Event::Done(_)))).count();
                        if !pieces.is_empty() {
                            let _ = rtx.send(RecMsg::Pieces(pieces));
                        }
                        if n_done > 0 {
                            d2.fetch_add(n_done as u64, Ordering::SeqCst);
                        }
                    }
                }
            }
            let _ = rtx.send(RecMsg::Close);
            on_exit();
        })?;
        let rec = recorder.clone();
        std::thread::Builder::new().name("ember-record".into()).spawn(move || record_loop(rec, rx))?;
        Ok(Self { master: pair.master, writer, child, rec_tx: tx, recorder, dones })
    }

    pub fn write(&mut self, b: &[u8]) -> Result<()> {
        self.writer.write_all(b)?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn resize(&self, rows: u16, cols: u16) {
        let _ = self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        let _ = self.rec_tx.send(RecMsg::Resize(rows, cols));
    }

    pub fn set_mode(&self, m: Mode) {
        let _ = self.rec_tx.send(RecMsg::Mode(m));
    }
}

fn record_loop(rec: Arc<Mutex<Recorder>>, rx: Receiver<RecMsg>) {
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(RecMsg::Pieces(p)) => {
                let mut r = rec.lock();
                for piece in p {
                    if let Err(e) = r.handle(piece) {
                        eprintln!("ember: recorder: {e}");
                    }
                }
                let _ = r.tick();
            }
            Ok(RecMsg::Mode(m)) => rec.lock().set_mode(m),
            Ok(RecMsg::Resize(a, b)) => rec.lock().resize(a, b),
            Ok(RecMsg::Close) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                let _ = rec.lock().close();
                break;
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                let _ = rec.lock().tick();
            }
        }
    }
}
