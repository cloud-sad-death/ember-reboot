//! Export: HTML in the blast30/stream2 format (part rotation only here), JSON alongside, plain/ANSI text.
use crate::settings::Settings;
use crate::store::{Row, SessionReader};
use anyhow::Result;
use chrono::{DateTime, Local, Timelike};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

const TEMPLATE: &str = include_str!("../assets/template.html");
const BUTTONS: &str = include_str!("../assets/buttons.html");
const META_BUTTONS: &str = include_str!("../assets/meta_buttons.html");
const FAVICON: &str = "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCA1MTIgNTEyIj48Y2lyY2xlIGN4PSIyNTYiIGN5PSIyNTYiIHI9IjIyNCIgZmlsbD0ibm9uZSIgc3Ryb2tlPSIjMDAwIiBzdHJva2Utd2lkdGg9IjMyIi8+PC9zdmc+";
const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#x27;"),
            _ => o.push(c),
        }
    }
    o
}

pub fn display_time(t: &DateTime<Local>) -> String {
    let h = t.hour() % 12;
    format!("{:02}:{:02} {}", if h == 0 { 12 } else { h }, t.minute(), if t.hour() < 12 { "AM" } else { "PM" })
}

pub fn strip_date(t: &DateTime<Local>) -> String {
    t.format("%b %d %y").to_string().to_uppercase()
}

fn display_date(t: &DateTime<Local>) -> String {
    format!("{} {} {}", MONTHS[t.format("%m").to_string().parse::<usize>().unwrap_or(1) - 1], t.format("%-d"), t.format("%Y"))
}

/// Streams a row's text in bounded chunks as valid UTF-8 (rows can be any size).
pub fn for_each_text(s: &SessionReader, r: &Row, mut f: impl FnMut(&str) -> Result<()>) -> Result<()> {
    let (a, b) = (r.text.0, s.row_end(r).1);
    let mut off = a;
    let mut carry: Vec<u8> = vec![];
    while off < b {
        let end = (off + (4 << 20)).min(b);
        let mut chunk = std::mem::take(&mut carry);
        chunk.extend(s.text.read(off, end)?);
        off = end;
        let valid = match std::str::from_utf8(&chunk) {
            Ok(_) => chunk.len(),
            Err(e) if e.error_len().is_none() && off < b => e.valid_up_to(),
            Err(_) => chunk.len(),
        };
        carry = chunk.split_off(valid);
        f(&String::from_utf8_lossy(&chunk))?;
    }
    if !carry.is_empty() {
        f(&String::from_utf8_lossy(&carry))?;
    }
    Ok(())
}

pub struct ExportOut {
    pub html: Vec<PathBuf>,
    pub json: Vec<PathBuf>,
    pub rows: usize,
}

struct Part {
    n: usize,
    f: BufWriter<File>,
    j: BufWriter<File>,
    size: u64,
    rows: usize,
    jfirst: bool,
    rc: Vec<(String, i32)>,
}

