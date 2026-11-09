//! The report view: a native rebuild of the blast30/stream2 report. Virtualised by line, so a session or
//! a single row of any size opens instantly; only what is on screen is decoded.
use crate::ui::{btn, pal, pill, AppState, Input, InputEvent, Pal};
use ember_core::export::{display_time, export_html, export_text, strip_date};
use ember_core::search::{search, Hit, Query};
use ember_core::store::{list_sessions, RowKind, SessionReader};
use gpui::prelude::FluentBuilder;
use gpui::*;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const CONFIG: usize = usize::MAX;

struct Entry {
    row: usize,
    items: u64,
    indent: bool,
}

pub struct ReportView {
    pub dir: Option<PathBuf>,
    reader: Option<SessionReader>,
    entries: Vec<Entry>,
    starts: Vec<u64>,
    total: u64,
    top: u64,
    collapsed: HashSet<u64>,
    marks: HashMap<u64, Vec<(u64, u64)>>,
    lines: HashMap<u64, u64>,
    toc_top: usize,
    toc_filter: Entity<Input>,
    pub search: Entity<Input>,
    hits: Vec<Hit>,
    hit_ix: usize,
    search_all: bool,
    confirm: Option<Vec<u64>>,
    notice: Option<(String, Instant)>,
    view_h: f32,
    rail_drag: bool,
    side_drag: bool,
    pub focus: FocusHandle,
    follow: bool,
    _poll: Task<()>,
}

