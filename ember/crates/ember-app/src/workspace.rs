//! A window: tabs of split panes, plus the report, search, snippets and settings views.
use crate::palette::{PalAction, Palette, PaletteEvent, SnippetEvent, SnippetsView};
use crate::report::ReportView;
use crate::searchview::{OpenHit, SearchView};
use crate::settingsview::SettingsView;
use crate::term::{TermViewEvent, TerminalView};
use crate::ui::{ca, pal, AppState};
use ember_core::recorder::Mode;
use gpui::prelude::FluentBuilder;
use gpui::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq)]
pub enum View {
    Terminal,
    Report,
    Search,
    Snippets,
    Settings,
}

#[derive(Clone)]
enum Node {
    Leaf(Entity<TerminalView>),
    Split(bool, Vec<Node>, Vec<f32>),
}

impl Node {
    fn leaves(&self, out: &mut Vec<Entity<TerminalView>>) {
        match self {
            Node::Leaf(t) => out.push(t.clone()),
            Node::Split(_, ch, _) => ch.iter().for_each(|c| c.leaves(out)),
        }
    }
    /// Splits the leaf `id` along `vertical` (side by side) or stacked, inserting `new` after it.
    fn split(&mut self, id: EntityId, vertical: bool, new: Entity<TerminalView>) -> bool {
        match self {
            Node::Leaf(t) if t.entity_id() == id => {
                let old = Node::Leaf(t.clone());
                *self = Node::Split(vertical, vec![old, Node::Leaf(new)], vec![0.5, 0.5]);
                true
            }
            Node::Leaf(_) => false,
            Node::Split(v, ch, r) => {
                for i in 0..ch.len() {
                    if let Node::Leaf(t) = &ch[i] {
                        if t.entity_id() == id && *v == vertical {
                            let share = r[i] / 2.;
                            r[i] = share;
                            ch.insert(i + 1, Node::Leaf(new));
                            r.insert(i + 1, share);
                            return true;
                        }
                    }
                    if ch[i].split(id, vertical, new.clone()) {
                        return true;
                    }
                }
                false
            }
        }
    }
    fn remove(&mut self, id: EntityId) -> bool {
        if let Node::Split(_, ch, r) = self {
            if let Some(i) = ch.iter().position(|c| matches!(c, Node::Leaf(t) if t.entity_id() == id)) {
                ch.remove(i);
                let gone = r.remove(i);
                let n = r.len() as f32;
                r.iter_mut().for_each(|x| *x += gone / n);
                if ch.len() == 1 {
                    *self = ch.remove(0);
                }
                return true;
            }
            for c in ch.iter_mut() {
                if c.remove(id) {
                    return true;
                }
            }
        }
        false
    }
    fn ratios_mut(&mut self, path: &[usize]) -> Option<&mut Vec<f32>> {
        match (self, path.split_first()) {
            (Node::Split(_, _, r), None) => Some(r),
            (Node::Split(_, ch, _), Some((i, rest))) => ch.get_mut(*i)?.ratios_mut(rest),
            _ => None,
        }
    }
}

struct Tab {
    root: Node,
    focused: Entity<TerminalView>,
}

struct Drag {
    path: Vec<usize>,
    idx: usize,
    vertical: bool,
}

