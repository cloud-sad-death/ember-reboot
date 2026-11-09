//! Search across every session: plain/regex/word, filters, results that open in the report view.
use crate::ui::{btn, pal, AppState, Input, InputEvent};
use ember_core::export::{display_time, export_html, strip_date};
use ember_core::search::{search, Filters, Hit, Query, Stats};
use ember_core::store::SessionReader;
use gpui::prelude::FluentBuilder;
use gpui::*;
use std::path::PathBuf;

pub struct SearchView {
    pub query: Entity<Input>,
    host: Entity<Input>,
    folder: Entity<Input>,
    command: Entity<Input>,
    since: Entity<Input>,
    until: Entity<Input>,
    regex: bool,
    words: bool,
    case: bool,
    failed: bool,
    mode: u8,
    hits: Vec<Hit>,
    stats: Option<Stats>,
    error: Option<String>,
    top: usize,
    busy: bool,
}

pub struct OpenHit(pub PathBuf, pub u64, pub u64);
impl EventEmitter<OpenHit> for SearchView {}

fn day(s: &str, end: bool) -> Option<chrono::DateTime<chrono::Local>> {
    use chrono::TimeZone;
    let d = chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()?;
    let t = if end { d.and_hms_opt(23, 59, 59)? } else { d.and_hms_opt(0, 0, 0)? };
    chrono::Local.from_local_datetime(&t).single()
}

