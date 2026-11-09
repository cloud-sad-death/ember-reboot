//! Headless Ember: record a zsh session in this terminal, then browse, search and export history.
use anyhow::{bail, Result};
use chrono::{Local, NaiveDate, TimeZone};
use clap::{Parser, Subcommand};
use ember_core::export::{display_time, export_html, export_text, strip_date};
use ember_core::pty::{Pane, SpawnOpts};
use ember_core::recorder::{Mode, RecorderConfig};
use ember_core::search::{search, Filters, Query};
use ember_core::settings::Settings;
use ember_core::store::{list_sessions, SessionReader};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "ember-cli", about = "Ember recorder, search and export")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run your zsh in this terminal and record it
    Record {
        #[arg(long, default_value = "standard")]
        mode: String,
    },
    /// List sessions
    Sessions,
    /// List rows of a session (id, time, header)
    Rows { session: String },
    /// Print one row's output
    Show {
        session: String,
        row: u64,
        #[arg(long)]
        raw: bool,
    },
    /// Search all history (plain text by default)
    Search {
        pattern: String,
        #[arg(long)]
        regex: bool,
        #[arg(long)]
        words: bool,
        #[arg(long)]
        case: bool,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long)]
        mode: Option<String>,
        #[arg(long = "cmd")]
        command: Option<String>,
        #[arg(long)]
        failed: bool,
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        until: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Export a session to HTML + JSON (or text)
    Export {
        session: String,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        text: bool,
        #[arg(long)]
        ansi: bool,
        #[arg(long)]
        row: Option<u64>,
    },
    /// Build the keyword index (all sessions when none given)
    Index { session: Option<String> },
    /// Recompress closed sessions with tighter settings
    Recompress,
    /// Apply retention limits from settings
    Prune,
    /// Synthetic throughput benchmark: write N MB through the recorder, then search it
    Bench {
        #[arg(long, default_value_t = 1024)]
        mb: u64,
    },
}

fn settings() -> Settings {
    Settings::load(&ember_core::paths::config_file())
}

fn find_session(root: &std::path::Path, key: &str) -> Result<PathBuf> {
    let all = list_sessions(root);
    if key == "last" {
        return all.last().cloned().ok_or_else(|| anyhow::anyhow!("no sessions"));
    }
    all.into_iter().find(|p| p.file_name().map_or(false, |n| n.to_string_lossy().contains(key))).ok_or_else(|| anyhow::anyhow!("no session matching {key}"))
}

