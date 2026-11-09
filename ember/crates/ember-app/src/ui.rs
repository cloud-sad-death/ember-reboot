//! Shared state, colours and small widgets.
use ember_core::settings::Settings;
use ember_core::snippets::Snippets;
use ember_core::themes::Theme;
use gpui::prelude::FluentBuilder;
use gpui::*;
use std::path::PathBuf;

pub struct AppState {
    pub settings: Settings,
    pub themes: Vec<Theme>,
    pub root: PathBuf,
    pub shell_dir: PathBuf,
    pub snippets: Snippets,
    pub pills: Vec<(String, String)>,
    pub host: String,
}
impl Global for AppState {}

impl AppState {
    pub fn theme(&self) -> Theme {
        self.themes.iter().find(|t| t.name == self.settings.theme).cloned().unwrap_or_else(|| self.themes[0].clone())
    }
    pub fn save(&self) {
        let _ = self.settings.save(&ember_core::paths::config_file());
    }
}

/// Report/UI colours derived from the active theme (ink on paper, like the template).
#[derive(Clone, Copy)]
pub struct Pal {
    pub paper: Hsla,
    pub ink: Hsla,
    pub muted: Hsla,
    pub line: Hsla,
    pub accent: Hsla,
    pub bad: Hsla,
    pub hit: Hsla,
    pub std_light: Hsla,
    pub stream_light: Hsla,
}

pub fn c(v: u32) -> Hsla {
    rgb(v).into()
}
pub fn ca(v: u32, a: f32) -> Hsla {
    let mut h: Hsla = rgb(v).into();
    h.a = a;
    h
}

pub fn pal(cx: &App) -> Pal {
    let t = cx.global::<AppState>().theme();
    Pal {
        paper: c(t.background),
        ink: c(t.foreground),
        muted: ca(t.foreground, 0.55),
        line: ca(t.foreground, 0.85),
        accent: c(t.accent),
        bad: c(t.palette[1]),
        hit: ca(t.palette[3], 0.45),
        std_light: c(t.palette[1]),
        stream_light: c(t.palette[5]),
    }
}

pub fn mono(cx: &App) -> SharedString {
    cx.global::<AppState>().settings.font_family.clone().into()
}

/// A bordered monoblock button.
pub fn btn(id: impl Into<ElementId>, label: impl Into<SharedString>, p: Pal) -> Stateful<Div> {
    div()
        .id(id.into())
        .px_2()
        .py(px(2.))
        .border_1()
        .border_color(p.ink)
        .text_color(p.ink)
        .cursor_pointer()
        .hover(move |s| s.bg(p.ink).text_color(p.paper))
        .child(label.into())
}

pub fn pill(k: &str, v: &str, p: Pal) -> Div {
    div()
        .flex()
        .flex_col()
        .px_2()
        .py_1()
        .border_1()
        .border_color(p.paper)
        .bg(p.ink)
        .text_color(p.paper)
        .child(div().text_size(px(10.)).font_weight(FontWeight::EXTRA_BOLD).child(k.to_uppercase()))
        .child(div().text_size(px(12.)).child(if v.is_empty() { "Unknown".to_string() } else { v.to_string() }))
}

/// Single-line text input (key handling only; enough for search, filters and settings).
pub struct Input {
    pub text: String,
    pub placeholder: SharedString,
    pub focus: FocusHandle,
}

pub enum InputEvent {
    Changed,
    Submit,
    Up,
    Down,
    Escape,
}
impl EventEmitter<InputEvent> for Input {}

impl Input {
    pub fn new(placeholder: &str, cx: &mut Context<Self>) -> Self {
        Self { text: String::new(), placeholder: placeholder.to_string().into(), focus: cx.focus_handle() }
    }
    pub fn set(&mut self, t: &str, cx: &mut Context<Self>) {
        self.text = t.to_string();
        cx.notify();
    }
    fn key(&mut self, e: &KeyDownEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        if k.modifiers.platform || k.modifiers.control {
            if k.key == "v" {
                if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.text.push_str(t.lines().next().unwrap_or(""));
                    cx.emit(InputEvent::Changed);
                }
            } else if k.key == "a" || k.key == "u" {
                self.text.clear();
                cx.emit(InputEvent::Changed);
            }
            cx.notify();
            return;
        }
        match k.key.as_str() {
            "backspace" => {
                self.text.pop();
                cx.emit(InputEvent::Changed);
            }
            "enter" => cx.emit(InputEvent::Submit),
            "escape" => cx.emit(InputEvent::Escape),
            "up" => cx.emit(InputEvent::Up),
            "down" => cx.emit(InputEvent::Down),
            _ => {
                if let Some(ch) = &k.key_char {
                    if !ch.chars().any(|c| c.is_control()) {
                        self.text.push_str(ch);
                        cx.emit(InputEvent::Changed);
                    }
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for Input {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let focused = self.focus.is_focused(window);
        let empty = self.text.is_empty();
        div()
            .id("input")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .on_click(cx.listener(|this, _, w, _| this.focus.focus(w)))
            .flex()
            .items_center()
            .min_w(px(120.))
            .px_2()
            .py_1()
            .border_2()
            .border_color(p.ink)
            .bg(p.paper)
            .text_color(if empty { p.muted } else { p.ink })
            .overflow_hidden()
            .child(if empty { self.placeholder.to_string() } else { self.text.clone() })
            .when(focused, |d| d.child(div().w(px(7.)).h(px(15.)).ml(px(1.)).bg(p.accent)))
    }
}
