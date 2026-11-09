//! Settings pane: every setting in the GUI; saved to one readable TOML file on each change.
use crate::ui::{btn, c, pal, AppState, Input, InputEvent, Pal};
use ember_core::recorder::Mode;
use ember_core::settings::Settings;
use gpui::prelude::FluentBuilder;
use gpui::*;

type Get = fn(&Settings) -> f64;
type Set = fn(&mut Settings, f64);

struct Num {
    label: &'static str,
    get: Get,
    set: Set,
    step: f64,
    min: f64,
    max: f64,
}

macro_rules! num {
    ($label:expr, $($f:ident).+, $step:expr, $min:expr, $max:expr, $t:ty) => {
        Num { label: $label, get: |s| s.$($f).+ as f64, set: |s, v| s.$($f).+ = v as $t, step: $step, min: $min, max: $max }
    };
}

fn nums() -> Vec<(&'static str, Vec<Num>)> {
    vec![
        ("Window", vec![num!("Opacity", opacity, 0.02, 0.05, 1.0, f32), num!("Padding px", padding, 1., 0., 48., f32), num!("Pane border px", pane_border_px, 1., 0., 6., f32)]),
        ("Font", vec![num!("Font size", font_size, 0.5, 8., 32., f32), num!("Line height", line_height, 0.05, 1., 2.5, f32)]),
        (
            "Recording",
            vec![num!("Streaming slice interval ms", slice_interval_ms, 500., 500., 60000., u64), num!("Index limit per stream row MB (0 = all)", index_stream_max_mb_per_row, 10., 0., 100000., u64)],
        ),
        (
            "Storage",
            vec![num!("Retention max GB (0 = none)", retention_max_gb, 5., 0., 100000., f64), num!("Retention max days (0 = none)", retention_max_days, 7., 0., 3650., u64), num!("Background recompress level", recompress_level, 1., 1., 19., i32), num!("Export part size MB", export_part_mb, 5., 1., 2000., u64)],
        ),
        (
            "Report view",
            vec![
                num!("Output font px", report.output_font_px, 0.5, 6., 48., f32),
                num!("Output line height", report.output_line_height, 0.1, 1., 4., f32),
                num!("Row font px", report.row_font_px, 0.5, 6., 48., f32),
                num!("Timestamp font px", report.timestamp_font_px, 0.5, 6., 48., f32),
                num!("Index font px", report.toc_font_px, 0.5, 6., 48., f32),
                num!("Maximum indent %", report.maximum_indent_percent, 1., 0., 50., f32),
                num!("Part columns", report.part_columns, 1., 1., 10., u32),
                num!("Part button width px", report.part_button_width_px, 2., 16., 96., f32),
                num!("Part button height px", report.part_button_height_px, 2., 16., 96., f32),
                num!("Scroll duration ms", report.scroll_duration_ms, 20., 0., 2000., u32),
                num!("Output block chars", report.output_block_chars, 4096., 256., 1048576., u32),
                num!("Rows per pager segment", report.segment_rows, 50., 10., 100000., u32),
                num!("Index width px", report.sidebar_width_px, 10., 180., 600., f32),
            ],
        ),
    ]
}

pub struct SettingsView {
    font: Entity<Input>,
    data_dir: Entity<Input>,
    shell: Entity<Input>,
    keys: Vec<(String, Entity<Input>)>,
    note: Option<String>,
}

impl SettingsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let s = cx.global::<AppState>().settings.clone();
        let field = |init: &str, f: fn(&mut Settings, String), cx: &mut Context<Self>| {
            let e = cx.new(|cx| {
                let mut i = Input::new("", cx);
                i.text = init.to_string();
                i
            });
            cx.subscribe(&e, move |_, inp, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Changed | InputEvent::Submit) {
                    let t = inp.read(cx).text.clone();
                    f(&mut cx.global_mut::<AppState>().settings, t);
                    cx.global::<AppState>().save();
                    cx.refresh_windows();
                }
            })
            .detach();
            e
        };
        let font = field(&s.font_family, |s, t| s.font_family = t, cx);
        let data_dir = field(&s.data_dir, |s, t| s.data_dir = t, cx);
        let shell = field(&s.shell, |s, t| s.shell = t, cx);
        let keys = s
            .keybindings
            .iter()
            .map(|(k, v)| {
                let name = k.clone();
                let e = cx.new(|cx| {
                    let mut i = Input::new("", cx);
                    i.text = v.clone();
                    i
                });
                let n2 = name.clone();
                cx.subscribe(&e, move |v, inp, ev: &InputEvent, cx| {
                    if matches!(ev, InputEvent::Submit) {
                        let t = inp.read(cx).text.clone();
                        cx.global_mut::<AppState>().settings.keybindings.insert(n2.clone(), t);
                        cx.global::<AppState>().save();
                        v.note = Some("Keybinding saved; takes effect on next launch.".into());
                        cx.notify();
                    }
                })
                .detach();
                (name, e)
            })
            .collect();
        Self { font, data_dir, shell, keys, note: None }
    }

    fn change(f: impl FnOnce(&mut Settings), cx: &mut App) {
        f(&mut cx.global_mut::<AppState>().settings);
        cx.global::<AppState>().save();
        cx.refresh_windows();
    }

    fn import_theme(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: None });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = this.update(cx, |v, cx| {
                    let root = cx.global::<AppState>().root.clone();
                    match ember_core::themes::import(&root, &paths[0]) {
                        Ok(t) => {
                            let name = t.name.clone();
                            let st = cx.global_mut::<AppState>();
                            st.themes = ember_core::themes::all(&root);
                            st.settings.theme = name.clone();
                            st.save();
                            v.note = Some(format!("Imported theme {name}"));
                        }
                        Err(e) => v.note = Some(format!("Import failed: {e}")),
                    }
                    cx.refresh_windows();
                });
            }
        })
        .detach();
    }
}

