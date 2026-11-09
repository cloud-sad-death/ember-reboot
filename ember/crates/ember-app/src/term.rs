//! A terminal pane: Ember owns the PTY; alacritty_terminal keeps the screen; GPUI paints it on the GPU.
use crate::ui::{c, pal, AppState};
use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as TPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor};
use crossbeam_channel::{unbounded, Receiver, Sender};
use ember_core::pty::{Pane, SpawnOpts};
use ember_core::recorder::{Mode, RecorderConfig};
use ember_core::themes::Theme;
use gpui::prelude::FluentBuilder;
use gpui::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct Listener(Sender<TermEvent>);
impl EventListener for Listener {
    fn send_event(&self, e: TermEvent) {
        let _ = self.0.send(e);
    }
}

pub struct TerminalView {
    pub term: Arc<FairMutex<Term<Listener>>>,
    pub pane: Pane,
    pub focus: FocusHandle,
    pub mode: Mode,
    pub title: String,
    pub exited: Arc<AtomicBool>,
    grid: (u16, u16),
    cell: (f32, f32),
    events: Receiver<TermEvent>,
    dirty: Arc<AtomicBool>,
    selecting: bool,
    origin: Point<Pixels>,
    _poll: Task<()>,
}

pub enum TermViewEvent {
    Exited,
    ModeChanged,
}
impl EventEmitter<TermViewEvent> for TerminalView {}

impl TerminalView {
    pub fn new(cwd: Option<PathBuf>, cx: &mut Context<Self>) -> anyhow::Result<Self> {
        let st = cx.global::<AppState>();
        let s = &st.settings;
        let (tx, rx) = unbounded();
        let size = TermSize::new(100, 30);
        let config = Config { scrolling_history: 100_000, ..Config::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, Listener(tx))));
        let dirty = Arc::new(AtomicBool::new(true));
        let exited = Arc::new(AtomicBool::new(false));
        let rec = RecorderConfig {
            root: st.root.clone(),
            host: st.host.clone(),
            pills: st.pills.clone(),
            slice_interval: Duration::from_millis(s.slice_interval_ms.max(250)),
            slice_on_refresh: s.slice_on_refresh,
            index_max: s.index_stream_max_mb_per_row * 1_000_000,
            recompress_level: s.recompress_level,
        };
        let (t2, d2, e2) = (term.clone(), dirty.clone(), exited.clone());
        let mut parser: Processor = Processor::new();
        let pane = Pane::spawn(
            SpawnOpts { shell: &s.shell, cwd, shell_dir: &st.shell_dir, rows: 30, cols: 100, extra_env: vec![] },
            rec,
            s.default_mode,
            move |bytes| {
                parser.advance(&mut *t2.lock(), bytes);
                d2.store(true, Ordering::Release);
            },
            move || e2.store(true, Ordering::Release),
        )?;
        let mode = s.default_mode;
        let poll = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| loop {
            cx.background_executor().timer(Duration::from_millis(8)).await;
            let ok = this.update(cx, |v, cx| v.poll(cx));
            if ok.is_err() {
                break;
            }
        });
        Ok(Self { term, pane, focus: cx.focus_handle(), mode, title: "zsh".into(), exited, grid: (30, 100), cell: (8., 17.), events: rx, dirty, selecting: false, origin: point(px(0.), px(0.)), _poll: poll })
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        while let Ok(e) = self.events.try_recv() {
            match e {
                TermEvent::PtyWrite(s) => {
                    let _ = self.pane.write(s.as_bytes());
                }
                TermEvent::Title(t) => self.title = t,
                TermEvent::ResetTitle => self.title = "zsh".into(),
                _ => {}
            }
        }
        if self.exited.swap(false, Ordering::AcqRel) {
            cx.emit(TermViewEvent::Exited);
        }
        if self.dirty.swap(false, Ordering::AcqRel) {
            cx.notify();
        }
    }

    pub fn set_mode(&mut self, m: Mode, cx: &mut Context<Self>) {
        self.mode = m;
        self.pane.set_mode(m);
        cx.emit(TermViewEvent::ModeChanged);
        cx.notify();
    }

    pub fn session_dir(&self) -> Option<PathBuf> {
        self.pane.recorder.lock().session_dir()
    }

    pub fn paste(&mut self, text: &str) {
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        let t = text.replace("\r\n", "\r").replace('\n', "\r");
        let payload = if bracketed { format!("\x1b[200~{t}\x1b[201~") } else { t };
        let _ = self.pane.write(payload.as_bytes());
    }

    pub fn copy(&self) -> Option<String> {
        self.term.lock().selection_to_string()
    }

    fn key(&mut self, e: &KeyDownEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let app_cursor = self.term.lock().mode().contains(TermMode::APP_CURSOR);
        if let Some(b) = key_bytes(&e.keystroke, app_cursor) {
            self.term.lock().scroll_display(Scroll::Bottom);
            self.term.lock().selection = None;
            let _ = self.pane.write(&b);
            cx.stop_propagation();
        }
    }

    fn point(&self, pos: Point<Pixels>, origin: Point<Pixels>) -> (TPoint, Side) {
        let pad = 0.0;
        let x = (f32::from(pos.x - origin.x) - pad).max(0.0);
        let y = (f32::from(pos.y - origin.y) - pad).max(0.0);
        let col = ((x / self.cell.0) as usize).min(self.grid.1.saturating_sub(1) as usize);
        let row = ((y / self.cell.1) as i32).min(self.grid.0 as i32 - 1);
        let off = self.term.lock().grid().display_offset() as i32;
        let side = if (x / self.cell.0).fract() < 0.5 { Side::Left } else { Side::Right };
        (TPoint::new(Line(row - off), Column(col)), side)
    }
}

