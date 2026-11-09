//! Real zsh in a real PTY: the user's own config loads, markers stay invisible, loops split per target.
use ember_core::pty::{Pane, SpawnOpts};
use ember_core::recorder::{Mode, RecorderConfig};
use ember_core::store::{list_sessions, RowKind, SessionReader};
use parking_lot::Mutex;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

const ZSHRC: &str = r#"
PROMPT='%F{magenta}mine%f %~ %# '
alias ll='ls -la'
greet() { print -r -- "hello from my function" }
plup() {
  find $EMBER_T -name '*.plist' -print0 | sort -z | while IFS= read -r -d '' f
  do
    print -r -- $f
    cat $f
  done
}
qcat() { find $EMBER_T -name '*.query' -exec cat {} \; }
"#;

struct Run {
    out: Arc<Mutex<Vec<u8>>>,
    pane: Pane,
    root: std::path::PathBuf,
}

fn start(mode: Mode, home: &std::path::Path, extra: Vec<(String, String)>) -> Run {
    let root = home.join("data");
    let shell_dir = ember_core::shell::install(&root).unwrap();
    let out = Arc::new(Mutex::new(Vec::new()));
    let o2 = out.clone();
    let cfg = RecorderConfig { root: root.clone(), host: "testhost".into(), pills: vec![("ZSH".into(), "5.9".into())], slice_interval: Duration::from_millis(300), slice_on_refresh: true, index_max: 0, recompress_level: 0 };
    let mut env = vec![("HOME".to_string(), home.display().to_string())];
    env.extend(extra);
    let pane = Pane::spawn(
        SpawnOpts { shell: "zsh", cwd: Some(home.to_path_buf()), shell_dir: &shell_dir, rows: 30, cols: 120, extra_env: env },
        cfg,
        mode,
        move |b| o2.lock().extend_from_slice(b),
        || {},
    )
    .unwrap();
    Run { out, pane, root }
}

fn wait_for(cond: impl Fn() -> bool, secs: u64) {
    let t = Instant::now();
    while !cond() {
        assert!(t.elapsed() < Duration::from_secs(secs), "timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn run_cmds(r: &mut Run, cmds: &[&str]) {
    // first prompt
    wait_for(|| String::from_utf8_lossy(&r.out.lock()).contains("mine"), 20);
    // the integration installs on the first prompt; an empty line gives one more prompt cycle
    for c in cmds {
        let before = r.pane.dones.load(Ordering::SeqCst);
        r.pane.write(format!("{c}\r").as_bytes()).unwrap();
        wait_for(|| r.pane.dones.load(Ordering::SeqCst) > before, 30);
    }
    r.pane.write(b"exit\r").unwrap();
    let rec = r.pane.recorder.clone();
    wait_for(|| rec.lock().writer.is_none() && rec.lock().commands > 0, 20);
}

fn session(root: &std::path::Path) -> SessionReader {
    let s = list_sessions(root);
    assert_eq!(s.len(), 1, "one session");
    SessionReader::open(&s[0]).unwrap()
}

fn text(s: &SessionReader, h: &str) -> String {
    let r = s.rows.iter().find(|r| r.header == h).unwrap_or_else(|| panic!("no row {h}: {:?}", s.rows.iter().map(|r| &r.header).collect::<Vec<_>>()));
    String::from_utf8_lossy(&s.row_text(r).unwrap()).to_string()
}

#[test]
fn standard_mode_loops_and_config() {
    let home = tempfile::tempdir().unwrap();
    let t = home.path().join("target");
    std::fs::create_dir_all(&t).unwrap();
    std::fs::write(home.path().join(".zshrc"), ZSHRC).unwrap();
    for (n, body) in [("a", "AAA"), ("b", "BBB"), ("c", "CCC")] {
        std::fs::write(t.join(format!("{n}.query")), format!("{body}\n")).unwrap();
        std::fs::write(t.join(format!("{n}.plist")), format!("{{\n  \"k\" => [\n/nested/{n}\n  ]\n}}\n")).unwrap();
    }
    let mut r = start(Mode::Standard, home.path(), vec![("EMBER_T".into(), t.display().to_string())]);
    let ts = t.display().to_string();
    run_cmds(
        &mut r,
        &[
            "echo plain-output",
            "greet",
            &format!("find {ts} -name '*.query' -exec cat {{}} \\;"),
            &format!("for f in {ts}/*.query; do cat $f; done"),
            "qcat",
            "plup",
            "false",
            "ll >/dev/null",
        ],
    );
    let screen = String::from_utf8_lossy(&r.out.lock()).to_string();
    assert!(!screen.contains("7777"), "markers leaked to screen");
    assert!(screen.contains("AAA") && screen.contains("hello from my function"));
    let s = session(&r.root);
    let hs: Vec<&str> = s.rows.iter().map(|r| r.header.as_str()).collect();
    println!("{hs:#?}");
    assert!(text(&s, "echo plain-output").contains("plain-output"));
    assert!(text(&s, "greet").contains("hello from my function"));
    for n in ["a", "b", "c"] {
        let h = format!("cat {ts}/{n}.query");
        assert_eq!(hs.iter().filter(|x| **x == h).count(), 3, "find, for and qcat each give {h}");
        assert!(text(&s, &h).contains(&n.to_uppercase().repeat(3)));
        let p = format!("print -r -- {ts}/{n}.plist");
        assert!(hs.contains(&p.as_str()), "plup item {p}");
        assert!(text(&s, &p).contains(&format!("/nested/{n}")), "nested path stays inside its item");
    }
    let f = s.rows.iter().find(|r| r.header == "false").unwrap();
    assert_eq!(f.rc, Some(1));
    assert!(s.rows.iter().filter(|r| r.kind == RowKind::Item).all(|r| r.rc == Some(0)));
    let expected_cwd = std::fs::canonicalize(home.path()).unwrap().display().to_string();
    assert!(
        s.rows.iter().all(|r| r.cwd == expected_cwd),
        "cwd mismatch: expected {expected_cwd:?}, got {:?}",
        s.rows.iter().map(|r| r.cwd.clone()).collect::<Vec<_>>()
    );
}

#[test]
fn streaming_mode_slices() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join(".zshrc"), ZSHRC).unwrap();
    let mut r = start(Mode::Streaming, home.path(), vec![]);
    run_cmds(&mut r, &["for i in 1 2 3 4 5 6; do echo tick-$i; sleep 0.2; done", "for i in 1 2 3; do printf '\\e[H\\e[2Jscreen-%s\\n' $i; sleep 1; done"]);
    let s = session(&r.root);
    let head: Vec<_> = s.rows.iter().filter(|r| r.kind == RowKind::Command).collect();
    assert_eq!(head.len(), 2);
    let slices: Vec<_> = s.rows.iter().filter(|r| r.kind == RowKind::Slice && r.parent == Some(head[0].id)).collect();
    assert!(slices.len() >= 3, "line stream sliced: {}", slices.len());
    let all: String = slices.iter().map(|r| String::from_utf8_lossy(&s.row_text(r).unwrap()).to_string()).collect();
    for i in 1..=6 {
        assert!(all.contains(&format!("tick-{i}")));
    }
    let screens: Vec<_> = s.rows.iter().filter(|r| r.kind == RowKind::Slice && r.parent == Some(head[1].id)).collect();
    assert_eq!(screens.len(), 3, "one slice per refresh");
    for (i, r) in screens.iter().enumerate() {
        let t = String::from_utf8_lossy(&s.row_text(r).unwrap()).to_string();
        assert!(t.contains(&format!("screen-{}", i + 1)) && !t.contains('\x1b'), "{t:?}");
    }
}