/// Writes `<dir>/<base>.html` (+ `_pN` parts) and JSON alongside, from (session, row) pairs in order.
pub fn export_html(sessions: &[SessionReader], items: &[(usize, usize)], dir: &Path, base: &str, settings: &Settings) -> Result<ExportOut> {
    std::fs::create_dir_all(dir)?;
    let first = sessions.first();
    let host = first.map(|s| s.meta.host.clone()).unwrap_or_default();
    let started = first.and_then(|s| s.meta.started).unwrap_or_else(Local::now);
    let pills = first.map(|s| s.meta.pills.clone()).unwrap_or_default();
    let host_markup: String = pills
        .iter()
        .map(|(k, v)| format!("<div class=\"kv\"><span class=\"k\">{}</span><span class=\"v\">{}</span></div>", esc(k), esc(if v.is_empty() { "Unknown" } else { v })))
        .collect();
    let (head_tpl, tail_tpl) = TEMPLATE.split_once("<!--ROWS-->").unwrap();
    let block = settings.report.output_block_chars.max(256) as usize;
    let collapse = settings.report.collapse_outputs_initially;
    let target = settings.export_part_mb.max(1) * 1_000_000;
    let stem = |n: usize| if n == 1 { base.to_string() } else { format!("{base}_p{n}") };
    let nav = |cur: usize, total: usize| {
        let links: String = (1..=total)
            .map(|i| format!("<a class=\"{}\" href=\"{}.html\"{} title=\"Part {i} of {total}\">{i}</a>", if i == cur { "current" } else { "" }, esc(&stem(i)), if i == cur { " aria-current=\"page\"" } else { "" }))
            .collect();
        format!("<nav class=\"parts\" aria-label=\"Report parts\" data-base=\"{}\" data-cur=\"{cur}\" data-total=\"{total}\" style=\"--part-columns:{}\">{links}</nav>", esc(base), total.min(10))
    };
    let mut out = ExportOut { html: vec![], json: vec![], rows: 0 };
    let open = |n: usize, out: &mut ExportOut| -> Result<Part> {
        let tokens = |k: &str| -> String {
            match k {
                "TITLE" => esc(&format!("ember: {host}")),
                "SCRIPTNAME" => "ember".into(),
                "TARGETNAME" => esc(&host),
                "HOSTINFO" => host_markup.clone(),
                "FAVICON" => FAVICON.into(),
                "DATE" => display_date(&started),
                "TIME" => display_time(&started),
                "STATSDECK" => String::new(),
                "CARDS" => "<p id=\"nofiles\">No commands recorded.</p>".into(),
                "PARTS" => nav(n, n),
                _ => String::new(),
            }
        };
        let re = regex::Regex::new(r"@@([A-Z]+)@@").unwrap();
        let head = re.replace_all(head_tpl, |c: &regex::Captures| tokens(&c[1]));
        let hp = dir.join(format!("{}.html", stem(n)));
        let jp = dir.join(format!("{}.json", stem(n)));
        let mut f = BufWriter::with_capacity(1 << 20, File::create(&hp)?);
        f.write_all(head.as_bytes())?;
        let mut j = BufWriter::new(File::create(&jp)?);
        let hdr = serde_json::json!({"run_id": base, "host": host, "part": n, "started": Local::now().to_rfc3339(), "description": ""}).to_string();
        write!(j, "{},\"entries\":[", &hdr[..hdr.len() - 1])?;
        out.html.push(hp);
        out.json.push(jp);
        Ok(Part { n, f, j, size: head.len() as u64, rows: 0, jfirst: true, rc: vec![] })
    };
    let close = |p: &mut Part, next: bool| -> Result<()> {
        let cfg = format!(
            "<div class=\"row metadata-item meta-row\" id=\"metadata-config-p{}\" data-copy-path=\"config\"><div class=\"row-hd meta-hd\"><span class=\"row-cmd\">config</span>{}</div><div class=\"row-output\"><div class=\"out\"><pre class=\"ln\">{}</pre></div></div></div>\n",
            p.n,
            META_BUTTONS,
            esc(&settings.report_config_text())
        );
        p.f.write_all(cfg.as_bytes())?;
        write!(p.f, "<script>__u({{\"parts\":{}}})</script>\n{}", if next { p.n + 1 } else { p.n }, tail_tpl)?;
        p.f.flush()?;
        let rc: serde_json::Map<String, serde_json::Value> = p.rc.iter().map(|(k, v)| (k.clone(), (*v).into())).collect();
        write!(p.j, "],\"exit_codes\":{},\"entry_count\":{},\"ended\":\"{}\"}}\n", serde_json::Value::Object(rc), p.rows, Local::now().to_rfc3339())?;
        p.j.flush()?;
        Ok(())
    };
    let mut part = open(1, &mut out)?;
    for (k, &(si, ri)) in items.iter().enumerate() {
        let s = &sessions[si];
        let r = &s.rows[ri];
        if part.rows > 0 && part.size >= target {
            close(&mut part, true)?;
            part = open(part.n + 1, &mut out)?;
        }
        let id = format!("row-{}", k + 1);
        let card = format!(
            "<div class=\"row{}\" id=\"{id}\" data-copy-path=\"{}\"><div class=\"row-hd\"><div class=\"row-strip\"><span class=\"row-ts\"><b class=\"ts-d\">{}</b>&nbsp;&nbsp;{}</span>{}</div><div class=\"row-cmd\">{}</div></div><div class=\"row-output\"><div class=\"out\"><pre class=\"ln\">",
            if collapse { " collapsed" } else { "" },
            esc(&r.header),
            esc(&strip_date(&r.ts)),
            esc(&display_time(&r.ts)),
            BUTTONS,
            esc(&r.header)
        );
        part.f.write_all(card.as_bytes())?;
        let entry = serde_json::json!({"id": id, "ts": r.ts.to_rfc3339(), "ts_display": display_time(&r.ts), "command": r.header}).to_string();
        write!(part.j, "{}{},\"output\":\"", if part.jfirst { "" } else { "," }, &entry[..entry.len() - 1])?;
        part.jfirst = false;
        let mut chars = 0usize;
        let mut blk = 0usize;
        let mut span = false;
        let mut size = card.len() as u64;
        for_each_text(s, r, |t| {
            for line in t.split_inclusive('\n') {
                let mut pre = "";
                if chars >= block && !span {
                    pre = "<span class=\"render-block\">";
                    span = true;
                    blk = 0;
                }
                chars += line.len();
                blk += line.len();
                let mut post = "";
                if span && blk >= block && line.ends_with('\n') {
                    post = "</span>";
                    span = false;
                }
                let e = esc(line);
                size += (e.len() + pre.len() + post.len()) as u64;
                part.f.write_all(pre.as_bytes())?;
                part.f.write_all(e.as_bytes())?;
                part.f.write_all(post.as_bytes())?;
                let js = serde_json::to_string(line)?;
                part.j.write_all(js[1..js.len() - 1].as_bytes())?;
            }
            Ok(())
        })?;
        write!(part.f, "{}</pre></div></div></div>\n", if span { "</span>" } else { "" })?;
        part.j.write_all(b"\"}")?;
        if let Some(rc) = r.rc {
            part.rc.push((id.clone(), rc));
            if rc != 0 {
                write!(part.f, "<script>__rc([\"{id}\"],{rc})</script>\n")?;
            }
        }
        part.size += size;
        part.rows += 1;
        out.rows += 1;
    }
    close(&mut part, false)?;
    Ok(out)
}

/// Plain text (or exact ANSI bytes) with a header line per row.
pub fn export_text(sessions: &[SessionReader], items: &[(usize, usize)], path: &Path, ansi: bool) -> Result<()> {
    let mut f = BufWriter::new(File::create(path)?);
    for &(si, ri) in items {
        let s = &sessions[si];
        let r = &s.rows[ri];
        writeln!(f, "── {} {}  {}  [{}]{}", strip_date(&r.ts), display_time(&r.ts), r.header, r.cwd, r.rc.map(|c| format!("  exit {c}")).unwrap_or_default())?;
        if ansi {
            let (a, b) = (r.raw.0, s.row_end(r).0);
            let mut off = a;
            while off < b {
                let e = (off + (4 << 20)).min(b);
                f.write_all(&s.raw.read(off, e)?)?;
                off = e;
            }
        } else {
            for_each_text(s, r, |t| Ok(f.write_all(t.as_bytes())?))?;
        }
        writeln!(f)?;
    }
    f.flush()?;
    Ok(())
}