impl ReportView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let toc_filter = cx.new(|cx| Input::new("Filter index", cx));
        let search = cx.new(|cx| Input::new("Search report", cx));
        cx.subscribe(&toc_filter, |this, _, _e: &InputEvent, cx| {
            this.toc_top = 0;
            cx.notify()
        })
        .detach();
        cx.subscribe(&search, |this, _, e: &InputEvent, cx| match e {
            InputEvent::Submit | InputEvent::Down => this.run_search(cx),
            InputEvent::Up => this.step_hit(-1, cx),
            _ => {}
        })
        .detach();
        let poll = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| loop {
            cx.background_executor().timer(Duration::from_millis(500)).await;
            if this.update(cx, |v, cx| v.refresh(cx)).is_err() {
                break;
            }
        });
        Self {
            dir: None,
            reader: None,
            entries: vec![],
            starts: vec![],
            total: 0,
            top: 0,
            collapsed: HashSet::new(),
            marks: HashMap::new(),
            lines: HashMap::new(),
            toc_top: 0,
            toc_filter,
            search,
            hits: vec![],
            hit_ix: 0,
            search_all: false,
            confirm: None,
            notice: None,
            view_h: 600.,
            rail_drag: false,
            side_drag: false,
            focus: cx.focus_handle(),
            follow: true,
            _poll: poll,
        }
    }

    pub fn open(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        if self.dir.as_ref() == Some(&dir) {
            return;
        }
        match SessionReader::open(&dir) {
            Ok(r) => {
                let collapse = cx.global::<AppState>().settings.report.collapse_outputs_initially;
                self.collapsed = if collapse { r.rows.iter().map(|r| r.id).collect() } else { HashSet::new() };
                self.reader = Some(r);
                self.dir = Some(dir);
                self.marks.clear();
                self.lines.clear();
                self.top = 0;
                self.follow = false;
                self.relayout(cx);
            }
            Err(e) => self.note(format!("Cannot open session: {e}")),
        }
        cx.notify();
    }

    pub fn open_latest(&mut self, cx: &mut Context<Self>) {
        if self.dir.is_none() {
            if let Some(d) = list_sessions(&cx.global::<AppState>().root).last().cloned() {
                self.open(d, cx);
            }
        }
    }

    fn cycle_session(&mut self, step: i32, cx: &mut Context<Self>) {
        let all = list_sessions(&cx.global::<AppState>().root);
        if all.is_empty() {
            return;
        }
        let cur = self.dir.as_ref().and_then(|d| all.iter().position(|x| x == d)).unwrap_or(all.len() - 1) as i32;
        let next = (cur + step).rem_euclid(all.len() as i32) as usize;
        self.open(all[next].clone(), cx);
    }

    fn note(&mut self, s: String) {
        self.notice = Some((s, Instant::now()));
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.reader.as_mut() else { return };
        let changed = r.refresh().unwrap_or(false);
        let live = r.rows.iter().any(|x| x.open);
        if changed || live {
            let at_end = self.top + self.visible() >= self.total;
            self.relayout(cx);
            if at_end && self.follow {
                self.top = self.total.saturating_sub(self.visible());
            }
            cx.notify();
        }
        if self.notice.as_ref().map_or(false, |(_, t)| t.elapsed() > Duration::from_secs(3)) {
            self.notice = None;
            cx.notify();
        }
    }

    fn lh(cx: &App) -> f32 {
        let r = &cx.global::<AppState>().settings.report;
        (r.output_font_px * r.output_line_height).round()
    }

    fn visible(&self) -> u64 {
        (self.view_h / 23.0).ceil() as u64 + 1
    }

    fn relayout(&mut self, cx: &App) {
        let Some(r) = &self.reader else { return };
        let mut entries = vec![];
        for (i, row) in r.rows.iter().enumerate() {
            if row.deleted {
                continue;
            }
            if !self.marks.contains_key(&row.id) || row.open {
                self.marks.insert(row.id, r.line_marks(row.id));
            }
            let marks = &self.marks[&row.id];
            let mut n = r.row_lines(row, marks);
            let (a, b) = (row.text.0, r.row_end(row).1);
            if b > a && (n == 0 || r.text.read(b - 1, b).map_or(false, |x| x != b"\n")) {
                n += 1;
            }
            self.lines.insert(row.id, n);
            let body = if self.collapsed.contains(&row.id) { 0 } else { n.max(1) };
            let header_only = row.kind == RowKind::Command && row.mode == "streaming";
            entries.push(Entry { row: i, items: if header_only { 3 } else { 3 + body }, indent: row.parent.is_some() });
        }
        let cfg_lines = cx.global::<AppState>().settings.report_config_text().lines().count() as u64;
        entries.push(Entry { row: CONFIG, items: 2 + cfg_lines + 1, indent: false });
        let mut acc = 0;
        self.starts = entries
            .iter()
            .map(|e| {
                let s = acc;
                acc += e.items;
                s
            })
            .collect();
        self.total = acc;
        self.entries = entries;
        self.top = self.top.min(self.total.saturating_sub(1));
    }

    fn entry_at(&self, item: u64) -> usize {
        self.starts.partition_point(|&s| s <= item).saturating_sub(1)
    }

    fn jump_entry(&mut self, e: usize, cx: &mut Context<Self>) {
        if let Some(&s) = self.starts.get(e) {
            self.top = s;
            self.follow = false;
            cx.notify();
        }
    }

    fn scroll(&mut self, d: i64, cx: &mut Context<Self>) {
        let max = self.total.saturating_sub(self.visible() / 2) as i64;
        self.top = (self.top as i64 + d).clamp(0, max.max(0)) as u64;
        self.follow = self.top as i64 >= max - 1;
        cx.notify();
    }

    fn row_text(&self, id: u64, cap: u64) -> (String, String) {
        let Some(r) = &self.reader else { return (String::new(), String::new()) };
        let Some(row) = r.rows.iter().find(|x| x.id == id) else { return (String::new(), String::new()) };
        let end = r.row_end(row).1.min(row.text.0 + cap);
        (row.header.clone(), String::from_utf8_lossy(&r.text.read(row.text.0, end).unwrap_or_default()).to_string())
    }

    fn copy(&mut self, id: u64, what: u8, cx: &mut Context<Self>) {
        let (h, o) = self.row_text(id, 64 << 20);
        let t = match what {
            0 => h,
            1 => o,
            _ => format!("{h}\n{o}"),
        };
        cx.write_to_clipboard(ClipboardItem::new_string(t));
        self.note(["Copied header", "Copied output", "Copied header + output"][what.min(2) as usize].into());
        cx.notify();
    }

    fn save_snippet(&mut self, id: u64, cx: &mut Context<Self>) {
        let (h, o) = self.row_text(id, 1 << 20);
        let src = self.dir.as_ref().map(|d| d.display().to_string());
        let res = cx.global_mut::<AppState>().snippets.add(&h, &format!("{h}\n{o}"), vec![], src);
        self.note(if res.is_ok() { "Saved as snippet".into() } else { "Could not save snippet".into() });
        cx.notify();
    }

    fn delete_confirmed(&mut self, cx: &mut Context<Self>) {
        if let (Some(ids), Some(r)) = (self.confirm.take(), self.reader.as_mut()) {
            for id in ids {
                let _ = r.delete_row(id);
            }
            self.relayout(cx);
            self.note("Deleted from report. Stored output is untouched.".into());
        }
        cx.notify();
    }

    fn toggle_all(&mut self, cx: &mut Context<Self>) {
        let Some(r) = &self.reader else { return };
        if self.collapsed.is_empty() {
            self.collapsed = r.rows.iter().map(|x| x.id).collect();
        } else {
            self.collapsed.clear();
        }
        self.relayout(cx);
        cx.notify();
    }

    fn toggle(&mut self, id: u64, cx: &mut Context<Self>) {
        if !self.collapsed.remove(&id) {
            self.collapsed.insert(id);
        }
        self.relayout(cx);
        cx.notify();
    }

    fn run_search(&mut self, cx: &mut Context<Self>) {
        let q = self.search.read(cx).text.clone();
        if q.is_empty() {
            self.hits.clear();
            cx.notify();
            return;
        }
        if !self.hits.is_empty() && self.hits.first().map_or(false, |h| h.line.to_lowercase().contains(&q.to_lowercase()) || h.header.to_lowercase().contains(&q.to_lowercase())) {
            self.step_hit(1, cx);
            return;
        }
        let root = cx.global::<AppState>().root.clone();
        let mut query = Query::new(&q);
        query.limit = 50_000;
        if !self.search_all {
            query.sessions = self.dir.clone().map(|d| vec![d]);
        }
        let task = cx.background_spawn(async move { search(&root, &query) });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let res = task.await;
            let _ = this.update(cx, |v, cx| {
                match res {
                    Ok((mut hits, st)) => {
                        hits.sort_by(|a, b| (a.session.clone(), a.row_id, a.offset_in_row).cmp(&(b.session.clone(), b.row_id, b.offset_in_row)));
                        v.note(format!("{} hits · {} MB in {} ms", hits.len(), st.bytes >> 20, st.millis));
                        v.hits = hits;
                        v.hit_ix = 0;
                        v.show_hit(cx);
                    }
                    Err(e) => v.note(format!("Search error: {e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn step_hit(&mut self, d: i64, cx: &mut Context<Self>) {
        if self.hits.is_empty() {
            return;
        }
        self.hit_ix = (self.hit_ix as i64 + d).rem_euclid(self.hits.len() as i64) as usize;
        self.show_hit(cx);
    }

    /// Jumps to the exact row and line of the current hit (opening its session if needed).
    pub fn show_hit(&mut self, cx: &mut Context<Self>) {
        let Some(h) = self.hits.get(self.hit_ix).cloned() else { return };
        self.reveal(h.session.clone(), h.row_id, h.offset_in_row, cx);
    }

    pub fn reveal(&mut self, session: PathBuf, row_id: u64, offset_in_row: u64, cx: &mut Context<Self>) {
        self.open(session, cx);
        self.collapsed.remove(&row_id);
        self.relayout(cx);
        let Some(r) = &self.reader else { return };
        let Some(ri) = r.rows.iter().position(|x| x.id == row_id) else { return };
        let Some(e) = self.entries.iter().position(|e| e.row == ri) else { return };
        let marks = self.marks.get(&row_id).cloned().unwrap_or_default();
        let line = if offset_in_row == 0 { 0 } else { r.line_of(&r.rows[ri], &marks, offset_in_row) };
        self.top = (self.starts[e] + 2 + line).saturating_sub(4).max(self.starts[e].min(self.starts[e] + 2 + line));
        self.follow = false;
        cx.notify();
    }

    fn export(&mut self, kind: u8, cx: &mut Context<Self>) {
        let Some(dir) = self.dir.clone() else { return };
        let st = cx.global::<AppState>();
        let settings = st.settings.clone();
        let out = st.root.join("exports");
        let task = cx.background_spawn(async move {
            let s = SessionReader::open(&dir)?;
            let base = s.meta.id.clone();
            let items: Vec<(usize, usize)> = (0..s.rows.len()).filter(|&i| !s.rows[i].deleted).map(|i| (0, i)).collect();
            let ss = vec![s];
            if kind == 0 {
                let r = export_html(&ss, &items, &out, &base, &settings)?;
                anyhow::Ok(format!("Exported {} rows → {}", r.rows, r.html[0].display()))
            } else {
                std::fs::create_dir_all(&out)?;
                let p = out.join(format!("{base}.txt"));
                export_text(&ss, &items, &p, false)?;
                anyhow::Ok(format!("Exported → {}", p.display()))
            }
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let r = task.await;
            let _ = this.update(cx, |v, cx| {
                v.note(r.unwrap_or_else(|e| format!("Export failed: {e}")));
                cx.notify();
            });
        })
        .detach();
    }

    /// Output lines `range` (0-based) of a row, decoded only for what is on screen.
    fn fetch_lines(&self, row_ix: usize, range: Range<u64>) -> Vec<String> {
        let Some(r) = &self.reader else { return vec![] };
        let row = &r.rows[row_ix];
        let marks = self.marks.get(&row.id).cloned().unwrap_or_default();
        let start = r.line_offset(row, &marks, range.start);
        let end = r.row_end(row).1;
        let want = (range.end - range.start) as usize;
        let mut out = Vec::with_capacity(want);
        let mut off = start;
        let mut carry = Vec::new();
        while out.len() < want && off < end {
            let chunk = r.text.read(off, (off + (256 << 10)).min(end)).unwrap_or_default();
            if chunk.is_empty() {
                break;
            }
            off += chunk.len() as u64;
            carry.extend_from_slice(&chunk);
            while let Some(nl) = memchr_nl(&carry) {
                out.push(clean(&carry[..nl]));
                carry.drain(..=nl);
                if out.len() >= want {
                    break;
                }
            }
            if carry.len() > 64 << 10 {
                out.push(clean(&carry[..4096]) + " …");
                carry.clear();
                // skip the rest of this very long line
                let rest = r.text.read(off, (off + (64 << 20)).min(end)).unwrap_or_default();
                match memchr_nl(&rest) {
                    Some(nl) => off += nl as u64 + 1,
                    None => off = end,
                }
            }
        }
        if out.len() < want && !carry.is_empty() {
            out.push(clean(&carry));
        }
        out
    }
}

fn memchr_nl(b: &[u8]) -> Option<usize> {
    b.iter().position(|&c| c == b'\n')
}

fn clean(b: &[u8]) -> String {
    let s = String::from_utf8_lossy(b).replace('\t', "    ");
    if s.len() > 2000 {
        let mut i = 2000;
        while !s.is_char_boundary(i) {
            i -= 1;
        }
        format!("{} …", &s[..i])
    } else {
        s
    }
}

fn highlight(line: &str, q: &str, p: Pal) -> StyledText {
    let mut hl = vec![];
    if !q.is_empty() {
        let low = line.to_lowercase();
        let ql = q.to_lowercase();
        if low.len() == line.len() {
            let mut i = 0;
            while let Some(f) = low[i..].find(&ql) {
                hl.push((i + f..i + f + ql.len(), HighlightStyle { background_color: Some(p.hit), ..Default::default() }));
                i += f + ql.len().max(1);
            }
        }
    }
    StyledText::new(SharedString::from(line.to_string())).with_highlights(hl)
}

impl Render for ReportView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let st = cx.global::<AppState>();
        let rs = st.settings.report.clone();
        let font = st.settings.font_family.clone();
        let lh = Self::lh(cx);
        let wh = f32::from(window.viewport_size().height);
        self.view_h = (wh - 190.).max(200.);
        let n = (self.view_h / lh).ceil() as u64 + 1;
        let q = self.search.read(cx).text.clone();
        let pills: Vec<(String, String)> = self.reader.as_ref().map(|r| r.meta.pills.clone()).unwrap_or_else(|| st.pills.clone());
        let started = self.reader.as_ref().and_then(|r| r.meta.started);
        let host = self.reader.as_ref().map(|r| r.meta.host.clone()).unwrap_or_else(|| st.host.clone());
        let live = self.reader.as_ref().map_or(false, |r| r.rows.iter().any(|x| x.open) || r.meta.ended.is_none());

        // ── header strip ──
        let header = div()
            .flex()
            .flex_col()
            .gap_3()
            .px_6()
            .pt_4()
            .pb_3()
            .bg(p.ink)
            .text_color(p.paper)
            .child(
                div()
                    .flex()
                    .items_end()
                    .justify_between()
                    .child(div().flex().items_baseline().gap_3().child(div().text_size(px(13.)).child("EMBER")).child(div().text_size(px(30.)).font_weight(FontWeight::SEMIBOLD).child(host.clone())))
                    .child(
                        div()
                            .flex()
                            .gap_4()
                            .px_3()
                            .py_1()
                            .bg(p.paper)
                            .text_color(p.ink)
                            .text_size(px(16.))
                            .child(started.map(|s| s.format("%B %-d %Y").to_string()).unwrap_or_default())
                            .child(started.map(|s| display_time(&s)).unwrap_or_default())
                            .when(live, |d| d.child(div().text_color(p.std_light).child("● LIVE"))),
                    ),
            )
            .child(div().flex().flex_wrap().gap_2().children(pills.iter().map(|(k, v)| pill(k, v, p))));

        let toolbar = div()
            .flex()
            .items_center()
            .gap_2()
            .px_6()
            .py_2()
            .border_b_2()
            .border_color(p.ink)
            .bg(p.paper)
            .text_color(p.ink)
            .text_size(px(12.))
            .child(div().w(px(280.)).child(self.search.clone()))
            .child(btn("prev-hit", "↑", p).on_click(cx.listener(|v, _, _, cx| v.step_hit(-1, cx))))
            .child(btn("next-hit", "↓", p).on_click(cx.listener(|v, _, _, cx| v.step_hit(1, cx))))
            .child(div().min_w(px(70.)).child(if self.hits.is_empty() { String::new() } else { format!("{}/{}", self.hit_ix + 1, self.hits.len()) }))
            .child(btn("scope", if self.search_all { "All history" } else { "This session" }, p).on_click(cx.listener(|v, _, _, cx| {
                v.search_all = !v.search_all;
                v.hits.clear();
                cx.notify()
            })))
            .child(div().flex_1())
            .child(btn("fold-all", if self.collapsed.is_empty() { "Collapse all" } else { "Expand all" }, p).on_click(cx.listener(|v, _, _, cx| v.toggle_all(cx))))
            .child(btn("export-html", "Export HTML + JSON", p).on_click(cx.listener(|v, _, _, cx| v.export(0, cx))))
            .child(btn("export-txt", "Export text", p).on_click(cx.listener(|v, _, _, cx| v.export(1, cx))))
            .child(btn("sess-prev", "◀", p).on_click(cx.listener(|v, _, _, cx| v.cycle_session(-1, cx))))
            .child(div().max_w(px(260.)).overflow_hidden().child(self.reader.as_ref().map(|r| r.meta.id.clone()).unwrap_or_else(|| "no session".into())))
            .child(btn("sess-next", "▶", p).on_click(cx.listener(|v, _, _, cx| v.cycle_session(1, cx))));

        // ── content (virtualised by line) ──
        let mut lines_el: Vec<AnyElement> = vec![];
        let mut item = self.top;
        let end = (self.top + n).min(self.total);
        while item < end {
            let ei = self.entry_at(item);
            let e = &self.entries[ei];
            let k = item - self.starts[ei];
            let last = e.items - 1;
            if k == last {
                lines_el.push(div().h(px(lh)).into_any_element());
                item += 1;
                continue;
            }
            if e.row == CONFIG {
                let cfg = cx.global::<AppState>().settings.report_config_text();
                let l = if k == 0 { String::new() } else if k == 1 { "config".into() } else { cfg.lines().nth((k - 2) as usize).unwrap_or("").to_string() };
                lines_el.push(div().h(px(lh)).px_3().border_x_2().border_color(p.ink).when(k == 1, |d| d.font_weight(FontWeight::BOLD).text_size(px(rs.row_font_px))).child(l).into_any_element());
                item += 1;
                continue;
            }
            let row = self.reader.as_ref().unwrap().rows[e.row].clone();
            let id = row.id;
            let indent = if e.indent { px(28.) } else { px(0.) };
            let bad = row.rc.map_or(false, |c| c != 0);
            if k == 0 {
                let folded = self.collapsed.contains(&id);
                lines_el.push(
                    div()
                        .h(px(lh))
                        .ml(indent)
                        .flex()
                        .items_center()
                        .justify_between()
                        .px_3()
                        .border_t_2()
                        .border_x_2()
                        .border_color(p.ink)
                        .bg(if bad { p.bad } else { p.ink })
                        .text_color(p.paper)
                        .text_size(px(rs.timestamp_font_px))
                        .child(div().flex().gap_2().child(div().font_weight(FontWeight::BOLD).child(strip_date(&row.ts))).child(row.ts.format("%I:%M:%S %p").to_string()).when(bad, |d| d.child(format!("exit {}", row.rc.unwrap()))).when(row.open, |d| d.child("● recording")))
                        .child(
                            div()
                                .flex()
                                .gap_1()
                                .child(mini(("del", id), "✕", p).on_click(cx.listener(move |v, _, _, cx| {
                                    v.confirm = Some(vec![id]);
                                    cx.notify()
                                })))
                                .child(mini(("cph", id), "⧉ cmd", p).on_click(cx.listener(move |v, e: &ClickEvent, _, cx| v.copy(id, if e.modifiers().shift { 2 } else { 0 }, cx))))
                                .child(mini(("cpo", id), "⧉ out", p).on_click(cx.listener(move |v, e: &ClickEvent, _, cx| v.copy(id, if e.modifiers().shift { 2 } else { 1 }, cx))))
                                .child(mini(("fold", id), if folded { "+" } else { "−" }, p).on_click(cx.listener(move |v, _, _, cx| v.toggle(id, cx))))
                                .child(mini(("snip", id), "☆", p).on_click(cx.listener(move |v, _, _, cx| v.save_snippet(id, cx)))),
                        )
                        .into_any_element(),
                );
                item += 1;
                continue;
            }
            if k == 1 {
                lines_el.push(
                    div()
                        .h(px(lh))
                        .ml(indent)
                        .px_3()
                        .flex()
                        .items_center()
                        .border_x_2()
                        .border_b_1()
                        .border_color(p.ink)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(rs.row_font_px))
                        .overflow_hidden()
                        .child(highlight(&row.header, &q, p))
                        .into_any_element(),
                );
                item += 1;
                continue;
            }
            // a run of output lines of this row
            let first = k - 2;
            let run_end = (e.items - 1).min(k + (end - item));
            let count = run_end - k;
            let texts = self.fetch_lines(e.row, first..first + count);
            for i in 0..count as usize {
                let t = texts.get(i).cloned().unwrap_or_default();
                lines_el.push(div().h(px(lh)).ml(indent).px_3().border_x_2().border_color(p.ink).text_size(px(rs.output_font_px)).overflow_hidden().whitespace_nowrap().child(highlight(&t, &q, p)).into_any_element());
            }
            item += count.max(1);
        }

        // ── sidebar index ──
        let filter = self.toc_filter.read(cx).text.to_lowercase();
        let reader = self.reader.as_ref();
        let toc: Vec<usize> = (0..self.entries.len().saturating_sub(1))
            .filter(|&i| filter.is_empty() || reader.map_or(false, |r| r.rows[self.entries[i].row].header.to_lowercase().contains(&filter)))
            .collect();
        let cur_entry = self.entry_at(self.top);
        let toc_n = ((self.view_h - 120.) / (rs.toc_font_px + 14.)).max(3.) as usize;
        self.toc_top = self.toc_top.min(toc.len().saturating_sub(1));
        let toc_items = toc.iter().skip(self.toc_top).take(toc_n).map(|&i| {
            let row = &reader.unwrap().rows[self.entries[i].row];
            let active = i == cur_entry;
            let child = self.entries[i].indent || row.kind == RowKind::Item;
            div()
                .id(("toc", i))
                .px_2()
                .py_1()
                .when(child, |d| d.ml(px(14.)))
                .border_2()
                .border_color(p.ink)
                .bg(if active { p.ink } else { p.paper })
                .text_color(if active { p.paper } else if row.rc.map_or(false, |c| c != 0) { p.bad } else { p.ink })
                .text_size(px(rs.toc_font_px))
                .whitespace_nowrap()
                .overflow_hidden()
                .cursor_pointer()
                .hover(move |s| s.bg(p.ink).text_color(p.paper))
                .on_click(cx.listener(move |v, _, _, cx| v.jump_entry(i, cx)))
                .child(row.header.clone())
        });
        let rows_n = self.entries.len().saturating_sub(1);
        let seg = rs.segment_rows.max(1) as usize;
        let parts = rows_n.div_ceil(seg).max(1);
        let cur_part = cur_entry / seg;
        let parts_lane = div().flex().flex_wrap().pt_2().border_t_2().border_color(p.ink).children((0..parts.min(200)).map(|i| {
            div()
                .id(("part", i))
                .w(px(rs.part_button_width_px))
                .h(px(rs.part_button_height_px))
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(p.ink)
                .bg(if i == cur_part { p.ink } else { p.paper })
                .text_color(if i == cur_part { p.paper } else { p.ink })
                .cursor_pointer()
                .on_click(cx.listener(move |v, _, _, cx| v.jump_entry(i * seg, cx)))
                .child(format!("{}", i + 1))
        }));
        let sidebar = div()
            .id("toc")
            .w(px(rs.sidebar_width_px))
            .h_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .border_r_2()
            .border_color(p.ink)
            .on_scroll_wheel(cx.listener(move |v, e: &ScrollWheelEvent, _, cx| {
                let d = match e.delta {
                    ScrollDelta::Lines(l) => -l.y * 3.,
                    ScrollDelta::Pixels(p) => -f32::from(p.y) / 28.,
                };
                v.toc_top = (v.toc_top as i64 + d.round() as i64).max(0) as usize;
                cx.notify()
            }))
            .child(self.toc_filter.clone())
            .child(div().flex().flex_col().gap_1().flex_1().overflow_hidden().children(toc_items))
            .child(parts_lane);
        let divider = div().id("divider").w(px(6.)).h_full().cursor_col_resize().hover(move |s| s.bg(p.accent)).on_mouse_down(
            MouseButton::Left,
            cx.listener(|v, _, _, _| v.side_drag = true),
        );

        // ── fast-scroll rail ──
        let rail_h = self.view_h;
        let frac = if self.total > 0 { self.top as f32 / self.total as f32 } else { 0. };
        let thumb_h = (n as f32 / self.total.max(1) as f32 * rail_h).clamp(48., rail_h);
        let rail = div()
            .id("rail")
            .w(px(16.))
            .h_full()
            .relative()
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, cx.listener(move |v, e: &MouseDownEvent, _, cx| {
                v.rail_drag = true;
                v.seek(f32::from(e.position.y), cx);
            }))
            .child(div().absolute().top(px(frac * (rail_h - thumb_h))).left(px(2.)).w(px(12.)).h(px(thumb_h)).rounded(px(6.)).border_2().border_color(p.ink).bg(p.ink));

        let content = div()
            .id("report-content")
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .px_6()
            .pt_3()
            .overflow_hidden()
            .font_family(font.clone())
            .line_height(px(lh))
            .on_scroll_wheel(cx.listener(move |v, e: &ScrollWheelEvent, _, cx| {
                let d = match e.delta {
                    ScrollDelta::Lines(l) => -l.y * 3.,
                    ScrollDelta::Pixels(p) => -f32::from(p.y) / lh,
                };
                v.scroll(d.round() as i64, cx)
            }))
            .children(lines_el)
            .when(self.reader.is_none(), |d| d.child(div().p_6().child("No recorded sessions yet. Turn on recording in a pane (⌘R standard, ⌘⇧R streaming) and run something.")));

        div()
            .id("report")
            .track_focus(&self.focus)
            .key_context("Report")
            .size_full()
            .flex()
            .flex_col()
            .bg(p.paper)
            .text_color(p.ink)
            .font_family(font)
            .on_key_down(cx.listener(move |v, e: &KeyDownEvent, _, cx| {
                let page = n as i64 - 2;
                match e.keystroke.key.as_str() {
                    "down" => v.scroll(1, cx),
                    "up" => v.scroll(-1, cx),
                    "pagedown" | "space" => v.scroll(page, cx),
                    "pageup" => v.scroll(-page, cx),
                    "home" => v.scroll(i64::MIN / 2, cx),
                    "end" => v.scroll(i64::MAX / 2, cx),
                    _ => {}
                }
            }))
            .on_mouse_move(cx.listener(|v, e: &MouseMoveEvent, _, cx| {
                if e.pressed_button != Some(MouseButton::Left) {
                    v.side_drag = false;
                    v.rail_drag = false;
                    return;
                }
                if v.side_drag {
                    let w = f32::from(e.position.x).clamp(180., 600.);
                    cx.global_mut::<AppState>().settings.report.sidebar_width_px = w;
                    cx.notify();
                } else if v.rail_drag {
                    v.seek(f32::from(e.position.y), cx);
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|v, _, _, cx| {
                if v.side_drag {
                    cx.global::<AppState>().save();
                }
                v.side_drag = false;
                v.rail_drag = false;
            }))
            .child(header)
            .child(toolbar)
            .child(div().flex().flex_1().overflow_hidden().child(sidebar).child(divider).child(content).child(rail))
            .when_some(self.notice.clone(), |d, (t, _)| d.child(div().absolute().bottom(px(18.)).left(px(18.)).px_3().py_2().bg(p.ink).text_color(p.paper).child(t)))
            .when_some(self.confirm.clone(), |d, ids| {
                d.child(
                    div().absolute().size_full().flex().items_center().justify_center().bg(gpui::black().opacity(0.25)).child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .p_5()
                            .bg(p.paper)
                            .border_2()
                            .border_color(p.ink)
                            .child(format!("Delete {} entr{} from this report? Stored output is untouched.", ids.len(), if ids.len() == 1 { "y" } else { "ies" }))
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .justify_end()
                                    .child(btn("cancel-del", "Cancel", p).on_click(cx.listener(|v, _, _, cx| {
                                        v.confirm = None;
                                        cx.notify()
                                    })))
                                    .child(btn("confirm-del", "Delete", p).on_click(cx.listener(|v, _, _, cx| v.delete_confirmed(cx)))),
                            ),
                    ),
                )
            })
    }
}

impl ReportView {
    /// Rail position → item, snapped to the start of the row under it (input, then output).
    fn seek(&mut self, y: f32, cx: &mut Context<Self>) {
        let top_px = 190.;
        let f = ((y - top_px) / self.view_h).clamp(0., 1.);
        let item = (f as f64 * self.total as f64) as u64;
        let e = self.entry_at(item);
        self.top = self.starts.get(e).copied().unwrap_or(0);
        self.follow = false;
        cx.notify();
    }
}

fn mini(id: impl Into<ElementId>, label: &'static str, p: Pal) -> Stateful<Div> {
    div().id(id.into()).px_1().text_color(p.paper).cursor_pointer().hover(move |s| s.bg(p.paper).text_color(p.ink)).child(label)
}
