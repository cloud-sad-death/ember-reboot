use ember_core::export::{export_html, export_text};
use ember_core::search::{search, Query};
use ember_core::settings::Settings;
use ember_core::store::{list_sessions, recompress, RowKind, SessionReader, SessionWriter};

fn make(root: &std::path::Path) -> std::path::PathBuf {
    let mut w = SessionWriter::create(root, vec![("ZSH".into(), "5.9".into())], "mac.local").unwrap();
    let a = w.begin(None, RowKind::Command, "echo hi", "echo hi", "/Users/me", "standard").unwrap();
    w.raw(b"\x1b[31mhi\x1b[0m\r\n").unwrap();
    w.text(b"hi\n").unwrap();
    w.end(a).unwrap();
    w.set_rc(&[a], 0).unwrap();
    let b = w.begin(None, RowKind::Item, "cat /t/big.log", "qcat", "/Users/me", "standard").unwrap();
    for i in 0..60_000 {
        let l = format!("line {i} payload <tag> & stuff ErrorCode{}\n", i % 1000);
        w.raw(l.as_bytes()).unwrap();
        w.text(l.as_bytes()).unwrap();
    }
    w.end(b).unwrap();
    w.set_rc(&[b], 2).unwrap();
    let c = w.begin(None, RowKind::Command, "rm junk", "rm junk", "/tmp", "standard").unwrap();
    w.text(b"secret-to-delete\n").unwrap();
    w.end(c).unwrap();
    let dir = w.dir.clone();
    w.close().unwrap();
    dir
}

#[test]
fn store_search_export_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let dir = make(root);
    assert_eq!(list_sessions(root).len(), 1);
    let mut s = SessionReader::open(&dir).unwrap();
    assert_eq!(s.rows.len(), 3);
    assert_eq!(s.row_raw(&s.rows[0]).unwrap(), b"\x1b[31mhi\x1b[0m\r\n");
    assert_eq!(s.rows[1].rc, Some(2));
    let t = s.row_text(&s.rows[1]).unwrap();
    assert!(t.starts_with(b"line 0 ") && t.ends_with(b"ErrorCode999\n"));
    let big = &s.rows[1];
    assert_eq!(big.lines, 60_000);
    let marks = s.line_marks(big.id);
    assert_eq!(marks.len(), 14);
    let off = s.line_offset(big, &marks, 50_001);
    assert!(s.text.read(off, off + 11).unwrap().starts_with(b"line 50001 "));
    assert_eq!(s.line_of(big, &marks, off - big.text.0 + 3), 50_001);
    s.delete_row(3).unwrap();
    assert!(SessionReader::open(&dir).unwrap().rows[2].deleted);

    let (h, _) = search(root, &Query::new("line 59999 ")).unwrap();
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].header, "cat /t/big.log");
    assert!(h[0].line.starts_with("line 59999"));
    let mut q = Query::new(r"ErrorCode99[0-9]$");
    q.regex = true;
    assert_eq!(search(root, &q).unwrap().0.len(), 600);
    assert!(search(root, &Query::new("secret-to-delete")).unwrap().0.is_empty(), "deleted rows are not searched");
    let mut f = Query::new("hi");
    f.filters.cwd = Some("/Users".into());
    f.filters.failed = Some(false);
    assert!(search(root, &f).unwrap().0.iter().all(|h| h.header == "echo hi"));

    ember_core::index::build(&dir, 0).unwrap();
    let mut w = Query::new("errorcode7");
    w.words = true;
    let (hw, st) = search(root, &w).unwrap();
    assert!(st.used_index);
    assert_eq!(hw.len(), 60);

    let s = SessionReader::open(&dir).unwrap();
    let items: Vec<_> = vec![(0, 1), (0, 0)];
    let mut st = Settings::default();
    st.export_part_mb = 1;
    let out = tmp.path().join("out");
    let r = export_html(&[s], &items, &out, "ember_test", &st).unwrap();
    assert_eq!(r.rows, 2);
    assert_eq!(r.html.len(), 2, "rotates between rows only");
    let p2 = std::fs::read_to_string(&r.html[1]).unwrap();
    assert!(p2.contains("class=\"row-cmd\">echo hi</div>") && p2.contains("BEGIN REPORT CONFIG"));
    let p1 = std::fs::read_to_string(&r.html[0]).unwrap();
    assert!(p1.contains("__u({\"parts\":2})") && p1.contains("&lt;tag&gt; &amp; stuff") && p1.contains("__rc([\"row-1\"],2)") && p1.contains("render-block"));
    let j: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&r.json[0]).unwrap()).unwrap();
    assert_eq!(j["entries"][0]["command"], "cat /t/big.log");

    let before = SessionReader::open(&dir).unwrap().row_text(&SessionReader::open(&dir).unwrap().rows[1]).unwrap();
    recompress(&dir, 9).unwrap();
    let s = SessionReader::open(&dir).unwrap();
    assert_eq!(s.row_text(&s.rows[1]).unwrap(), before);
    export_text(&[s], &items, &tmp.path().join("x.txt"), false).unwrap();
    assert!(std::fs::read_to_string(tmp.path().join("x.txt")).unwrap().contains("── "));
}