fn section(title: &str, p: Pal) -> Div {
    div().pt_4().pb_1().border_b_2().border_color(p.ink).text_size(px(16.)).font_weight(FontWeight::BOLD).child(title.to_string())
}

fn row(label: &str, control: impl IntoElement) -> Div {
    div().flex().items_center().justify_between().py_1().child(label.to_string()).child(control)
}

impl Render for SettingsView {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let st = cx.global::<AppState>();
        let s = st.settings.clone();
        let themes = st.themes.clone();
        let mut col = div().flex().flex_col().gap_1().w(px(760.));
        col = col.child(section("Theme", p)).child(
            div().flex().flex_wrap().gap_2().children(themes.into_iter().enumerate().map(|(i, t)| {
                let name = t.name.clone();
                let on = name == s.theme;
                div()
                    .id(("theme", i))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .border_2()
                    .border_color(if on { p.accent } else { p.ink })
                    .bg(c(t.background))
                    .text_color(c(t.foreground))
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        let n = name.clone();
                        Self::change(move |s| s.theme = n, cx)
                    })
                    .child(div().flex().children(t.palette[1..7].iter().map(|&pc| div().w(px(8.)).h(px(14.)).bg(c(pc)))))
                    .child(t.name.clone())
            })),
        );
        col = col.child(row("Import a Ghostty theme file", btn("import-theme", "Import…", p).on_click(cx.listener(|v, _, _, cx| v.import_theme(cx)))));
        col = col.child(row("Window blur", btn("blur", if s.blur { "On" } else { "Off" }, p).on_click(|_, _, cx| Self::change(|s| s.blur = !s.blur, cx))));
        col = col.child(row("Font family", div().w(px(300.)).child(self.font.clone())));
        col = col.child(row("Cursor", btn("cursor", s.cursor_style.clone(), p).on_click(|_, _, cx| Self::change(|s| s.cursor_style = next(&s.cursor_style, &["block", "beam", "underline"]), cx))));
        col = col.child(row("Recording light", btn("light", s.recording_light.clone(), p).on_click(|_, _, cx| Self::change(|s| s.recording_light = next(&s.recording_light, &["dot", "pill", "bar"]), cx))));
        col = col.child(row(
            "Recording in new panes",
            btn("defmode", s.default_mode.as_str(), p).on_click(|_, _, cx| {
                Self::change(
                    |s| {
                        s.default_mode = match s.default_mode {
                            Mode::Off => Mode::Standard,
                            Mode::Standard => Mode::Streaming,
                            Mode::Streaming => Mode::Off,
                        }
                    },
                    cx,
                )
            }),
        ));
        col = col.child(row("Streaming: slice at each screen refresh", btn("refresh", if s.slice_on_refresh { "On" } else { "Off" }, p).on_click(|_, _, cx| Self::change(|s| s.slice_on_refresh = !s.slice_on_refresh, cx))));
        col = col.child(row("Collapse outputs on open", btn("collapse", if s.report.collapse_outputs_initially { "On" } else { "Off" }, p).on_click(|_, _, cx| Self::change(|s| s.report.collapse_outputs_initially = !s.report.collapse_outputs_initially, cx))));
        col = col.child(row("Shell", div().w(px(300.)).child(self.shell.clone())));
        col = col.child(row("Data folder (empty = ~/Library/Application Support/Ember)", div().w(px(300.)).child(self.data_dir.clone())));
        for (title, items) in nums() {
            col = col.child(section(title, p));
            for (i, n) in items.into_iter().enumerate() {
                let v = (n.get)(&s);
                let (get, set, step, min, max) = (n.get, n.set, n.step, n.min, n.max);
                let id = SharedString::from(format!("{title}-{i}"));
                col = col.child(row(
                    n.label,
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(btn(SharedString::from(format!("{id}-dec")), "−", p).on_click(move |_, _, cx| Self::change(move |s| set(s, (get(s) - step).clamp(min, max)), cx)))
                        .child(div().w(px(90.)).flex().justify_center().child(if step < 1. { format!("{v:.2}") } else { format!("{v:.0}") }))
                        .child(btn(SharedString::from(format!("{id}-inc")), "+", p).on_click(move |_, _, cx| Self::change(move |s| set(s, (get(s) + step).clamp(min, max)), cx))),
                ));
            }
        }
        col = col.child(section("Keybindings (Enter to save)", p));
        for (k, e) in &self.keys {
            col = col.child(row(k, div().w(px(220.)).child(e.clone())));
        }
        col = col.child(div().pt_4().text_color(p.muted).child(format!("Config file: {}", ember_core::paths::config_file().display())));
        div()
            .id("settings")
            .size_full()
            .overflow_y_scroll()
            .p_6()
            .bg(p.paper)
            .text_color(p.ink)
            .font_family(s.font_family.clone())
            .text_size(px(13.))
            .child(div().text_size(px(28.)).font_weight(FontWeight::SEMIBOLD).child("Settings"))
            .when_some(self.note.clone(), |d, n| d.child(div().py_2().text_color(p.accent).child(n)))
            .child(col)
    }
}

fn next(cur: &str, opts: &[&str]) -> String {
    let i = opts.iter().position(|o| *o == cur).unwrap_or(0);
    opts[(i + 1) % opts.len()].to_string()
}