fn date(s: &str) -> Result<chrono::DateTime<Local>> {
    let d = NaiveDate::parse_from_str(s, "%Y-%m-%d")?;
    Ok(Local.from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap()).single().unwrap())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let st = settings();
    let root = st.data_root();
    match cli.cmd {
        Cmd::Record { mode } => record(&st, &mode)?,
        Cmd::Sessions => {
            for d in list_sessions(&root) {
                if let Ok(s) = SessionReader::open(&d) {
                    println!("{}  {:>6} rows  {:>10} bytes  {}{}", s.meta.id, s.rows.len(), s.size_on_disk(), s.meta.host, if s.meta.pinned { "  pinned" } else { "" });
                }
            }
        }
        Cmd::Rows { session } => {
            let s = SessionReader::open(&find_session(&root, &session)?)?;
            for r in s.visible_rows() {
                let ind = if r.parent.is_some() { "  " } else { "" };
                println!("{:>6} {} {}  {ind}{}{}", r.id, strip_date(&r.ts), display_time(&r.ts), r.header, r.rc.filter(|c| *c != 0).map(|c| format!("  [exit {c}]")).unwrap_or_default());
            }
        }
        Cmd::Show { session, row, raw } => {
            let s = SessionReader::open(&find_session(&root, &session)?)?;
            let r = s.rows.iter().find(|r| r.id == row).ok_or_else(|| anyhow::anyhow!("no row {row}"))?;
            let b = if raw { s.row_raw(r)? } else { s.row_text(r)? };
            std::io::stdout().write_all(&b)?;
        }
        Cmd::Search { pattern, regex, words, case, session, host, cwd, mode, command, failed, since, until, limit } => {
            let mut q = Query::new(&pattern);
            q.regex = regex;
            q.words = words;
            q.ignore_case = !case;
            q.limit = limit;
            q.filters = Filters { host, cwd, mode, command, failed: failed.then_some(true), since: since.map(|s| date(&s)).transpose()?, until: until.map(|s| date(&s)).transpose()? };
            if let Some(s) = session {
                q.sessions = Some(vec![find_session(&root, &s)?]);
            }
            let (hits, stats) = search(&root, &q)?;
            for h in &hits {
                println!("{}  row {:>5}  {} {}  {}\n    {}", h.session_id, h.row_id, strip_date(&h.ts), display_time(&h.ts), h.header, h.line.trim_end());
            }
            eprintln!("{} hits · {} sessions · {} MB scanned · {} ms{}", hits.len(), stats.sessions, stats.bytes / 1_000_000, stats.millis, if stats.used_index { " · index" } else { "" });
        }
        Cmd::Export { session, out, text, ansi, row } => {
            let dir = find_session(&root, &session)?;
            let s = SessionReader::open(&dir)?;
            let items: Vec<(usize, usize)> = (0..s.rows.len()).filter(|&i| !s.rows[i].deleted && row.map_or(true, |r| s.rows[i].id == r)).map(|i| (0, i)).collect();
            let out = out.unwrap_or_else(|| root.join("exports"));
            let base = s.meta.id.clone();
            let ss = vec![s];
            if text || ansi {
                std::fs::create_dir_all(&out)?;
                let p = out.join(format!("{base}.{}", if ansi { "ansi" } else { "txt" }));
                export_text(&ss, &items, &p, ansi)?;
                println!("-> {}", p.display());
            } else {
                let r = export_html(&ss, &items, &out, &base, &st)?;
                for p in &r.html {
                    println!("-> {}", p.display());
                }
                println!("saved {} entries\n  json: {}", r.rows, r.json.last().unwrap().display());
            }
        }
        Cmd::Index { session } => {
            let dirs = match session {
                Some(s) => vec![find_session(&root, &s)?],
                None => list_sessions(&root),
            };
            for d in dirs {
                let n = ember_core::index::build(&d, st.index_stream_max_mb_per_row * 1_000_000)?;
                println!("{}: {n} words", d.display());
            }
        }
        Cmd::Recompress => {
            for d in list_sessions(&root) {
                let s = SessionReader::open(&d)?;
                if s.meta.ended.is_some() {
                    let before = s.size_on_disk();
                    ember_core::store::recompress(&d, st.recompress_level)?;
                    println!("{}: {} -> {} bytes", s.meta.id, before, SessionReader::open(&d)?.size_on_disk());
                }
            }
        }
        Cmd::Prune => {
            for d in ember_core::store::prune(&root, (st.retention_max_gb * 1e9) as u64, st.retention_max_days)? {
                println!("removed {}", d.display());
            }
        }
        Cmd::Bench { mb } => bench(&root, mb)?,
    }
    Ok(())
}

fn record(st: &Settings, mode: &str) -> Result<()> {
    let mode = match mode {
        "standard" => Mode::Standard,
        "streaming" => Mode::Streaming,
        _ => bail!("mode is standard or streaming"),
    };
    let root = st.data_root();
    let shell_dir = ember_core::shell::install(&root)?;
    let (rows, cols) = term_size();
    let cfg = RecorderConfig { root, host: ember_core::host::hostname(), pills: ember_core::host::pills(), slice_interval: Duration::from_millis(st.slice_interval_ms), slice_on_refresh: st.slice_on_refresh, index_max: st.index_stream_max_mb_per_row * 1_000_000, recompress_level: st.recompress_level };
    let done = Arc::new(AtomicBool::new(false));
    let d2 = done.clone();
    let mut pane = Pane::spawn(
        SpawnOpts { shell: &st.shell, cwd: std::env::current_dir().ok(), shell_dir: &shell_dir, rows, cols, extra_env: vec![] },
        cfg,
        mode,
        |b| {
            let mut o = std::io::stdout().lock();
            let _ = o.write_all(b);
            let _ = o.flush();
        },
        move || d2.store(true, Ordering::SeqCst),
    )?;
    let saved = raw_mode();
    let mut stdin = std::io::stdin();
    let mut buf = [0u8; 4096];
    let mut last = (rows, cols);
    while !done.load(Ordering::SeqCst) {
        let mut pfd = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
        let n = unsafe { libc::poll(&mut pfd, 1, 100) };
        let sz = term_size();
        if sz != last {
            pane.resize(sz.0, sz.1);
            last = sz;
        }
        if n > 0 {
            let k = stdin.read(&mut buf)?;
            if k == 0 {
                break;
            }
            let _ = pane.write(&buf[..k]);
        }
    }
    restore(saved);
    let rec = pane.recorder.clone();
    for _ in 0..100 {
        if rec.lock().writer.is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if let Some(d) = list_sessions(&st.data_root()).last() {
        eprintln!("ember: session saved in {}", d.display());
    }
    Ok(())
}

fn term_size() -> (u16, u16) {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) } == 0 && ws.ws_row > 0 {
        (ws.ws_row, ws.ws_col)
    } else {
        (24, 80)
    }
}