impl SearchView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mk = |p: &str, cx: &mut Context<Self>| {
            let e = cx.new(|cx| Input::new(p, cx));
            cx.subscribe(&e, |v, _, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Submit) {
                    v.run(cx)
                }
            })
            .detach();
            e
        };
        Self {
            query: mk("Search all history (Enter)", cx),
            host: mk("host", cx),
            folder: mk("folder", cx),
            command: mk("command", cx),
            since: mk("from YYYY-MM-DD", cx),
            until: mk("to YYYY-MM-DD", cx),
            regex: false,
            words: false,
            case: false,
            failed: false,
            mode: 0,
            hits: vec![],
            stats: None,
            error: None,
            top: 0,
            busy: false,
        }
    }

    fn build(&self, cx: &App) -> Query {
        let t = |e: &Entity<Input>| {
            let s = e.read(cx).text.trim().to_string();
            (!s.is_empty()).then_some(s)
        };
        let mut q = Query::new(&self.query.read(cx).text);
        q.regex = self.regex;
        q.words = self.words;
        q.ignore_case = !self.case;
        q.limit = 100_000;
        q.filters = Filters {
            host: t(&self.host),
            cwd: t(&self.folder),
            command: t(&self.command),
            mode: match self.mode {
                1 => Some("standard".into()),
                2 => Some("streaming".into()),
                _ => None,
            },
            failed: self.failed.then_some(true),
            since: t(&self.since).and_then(|s| day(&s, false)),
            until: t(&self.until).and_then(|s| day(&s, true)),
        };
        q
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        let q = self.build(cx);
        if q.pattern.is_empty() {
            return;
        }
        let root = cx.global::<AppState>().root.clone();
        self.busy = true;
        let task = cx.background_spawn(async move { search(&root, &q) });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let r = task.await;
            let _ = this.update(cx, |v, cx| {
                v.busy = false;
                match r {
                    Ok((h, s)) => {
                        v.hits = h;
                        v.stats = Some(s);
                        v.error = None;
                    }
                    Err(e) => v.error = Some(e.to_string()),
                }
                v.top = 0;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn export(&mut self, cx: &mut Context<Self>) {
        let st = cx.global::<AppState>();
        let (settings, out) = (st.settings.clone(), st.root.join("exports"));
        let hits = self.hits.clone();
        let task = cx.background_spawn(async move {
            let mut dirs: Vec<PathBuf> = vec![];
            let mut pairs: Vec<(usize, u64)> = vec![];
            for h in &hits {
                let si = dirs.iter().position(|d| d == &h.session).unwrap_or_else(|| {
                    dirs.push(h.session.clone());
                    dirs.len() - 1
                });
                if !pairs.contains(&(si, h.row_id)) {
                    pairs.push((si, h.row_id));
                }
            }
            let ss: Vec<SessionReader> = dirs.iter().filter_map(|d| SessionReader::open(d).ok()).collect();
            let items: Vec<(usize, usize)> = pairs.iter().filter_map(|&(si, id)| ss.get(si)?.rows.iter().position(|r| r.id == id).map(|ri| (si, ri))).collect();
            let base = format!("ember_search_{}", chrono::Local::now().format("%m.%d.%y_%H%M%S"));
            let r = export_html(&ss, &items, &out, &base, &settings)?;
            anyhow::Ok(r.html[0].display().to_string())
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let r = task.await;
            let _ = this.update(cx, |v, cx| {
                v.error = Some(match r {
                    Ok(p) => format!("Exported → {p}"),
                    Err(e) => format!("Export failed: {e}"),
                });
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for SearchView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let font = cx.global::<AppState>().settings.font_family.clone();
        let tog = |id: &'static str, label: &str, on: bool| btn(id, label.to_string(), p).when(on, |d| d.bg(p.ink).text_color(p.paper));
        let n = ((f32::from(window.viewport_size().height) - 220.) / 52.).max(3.) as usize;
        let q = self.query.read(cx).text.clone();
        let results = self.hits.iter().enumerate().skip(self.top).take(n).map(|(i, h)| {
            let (s, e) = h.col;
            let line = h.line.clone();
            let (a, m, b) = (line.get(..s).unwrap_or("").to_string(), line.get(s..e).unwrap_or("").to_string(), line.get(e..).unwrap_or("").to_string());
            let (sess, row, off) = (h.session.clone(), h.row_id, h.offset_in_row);
            div()
                .id(("hit", i))
                .flex()
                .flex_col()
                .px_3()
                .py_1()
                .border_2()
                .border_color(p.ink)
                .cursor_pointer()
                .hover(move |d| d.bg(p.hit))
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(OpenHit(sess.clone(), row, off))))
                .child(div().flex().gap_3().text_size(px(11.)).text_color(p.muted).child(format!("{} {}", strip_date(&h.ts), display_time(&h.ts))).child(h.host.clone()).child(h.header.clone()))
                .child(div().flex().whitespace_nowrap().overflow_hidden().child(a).child(div().bg(p.hit).child(m)).child(b))
        });
        let status = match (&self.error, &self.stats, self.busy) {
            (_, _, true) => "Searching…".to_string(),
            (Some(e), _, _) => e.clone(),
            (_, Some(s), _) => format!("{} hits · {} sessions · {} MB scanned · {} ms{}", self.hits.len(), s.sessions, s.bytes >> 20, s.millis, if s.used_index { " · keyword index" } else { "" }),
            _ => if q.is_empty() { "Plain text by default. Regex and whole-word (indexed) modes are toggles.".into() } else { String::new() },
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_6()
            .bg(p.paper)
            .text_color(p.ink)
            .font_family(font)
            .text_size(px(13.))
            .child(div().text_size(px(28.)).font_weight(FontWeight::SEMIBOLD).child("Search"))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().child(self.query.clone()))
                    .child(tog("t-regex", "Regex", self.regex).on_click(cx.listener(|v, _, _, cx| {
                        v.regex = !v.regex;
                        cx.notify()
                    })))
                    .child(tog("t-words", "Whole word", self.words).on_click(cx.listener(|v, _, _, cx| {
                        v.words = !v.words;
                        cx.notify()
                    })))
                    .child(tog("t-case", "Match case", self.case).on_click(cx.listener(|v, _, _, cx| {
                        v.case = !v.case;
                        cx.notify()
                    })))
                    .child(btn("go", "Search", p).on_click(cx.listener(|v, _, _, cx| v.run(cx)))),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(div().w(px(150.)).child(self.host.clone()))
                    .child(div().w(px(180.)).child(self.folder.clone()))
                    .child(div().w(px(180.)).child(self.command.clone()))
                    .child(div().w(px(150.)).child(self.since.clone()))
                    .child(div().w(px(150.)).child(self.until.clone()))
                    .child(btn("t-mode", ["Any mode", "Standard", "Streaming"][self.mode as usize], p).on_click(cx.listener(|v, _, _, cx| {
                        v.mode = (v.mode + 1) % 3;
                        cx.notify()
                    })))
                    .child(tog("t-failed", "Failed only", self.failed).on_click(cx.listener(|v, _, _, cx| {
                        v.failed = !v.failed;
                        cx.notify()
                    })))
                    .child(btn("export-hits", "Export results", p).on_click(cx.listener(|v, _, _, cx| v.export(cx)))),
            )
            .child(div().text_color(p.muted).child(status))
            .child(
                div()
                    .id("results")
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .overflow_hidden()
                    .on_scroll_wheel(cx.listener(|v, e: &ScrollWheelEvent, _, cx| {
                        let d = match e.delta {
                            ScrollDelta::Lines(l) => -l.y * 3.,
                            ScrollDelta::Pixels(p) => -f32::from(p.y) / 52.,
                        };
                        v.top = (v.top as i64 + d.round() as i64).clamp(0, v.hits.len().saturating_sub(1) as i64) as usize;
                        cx.notify()
                    }))
                    .children(results),
            )
    }
}