pub struct Workspace {
    tabs: Vec<Tab>,
    active: usize,
    pub view: View,
    pub report: Entity<ReportView>,
    search: Entity<SearchView>,
    settings: Entity<SettingsView>,
    snippets: Entity<SnippetsView>,
    palette: Option<Entity<Palette>>,
    bounds: Rc<RefCell<HashMap<Vec<usize>, Bounds<Pixels>>>>,
    drag: Option<Drag>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let report = cx.new(ReportView::new);
        let search = cx.new(SearchView::new);
        cx.subscribe_in(&search, window, |ws, _, e: &OpenHit, window, cx| {
            ws.report.update(cx, |r, cx| r.reveal(e.0.clone(), e.1, e.2, cx));
            ws.show(View::Report, window, cx);
        })
        .detach();
        let snippets = cx.new(SnippetsView::new);
        cx.subscribe(&snippets, |ws, _, e: &SnippetEvent, cx| {
            let SnippetEvent::Insert(id) = e;
            ws.insert_snippet(*id, cx);
        })
        .detach();
        let mut ws = Self { tabs: vec![], active: 0, view: View::Terminal, report, search, settings: cx.new(SettingsView::new), snippets, palette: None, bounds: Default::default(), drag: None };
        ws.new_tab(window, cx);
        ws
    }

    fn spawn_term(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<TerminalView>> {
        let t = cx.new(|cx| TerminalView::new(None, cx).expect("spawn zsh"));
        cx.subscribe_in(&t, window, |ws, t, e: &TermViewEvent, window, cx| {
            if matches!(e, TermViewEvent::Exited) {
                ws.close_pane(t.entity_id(), window, cx);
            }
            cx.notify();
        })
        .detach();
        let _ = window;
        Some(t)
    }

    pub fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = self.spawn_term(window, cx) {
            t.read(cx).focus.focus(window);
            self.tabs.push(Tab { root: Node::Leaf(t.clone()), focused: t });
            self.active = self.tabs.len() - 1;
            self.view = View::Terminal;
        }
        cx.notify();
    }

    pub fn split(&mut self, vertical: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.spawn_term(window, cx) else { return };
        let tab = &mut self.tabs[self.active];
        let id = tab.focused.entity_id();
        if tab.root.split(id, vertical, t.clone()) {
            tab.focused = t.clone();
            t.read(cx).focus.focus(window);
        }
        self.view = View::Terminal;
        cx.notify();
    }

    fn close_pane(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ti) = self.tabs.iter().position(|t| {
            let mut l = vec![];
            t.root.leaves(&mut l);
            l.iter().any(|x| x.entity_id() == id)
        }) else {
            return;
        };
        let tab = &mut self.tabs[ti];
        if matches!(&tab.root, Node::Leaf(_)) {
            self.tabs.remove(ti);
            if self.tabs.is_empty() {
                window.remove_window();
                return;
            }
            self.active = self.active.min(self.tabs.len() - 1);
        } else {
            tab.root.remove(id);
            let mut l = vec![];
            tab.root.leaves(&mut l);
            tab.focused = l[0].clone();
        }
        self.tabs[self.active].focused.read(cx).focus.focus(window);
        cx.notify();
    }

    pub fn focused(&self) -> Option<Entity<TerminalView>> {
        self.tabs.get(self.active).map(|t| t.focused.clone())
    }

    fn cycle_pane(&mut self, d: i32, window: &mut Window, cx: &mut Context<Self>) {
        let tab = &mut self.tabs[self.active];
        let mut l = vec![];
        tab.root.leaves(&mut l);
        let i = l.iter().position(|x| x.entity_id() == tab.focused.entity_id()).unwrap_or(0) as i32;
        tab.focused = l[(i + d).rem_euclid(l.len() as i32) as usize].clone();
        tab.focused.read(cx).focus.focus(window);
        cx.notify();
    }

    pub fn toggle_record(&mut self, m: Mode, cx: &mut Context<Self>) {
        if let Some(t) = self.focused() {
            t.update(cx, |t, cx| {
                let next = if t.mode == m { Mode::Off } else { m };
                t.set_mode(next, cx)
            });
        }
        cx.notify();
    }

    pub fn show(&mut self, v: View, window: &mut Window, cx: &mut Context<Self>) {
        self.view = v;
        match v {
            View::Report => {
                let dir = self.focused().and_then(|t| t.read(cx).session_dir());
                self.report.update(cx, |r, cx| match dir {
                    Some(d) => r.open(d, cx),
                    None => r.open_latest(cx),
                });
                self.report.read(cx).focus.focus(window);
            }
            View::Terminal => {
                if let Some(t) = self.focused() {
                    t.read(cx).focus.focus(window)
                }
            }
            View::Search => self.search.read(cx).query.read(cx).focus.focus(window),
            _ => {}
        }
        cx.notify();
    }

    fn insert_snippet(&mut self, id: u64, cx: &mut Context<Self>) {
        let body = cx.global::<AppState>().snippets.items.iter().find(|s| s.id == id).map(|s| s.body.clone());
        if let (Some(b), Some(t)) = (body, self.focused()) {
            t.update(cx, |t, _| t.paste(&b));
            self.view = View::Terminal;
        }
        cx.notify();
    }

    pub fn palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            return cx.notify();
        }
        let p = cx.new(|cx| Palette::new(window, cx));
        cx.subscribe_in(&p, window, |ws, _, e: &PaletteEvent, window, cx| {
            ws.palette = None;
            if let PaletteEvent::Run(a) = e {
                ws.run(a.clone(), window, cx);
            }
            cx.notify();
        })
        .detach();
        self.palette = Some(p);
        cx.notify();
    }

    pub fn run(&mut self, a: PalAction, window: &mut Window, cx: &mut Context<Self>) {
        match a {
            PalAction::Cmd(c) => self.command(c, window, cx),
            PalAction::Theme(t) => {
                cx.global_mut::<AppState>().settings.theme = t;
                cx.global::<AppState>().save();
                cx.refresh_windows();
            }
            PalAction::Insert(id) => self.insert_snippet(id, cx),
            PalAction::Copy(id) => {
                if let Some(s) = cx.global::<AppState>().snippets.items.iter().find(|s| s.id == id) {
                    cx.write_to_clipboard(ClipboardItem::new_string(s.body.clone()));
                }
            }
        }
    }

    pub fn command(&mut self, c: &str, window: &mut Window, cx: &mut Context<Self>) {
        match c {
            "new_tab" => self.new_tab(window, cx),
            "new_window" => crate::open_window(cx),
            "split_right" => self.split(true, window, cx),
            "split_down" => self.split(false, window, cx),
            "close" => {
                if let Some(t) = self.focused() {
                    self.close_pane(t.entity_id(), window, cx)
                }
            }
            "focus_next_pane" => self.cycle_pane(1, window, cx),
            "focus_prev_pane" => self.cycle_pane(-1, window, cx),
            "next_tab" | "prev_tab" => {
                let d = if c == "next_tab" { 1 } else { -1 };
                self.active = (self.active as i32 + d).rem_euclid(self.tabs.len() as i32) as usize;
                self.show(View::Terminal, window, cx);
            }
            "toggle_record_standard" => self.toggle_record(Mode::Standard, cx),
            "toggle_record_streaming" => self.toggle_record(Mode::Streaming, cx),
            "terminal_view" => self.show(View::Terminal, window, cx),
            "report_view" => self.show(View::Report, window, cx),
            "search_view" => self.show(View::Search, window, cx),
            "snippets_view" => self.show(View::Snippets, window, cx),
            "settings" => self.show(View::Settings, window, cx),
            "search" => {
                self.show(View::Report, window, cx);
                self.report.read(cx).search.read(cx).focus.focus(window);
            }
            "palette" => self.palette(window, cx),
            "snippet_from_clipboard" => {
                if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    let title: String = t.lines().next().unwrap_or("snippet").chars().take(80).collect();
                    let _ = cx.global_mut::<AppState>().snippets.add(&title, &t, vec![], None);
                }
                self.show(View::Snippets, window, cx);
            }
            "copy" => {
                if let Some(s) = self.focused().and_then(|t| t.read(cx).copy()) {
                    cx.write_to_clipboard(ClipboardItem::new_string(s));
                }
            }
            "paste" => {
                if let (Some(t), Some(s)) = (self.focused(), cx.read_from_clipboard().and_then(|c| c.text())) {
                    t.update(cx, |t, _| t.paste(&s));
                }
            }
            _ => {}
        }
    }

    fn render_node(&self, node: &Node, path: Vec<usize>, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let s = &cx.global::<AppState>().settings;
        let border = s.pane_border_px;
        let light = s.recording_light.clone();
        match node {
            Node::Leaf(t) => {
                let focused = self.tabs[self.active].focused.entity_id() == t.entity_id();
                let mode = t.read(cx).mode;
                let tt = t.clone();
                let lamp_color = match mode {
                    Mode::Off => p.muted,
                    Mode::Standard => p.std_light,
                    Mode::Streaming => p.stream_light,
                };
                let lamp = div()
                    .id(("lamp", t.entity_id().as_u64()))
                    .absolute()
                    .top(px(6.))
                    .right(px(10.))
                    .cursor_pointer()
                    .on_click(cx.listener(move |_, _, _, cx| {
                        tt.update(cx, |t, cx| {
                            let next = match t.mode {
                                Mode::Off => Mode::Standard,
                                Mode::Standard => Mode::Streaming,
                                Mode::Streaming => Mode::Off,
                            };
                            t.set_mode(next, cx)
                        })
                    }))
                    .map(|d| match light.as_str() {
                        "pill" => d.px_2().text_size(px(10.)).border_1().border_color(lamp_color).text_color(lamp_color).child(match mode {
                            Mode::Off => "REC OFF",
                            Mode::Standard => "● REC",
                            Mode::Streaming => "◉ STREAM",
                        }),
                        "bar" => d.top(px(0.)).right(px(0.)).w(px(48.)).h(px(4.)).bg(lamp_color),
                        _ => d.size(px(11.)).rounded_full().border_2().border_color(lamp_color).when(mode != Mode::Off, |d| d.bg(lamp_color)).when(mode == Mode::Streaming, |d| d.border_color(p.ink)),
                    });
                div()
                    .relative()
                    .size_full()
                    .border(px(border))
                    .border_color(if focused { gpui::transparent_black() } else { p.line.opacity(0.25) })
                    .when(focused && border > 0., |d| d.border_color(p.accent.opacity(0.6)))
                    .child(t.clone())
                    .child(lamp)
                    .into_any_element()
            }
            Node::Split(vertical, ch, ratios) => {
                let v = *vertical;
                let cell = self.bounds.clone();
                let p2 = path.clone();
                let mut d = div().relative().size_full().flex().when(!v, |d| d.flex_col()).child(
                    canvas(move |b, _, _| cell.borrow_mut().insert(p2.clone(), b), |_, _, _, _| {}).absolute().size_full(),
                );
                for (i, c) in ch.iter().enumerate() {
                    if i > 0 {
                        let path2 = path.clone();
                        d = d.child(
                            div()
                                .id(SharedString::from(format!("div-{path:?}-{i}")))
                                .when(v, |d| d.w(px(5.)).h_full().cursor_col_resize())
                                .when(!v, |d| d.h(px(5.)).w_full().cursor_row_resize())
                                .hover(move |s| s.bg(p.accent.opacity(0.4)))
                                .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, _| ws.drag = Some(Drag { path: path2.clone(), idx: i, vertical: v }))),
                        );
                    }
                    let mut cp = path.clone();
                    cp.push(i);
                    d = d.child(div().flex_basis(relative(ratios[i])).flex_grow().flex_shrink().overflow_hidden().child(self.render_node(c, cp, cx)));
                }
                d.into_any_element()
            }
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let st = cx.global::<AppState>();
        let theme = st.theme();
        let opacity = st.settings.opacity;
        let font = st.settings.font_family.clone();
        let bg = ca(theme.background, if self.view == View::Terminal { opacity } else { 1.0 });
        let tabs = self.tabs.iter().enumerate().map(|(i, t)| {
            let on = i == self.active && self.view == View::Terminal;
            let title = t.focused.read(cx).title.clone();
            div()
                .id(("tab", i))
                .px_3()
                .py(px(3.))
                .text_size(px(12.))
                .border_b_2()
                .border_color(if on { p.accent } else { gpui::transparent_black() })
                .text_color(if on { p.ink } else { p.muted })
                .cursor_pointer()
                .on_click(cx.listener(move |ws, _, w, cx| {
                    ws.active = i;
                    ws.show(View::Terminal, w, cx)
                }))
                .child(title)
        });
        let nav = |id: &'static str, label: &'static str, v: View, cx: &mut Context<Self>| {
            let on = self.view == v;
            div()
                .id(id)
                .px_2()
                .text_size(px(12.))
                .text_color(if on { p.ink } else { p.muted })
                .when(on, |d| d.font_weight(FontWeight::BOLD))
                .cursor_pointer()
                .on_click(cx.listener(move |ws, _, w, cx| ws.show(v, w, cx)))
                .child(label)
        };
        let bar = div()
            .flex()
            .items_center()
            .h(px(30.))
            .pl(px(if cfg!(target_os = "macos") { 80. } else { 10. }))
            .pr_2()
            .gap_1()
            .children(tabs)
            .child(div().id("plus").px_2().text_color(p.muted).cursor_pointer().child("+").on_click(cx.listener(|ws, _, w, cx| ws.new_tab(w, cx))))
            .child(div().flex_1())
            .child(nav("nav-term", "Terminal", View::Terminal, cx))
            .child(nav("nav-report", "Report", View::Report, cx))
            .child(nav("nav-search", "Search", View::Search, cx))
            .child(nav("nav-snip", "Snippets", View::Snippets, cx))
            .child(nav("nav-set", "Settings", View::Settings, cx))
            .child(div().id("nav-k").px_2().text_size(px(12.)).text_color(p.muted).cursor_pointer().child("⌘K").on_click(cx.listener(|ws, _, w, cx| ws.palette(w, cx))));
        let body: AnyElement = match self.view {
            View::Terminal => self.render_node(&self.tabs[self.active].root.clone(), vec![], cx),
            View::Report => self.report.clone().into_any_element(),
            View::Search => self.search.clone().into_any_element(),
            View::Snippets => self.snippets.clone().into_any_element(),
            View::Settings => self.settings.clone().into_any_element(),
        };
        let _ = window;
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(p.ink)
            .font_family(font)
            .key_context("Workspace")
            .map(|d| crate::bind_actions(d, cx))
            .on_mouse_move(cx.listener(|ws, e: &MouseMoveEvent, _, cx| {
                let Some(d) = &ws.drag else { return };
                if e.pressed_button != Some(MouseButton::Left) {
                    ws.drag = None;
                    return;
                }
                let Some(b) = ws.bounds.borrow().get(&d.path).copied() else { return };
                let (pos, start, len) = if d.vertical { (e.position.x, b.origin.x, b.size.width) } else { (e.position.y, b.origin.y, b.size.height) };
                let f = (f32::from(pos - start) / f32::from(len)).clamp(0.05, 0.95);
                let (path, idx) = (d.path.clone(), d.idx);
                let tab = &mut ws.tabs[ws.active];
                if let Some(r) = tab.root.ratios_mut(&path) {
                    let before: f32 = r[..idx - 1].iter().sum();
                    let pair = r[idx - 1] + r[idx];
                    let left = (f - before).clamp(0.05, pair - 0.05);
                    r[idx - 1] = left;
                    r[idx] = pair - left;
                }
                cx.notify();
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|ws, _, _, _| ws.drag = None))
            .child(bar)
            .child(div().flex_1().overflow_hidden().child(body))
            .when_some(self.palette.clone(), |d, pal| d.child(div().absolute().top(px(80.)).w_full().flex().justify_center().child(pal)))
    }
}
