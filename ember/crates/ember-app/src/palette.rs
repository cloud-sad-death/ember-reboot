//! ⌘K palette and the snippets view.
use crate::ui::{btn, pal, AppState, Input, InputEvent};
use gpui::prelude::FluentBuilder;
use gpui::*;

#[derive(Clone, Debug)]
pub enum PalAction {
    Cmd(&'static str),
    Theme(String),
    Insert(u64),
    Copy(u64),
}

pub const COMMANDS: &[(&str, &str)] = &[
    ("new_tab", "New tab"),
    ("new_window", "New window"),
    ("split_right", "Split right"),
    ("split_down", "Split down"),
    ("close", "Close pane"),
    ("toggle_record_standard", "Record: Standard on/off"),
    ("toggle_record_streaming", "Record: Streaming on/off"),
    ("terminal_view", "Show terminal"),
    ("report_view", "Show report"),
    ("search_view", "Search all history"),
    ("snippets_view", "Snippets"),
    ("settings", "Settings"),
    ("snippet_from_clipboard", "New snippet from clipboard"),
];

pub struct Palette {
    pub input: Entity<Input>,
    sel: usize,
}
pub enum PaletteEvent {
    Run(PalAction),
    Dismiss,
}
impl EventEmitter<PaletteEvent> for Palette {}

impl Palette {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| Input::new("Type a command, theme or snippet", cx));
        input.read(cx).focus.focus(window);
        cx.subscribe(&input, |v, _, e: &InputEvent, cx| match e {
            InputEvent::Changed => {
                v.sel = 0;
                cx.notify()
            }
            InputEvent::Up => {
                v.sel = v.sel.saturating_sub(1);
                cx.notify()
            }
            InputEvent::Down => {
                v.sel += 1;
                cx.notify()
            }
            InputEvent::Escape => cx.emit(PaletteEvent::Dismiss),
            InputEvent::Submit => {
                if let Some((_, a)) = v.items(cx).get(v.sel).cloned() {
                    cx.emit(PaletteEvent::Run(a))
                }
            }
        })
        .detach();
        Self { input, sel: 0 }
    }

    fn items(&self, cx: &App) -> Vec<(String, PalAction)> {
        let q = self.input.read(cx).text.to_lowercase();
        let st = cx.global::<AppState>();
        let mut v: Vec<(String, PalAction)> = COMMANDS.iter().map(|(k, l)| (l.to_string(), PalAction::Cmd(k))).collect();
        v.extend(st.themes.iter().map(|t| (format!("Theme: {}", t.name), PalAction::Theme(t.name.clone()))));
        for s in &st.snippets.items {
            v.push((format!("Insert snippet: {}", s.title), PalAction::Insert(s.id)));
            v.push((format!("Copy snippet: {}", s.title), PalAction::Copy(s.id)));
        }
        v.retain(|(l, _)| q.is_empty() || q.split_whitespace().all(|w| l.to_lowercase().contains(w)));
        v
    }
}

impl Render for Palette {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let items = self.items(cx);
        self.sel = self.sel.min(items.len().saturating_sub(1));
        let sel = self.sel;
        div()
            .w(px(620.))
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .bg(p.paper)
            .text_color(p.ink)
            .border_2()
            .border_color(p.ink)
            .shadow_lg()
            .child(self.input.clone())
            .children(items.into_iter().enumerate().take(14).map(|(i, (l, a))| {
                div()
                    .id(("pal", i))
                    .px_2()
                    .py_1()
                    .cursor_pointer()
                    .when(i == sel, |d| d.bg(p.ink).text_color(p.paper))
                    .hover(move |d| d.bg(p.ink).text_color(p.paper))
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(PaletteEvent::Run(a.clone()))))
                    .child(l)
            }))
    }
}

pub struct SnippetsView {
    filter: Entity<Input>,
    title: Entity<Input>,
    tags: Entity<Input>,
    pub note: Option<String>,
}
pub enum SnippetEvent {
    Insert(u64),
}
impl EventEmitter<SnippetEvent> for SnippetsView {}

impl SnippetsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| Input::new("Search snippets", cx));
        cx.subscribe(&filter, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        Self { filter, title: cx.new(|cx| Input::new("Title for new snippet", cx)), tags: cx.new(|cx| Input::new("tags, comma separated", cx)), note: None }
    }

    fn add_from_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(body) = cx.read_from_clipboard().and_then(|c| c.text()) else {
            self.note = Some("Clipboard is empty".into());
            return cx.notify();
        };
        let t = self.title.read(cx).text.clone();
        let title = if t.trim().is_empty() { body.lines().next().unwrap_or("snippet").chars().take(80).collect() } else { t };
        let tags = self.tags.read(cx).text.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        let _ = cx.global_mut::<AppState>().snippets.add(&title, &body, tags, None);
        self.note = Some(format!("Saved \"{title}\""));
        cx.notify();
    }
}

impl Render for SnippetsView {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let q = self.filter.read(cx).text.clone();
        let st = cx.global::<AppState>();
        let list: Vec<_> = st.snippets.search(&q).into_iter().cloned().collect();
        div()
            .id("snippets")
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_6()
            .overflow_y_scroll()
            .bg(p.paper)
            .text_color(p.ink)
            .font_family(st.settings.font_family.clone())
            .text_size(px(13.))
            .child(div().text_size(px(28.)).font_weight(FontWeight::SEMIBOLD).child("Snippets"))
            .child(div().flex().gap_2().child(div().w(px(320.)).child(self.filter.clone())).child(div().w(px(260.)).child(self.title.clone())).child(div().w(px(220.)).child(self.tags.clone())).child(btn("snip-new", "New from clipboard", p).on_click(cx.listener(|v, _, _, cx| v.add_from_clipboard(cx)))))
            .when_some(self.note.clone(), |d, n| d.child(div().text_color(p.accent).child(n)))
            .children(list.into_iter().map(|s| {
                let (id, body) = (s.id, s.body.clone());
                div()
                    .flex()
                    .flex_col()
                    .border_2()
                    .border_color(p.ink)
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .items_center()
                            .px_3()
                            .py_1()
                            .bg(p.ink)
                            .text_color(p.paper)
                            .child(div().flex().gap_3().child(div().font_weight(FontWeight::BOLD).child(s.title.clone())).child(s.tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" ")).child(s.created.format("%b %d %y %I:%M %p").to_string()))
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(div().id(("sc", id)).cursor_pointer().child("copy").on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(body.clone()))))
                                    .child(div().id(("si", id)).cursor_pointer().child("insert").on_click(cx.listener(move |_, _, _, cx| cx.emit(SnippetEvent::Insert(id)))))
                                    .child(div().id(("sd", id)).cursor_pointer().child("delete").on_click(cx.listener(move |_, _, _, cx| {
                                        let _ = cx.global_mut::<AppState>().snippets.remove(id);
                                        cx.notify()
                                    }))),
                            ),
                    )
                    .child(div().px_3().py_2().max_h(px(160.)).overflow_hidden().child(s.body.lines().take(8).collect::<Vec<_>>().join("\n")))
            }))
    }
}