fn raw_mode() -> Option<libc::termios> {
    unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(0, &mut t) != 0 {
            return None;
        }
        let saved = t;
        libc::cfmakeraw(&mut t);
        libc::tcsetattr(0, libc::TCSANOW, &t);
        Some(saved)
    }
}

fn restore(t: Option<libc::termios>) {
    if let Some(t) = t {
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &t) };
    }
}

fn bench(root: &std::path::Path, mb: u64) -> Result<()> {
    use ember_core::osc::{Event, OscFilter, Piece};
    use ember_core::recorder::Recorder;
    let cfg = RecorderConfig { root: root.to_path_buf(), host: "bench".into(), pills: vec![], slice_interval: Duration::from_secs(5), slice_on_refresh: true, index_max: 0, recompress_level: 0 };
    let mut rec = Recorder::new(cfg, Mode::Standard);
    let mut line = 0u64;
    let mut chunk = Vec::with_capacity(1 << 16);
    let mut filter = OscFilter::default();
    let t0 = std::time::Instant::now();
    rec.handle(Piece::Event(Event::Preexec { typed: "bench-gen".into(), full: "bench-gen".into(), cwd: "/".into(), is_loop: false, body: String::new() }))?;
    let total = mb << 20;
    let mut written = 0u64;
    let (mut display, mut pieces) = (Vec::with_capacity(1 << 16), Vec::new());
    while written < total {
        chunk.clear();
        while chunk.len() < 60_000 {
            line += 1;
            let _ = write!(chunk, "\x1b[32m{line:>10}\x1b[0m proc{} pid={} net en0 rx={} tx={} state=ESTABLISHED needle{}\r\n", line % 97, line % 65535, line * 7, line * 3, line % 1_000_003);
        }
        display.clear();
        pieces.clear();
        filter.feed(&chunk, &mut display, &mut pieces);
        for p in pieces.drain(..) {
            rec.handle(p)?;
        }
        written += chunk.len() as u64;
    }
    rec.handle(Piece::Event(Event::Done(0)))?;
    let dir = rec.session_dir().unwrap();
    rec.close()?;
    let secs = t0.elapsed().as_secs_f64();
    let disk = SessionReader::open(&dir)?.size_on_disk();
    println!("recorded {mb} MB in {secs:.2}s = {:.0} MB/s; on disk {} MB ({:.1}x)", mb as f64 / secs, disk >> 20, (total * 2) as f64 / disk as f64);
    let t1 = std::time::Instant::now();
    let s = SessionReader::open(&dir)?;
    let r = s.rows.last().unwrap();
    let mid = (r.text.0 + r.text.1) / 2;
    let _ = s.text.read(mid, mid + 200_000)?;
    println!("open + random seek into the middle: {} ms", t1.elapsed().as_millis());
    let mut q = Query::new("needle999999");
    q.sessions = Some(vec![dir.clone()]);
    let (hits, st) = search(root, &q)?;
    println!("search: {} hits, {} MB scanned in {} ms ({:.0} MB/s)", hits.len(), st.bytes >> 20, st.millis, (st.bytes >> 20) as f64 / (st.millis.max(1) as f64 / 1000.0));
    Ok(())
}