pub fn key_bytes(k: &Keystroke, app_cursor: bool) -> Option<Vec<u8>> {
    let m = &k.modifiers;
    if m.platform {
        return None;
    }
    let arrow = |c: u8| if app_cursor { vec![0x1b, b'O', c] } else { vec![0x1b, b'[', c] };
    let named: Option<Vec<u8>> = match k.key.as_str() {
        "enter" => Some(b"\r".to_vec()),
        "backspace" => Some(if m.alt { b"\x1b\x7f".to_vec() } else { b"\x7f".to_vec() }),
        "tab" => Some(if m.shift { b"\x1b[Z".to_vec() } else { b"\t".to_vec() }),
        "escape" => Some(b"\x1b".to_vec()),
        "up" => Some(arrow(b'A')),
        "down" => Some(arrow(b'B')),
        "right" => Some(if m.alt { b"\x1bf".to_vec() } else { arrow(b'C') }),
        "left" => Some(if m.alt { b"\x1bb".to_vec() } else { arrow(b'D') }),
        "home" => Some(b"\x1b[H".to_vec()),
        "end" => Some(b"\x1b[F".to_vec()),
        "pageup" => Some(b"\x1b[5~".to_vec()),
        "pagedown" => Some(b"\x1b[6~".to_vec()),
        "delete" => Some(b"\x1b[3~".to_vec()),
        "f1" => Some(b"\x1bOP".to_vec()),
        "f2" => Some(b"\x1bOQ".to_vec()),
        "f3" => Some(b"\x1bOR".to_vec()),
        "f4" => Some(b"\x1bOS".to_vec()),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    if m.control {
        let ch = k.key.chars().next()?;
        if k.key.len() == 1 && ch.is_ascii() {
            let b = (ch.to_ascii_lowercase() as u8) & 0x1f;
            return Some(vec![if ch == ' ' || ch == '@' { 0 } else { b }]);
        }
        if k.key == "space" {
            return Some(vec![0]);
        }
    }
    let s = k.key_char.clone().or_else(|| (k.key == "space").then(|| " ".to_string()))?;
    let mut out = Vec::new();
    if m.alt {
        out.push(0x1b);
    }
    out.extend_from_slice(s.as_bytes());
    Some(out)
}

fn xterm256(i: u8, t: &Theme) -> u32 {
    match i {
        0..=15 => t.palette[i as usize],
        16..=231 => {
            let i = i - 16;
            let v = |x: u8| if x == 0 { 0 } else { 55 + x as u32 * 40 };
            (v(i / 36) << 16) | (v((i / 6) % 6) << 8) | v(i % 6)
        }
        _ => {
            let g = 8 + (i as u32 - 232) * 10;
            (g << 16) | (g << 8) | g
        }
    }
}

fn color(col: &Color, t: &Theme, fg: bool) -> u32 {
    match col {
        Color::Spec(rgb) => ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32,
        Color::Indexed(i) => xterm256(*i, t),
        Color::Named(n) => match n {
            NamedColor::Foreground | NamedColor::BrightForeground => t.foreground,
            NamedColor::DimForeground => t.foreground,
            NamedColor::Background => t.background,
            NamedColor::Cursor => t.cursor,
            other => {
                let i = *other as usize;
                if i < 16 {
                    t.palette[i]
                } else if i >= 259 && i < 267 {
                    t.palette[i - 259]
                } else if fg {
                    t.foreground
                } else {
                    t.background
                }
            }
        },
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let st = cx.global::<AppState>();
        let theme = st.theme();
        let s = st.settings.clone();
        let font = gpui::font(s.font_family.clone());
        let font_size = px(s.font_size);
        let ts = window.text_system();
        let fid = ts.resolve_font(&font);
        let cw = ts.advance(fid, font_size, 'm').map(|a| f32::from(a.width)).unwrap_or(s.font_size * 0.6);
        let ch = (s.font_size * s.line_height).round();
        self.cell = (cw, ch);
        let term = self.term.clone();
        let entity = cx.entity();
        let focused = self.focus.is_focused(window);
        let p = pal(cx);
        let cursor_style = s.cursor_style.clone();
        div()
            .id("terminal")
            .track_focus(&self.focus)
            .key_context("Terminal")
            .on_key_down(cx.listener(Self::key))
            .size_full()
            .p(px(s.padding))
            .on_scroll_wheel(cx.listener(move |this, e: &ScrollWheelEvent, _, cx| {
                let lines = match e.delta {
                    ScrollDelta::Lines(l) => l.y * 3.0,
                    ScrollDelta::Pixels(p) => f32::from(p.y) / this.cell.1,
                };
                let n = lines.round() as i32;
                if n == 0 {
                    return;
                }
                let alt = this.term.lock().mode().contains(TermMode::ALT_SCREEN);
                if alt {
                    let seq: &[u8] = if n > 0 { b"\x1b[A" } else { b"\x1b[B" };
                    for _ in 0..n.unsigned_abs() {
                        let _ = this.pane.write(seq);
                    }
                } else {
                    this.term.lock().scroll_display(Scroll::Delta(n));
                }
                cx.notify();
            }))
            .child(
                canvas(
                    move |bounds, _w, cx| {
                        let cols = ((f32::from(bounds.size.width) / cw).floor() as u16).max(10);
                        let rows = ((f32::from(bounds.size.height) / ch).floor() as u16).max(3);
                        entity.update(cx, |v, _| {
                            v.origin = bounds.origin;
                            if (rows, cols) != v.grid {
                                v.grid = (rows, cols);
                                v.term.lock().resize(TermSize::new(cols as usize, rows as usize));
                                v.pane.resize(rows, cols);
                            }
                        });
                        bounds
                    },
                    move |bounds, _, window, cx| {
                        let t = term.lock();
                        let content = t.renderable_content();
                        let off = content.display_offset as i32;
                        let rows = t.screen_lines();
                        let mut lines: Vec<(String, Vec<TextRun>)> = (0..rows).map(|_| (String::new(), Vec::new())).collect();
                        let origin = bounds.origin;
                        let sel = content.selection;
                        for cell in content.display_iter {
                            let r = (cell.point.line.0 + off) as usize;
                            if r >= rows || cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                                continue;
                            }
                            let mut fg = color(&cell.fg, &theme, true);
                            let mut bg = color(&cell.bg, &theme, false);
                            if cell.flags.contains(Flags::INVERSE) {
                                std::mem::swap(&mut fg, &mut bg);
                            }
                            let selected = sel.map_or(false, |s| s.contains(cell.point));
                            if selected {
                                bg = theme.selection;
                            }
                            let x = origin.x + px(cell.point.column.0 as f32 * cw);
                            let y = origin.y + px(r as f32 * ch);
                            if bg != theme.background {
                                window.paint_quad(fill(Bounds::new(point(x, y), size(px(cw + 0.5), px(ch))), c(bg)));
                            }
                            let chr = if cell.flags.contains(Flags::HIDDEN) { ' ' } else { cell.c };
                            let (text, runs) = &mut lines[r];
                            // pad to the cell's column so text stays on the grid
                            while text.chars().count() < cell.point.column.0 {
                                text.push(' ');
                                push_run(runs, 1, &font, c(theme.foreground), false, false);
                            }
                            text.push(chr);
                            let bold = cell.flags.contains(Flags::BOLD);
                            let ul = cell.flags.contains(Flags::UNDERLINE);
                            let mut fgc = c(fg);
                            if cell.flags.contains(Flags::DIM) {
                                fgc.a = 0.6;
                            }
                            push_run(runs, chr.len_utf8(), &font, fgc, bold, ul);
                        }
                        let cur = content.cursor;
                        for (r, (text, runs)) in lines.into_iter().enumerate() {
                            if text.is_empty() {
                                continue;
                            }
                            let shaped = window.text_system().shape_line(text.into(), font_size, &runs, None);
                            let _ = shaped.paint(point(origin.x, origin.y + px(r as f32 * ch)), px(ch), window, cx);
                        }
                        let cr = cur.point.line.0 + off;
                        if cr >= 0 && (cr as usize) < rows {
                            let x = origin.x + px(cur.point.column.0 as f32 * cw);
                            let y = origin.y + px(cr as f32 * ch);
                            let col = ca_hsla(theme.cursor, if focused { 0.85 } else { 0.35 });
                            let b = match cursor_style.as_str() {
                                "beam" => Bounds::new(point(x, y), size(px(2.), px(ch))),
                                "underline" => Bounds::new(point(x, y + px(ch - 2.)), size(px(cw), px(2.))),
                                _ => Bounds::new(point(x, y), size(px(cw), px(ch))),
                            };
                            window.paint_quad(fill(b, col));
                        }
                    },
                )
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, w, cx| {
                    this.focus.focus(w);
                    let (pt, side) = this.point(e.position, this.origin);
                    let ty = if e.click_count >= 3 { SelectionType::Lines } else if e.click_count == 2 { SelectionType::Semantic } else { SelectionType::Simple };
                    this.term.lock().selection = Some(Selection::new(ty, pt, side));
                    this.selecting = true;
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _w, cx| {
                if this.selecting && e.pressed_button == Some(MouseButton::Left) {
                    let (pt, side) = this.point(e.position, this.origin);
                    if let Some(s) = this.term.lock().selection.as_mut() {
                        s.update(pt, side);
                    }
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, _| this.selecting = false))
            .when(!focused, |d| d.opacity(0.92))
            .text_color(p.ink)
    }
}


fn ca_hsla(v: u32, a: f32) -> Hsla {
    crate::ui::ca(v, a)
}

fn push_run(runs: &mut Vec<TextRun>, len: usize, font: &Font, color: Hsla, bold: bool, ul: bool) {
    if let Some(last) = runs.last_mut() {
        if last.color == color && (last.font.weight == FontWeight::BOLD) == bold && last.underline.is_some() == ul {
            last.len += len;
            return;
        }
    }
    let mut f = font.clone();
    if bold {
        f.weight = FontWeight::BOLD;
    }
    runs.push(TextRun {
        len,
        font: f,
        color,
        background_color: None,
        underline: ul.then(|| UnderlineStyle { thickness: px(1.), color: Some(color), wavy: false }),
        strikethrough: None,
    });
}
