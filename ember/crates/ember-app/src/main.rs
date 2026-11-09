//! Ember: a native, GPU-rendered terminal that records, slices and searches everything.
mod palette;
mod report;
mod searchview;
mod settingsview;
mod term;
mod ui;
mod workspace;

use ember_core::settings::Settings;
use gpui::*;
use std::time::Duration;
use ui::AppState;
use workspace::{View, Workspace};

macro_rules! commands {
    ($($act:ident => $name:literal),* $(,)?) => {
        actions!(ember, [$($act),*, Quit]);
        pub fn bind_actions(d: Div, cx: &mut Context<Workspace>) -> Div {
            d $(.on_action(cx.listener(|ws, _: &$act, w, cx| ws.command($name, w, cx))))*
        }
        fn key_bindings(s: &Settings) -> Vec<KeyBinding> {
            let mut v = vec![KeyBinding::new("cmd-q", Quit, None)];
            $(if let Some(k) = s.keybindings.get($name) { v.push(KeyBinding::new(k, $act, Some("Workspace"))); })*
            v
        }
    };
}

commands! {
    NewWindow => "new_window",
    NewTab => "new_tab",
    Close => "close",
    SplitRight => "split_right",
    SplitDown => "split_down",
    FocusNext => "focus_next_pane",
    FocusPrev => "focus_prev_pane",
    NextTab => "next_tab",
    PrevTab => "prev_tab",
    RecordStandard => "toggle_record_standard",
    RecordStreaming => "toggle_record_streaming",
    ShowReport => "report_view",
    ShowTerminal => "terminal_view",
    ShowSearch => "search_view",
    FindInReport => "search",
    OpenPalette => "palette",
    ShowSettings => "settings",
    Copy => "copy",
    Paste => "paste",
}

pub fn open_window(cx: &mut App) {
    let s = cx.global::<AppState>().settings.clone();
    let bg = if s.blur {
        WindowBackgroundAppearance::Blurred
    } else if s.opacity < 1.0 {
        WindowBackgroundAppearance::Transparent
    } else {
        WindowBackgroundAppearance::Opaque
    };
    let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions { title: Some("Ember".into()), appears_transparent: true, traffic_light_position: Some(point(px(10.), px(9.))) }),
        window_background: bg,
        app_id: Some("ember".into()),
        ..Default::default()
    };
    let _ = cx.open_window(opts, |window, cx| cx.new(|cx| Workspace::new(window, cx)));
}

fn main() {
    env_logger::init();
    let mut settings = Settings::load(&ember_core::paths::config_file());
    if settings.keybindings.is_empty() {
        settings.keybindings = ember_core::settings::default_keybindings();
    }
    let root = settings.data_root();
    let _ = std::fs::create_dir_all(&root);
    let shell_dir = ember_core::shell::install(&root).expect("install shell integration");
    let _ = ember_core::store::prune(&root, (settings.retention_max_gb * 1e9) as u64, settings.retention_max_days);
    let demo = std::env::var("EMBER_DEMO").ok();
    if demo.is_some() {
        settings.default_mode = ember_core::recorder::Mode::Standard;
    }
    Application::new().run(move |cx: &mut App| {
        cx.set_global(AppState {
            themes: ember_core::themes::all(&root),
            snippets: ember_core::snippets::Snippets::load(&root),
            pills: ember_core::host::pills(),
            host: ember_core::host::hostname(),
            settings: settings.clone(),
            root: root.clone(),
            shell_dir: shell_dir.clone(),
        });
        let _ = settings.save(&ember_core::paths::config_file());
        cx.bind_keys(key_bindings(&settings));
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &NewWindow, cx| open_window(cx));
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        open_window(cx);
        cx.activate(true);
        if let Some(d) = demo {
            cx.spawn(async move |cx: &mut AsyncApp| demo_script(d, cx).await).detach();
        }
    });
}

/// CI screenshots: drive a window like a person would.
async fn demo_script(kind: String, cx: &mut AsyncApp) {
    let ex = cx.background_executor().clone();
    let wait = |ms| ex.timer(Duration::from_millis(ms));
    wait(2500).await;
    let Some(win) = cx.update(|cx| cx.windows().first().and_then(|w| w.downcast::<Workspace>())).ok().flatten() else { return };
    let type_in = |s: &'static str, cx: &mut AsyncApp| {
        let _ = win.update(cx, |ws, _, cx| {
            if let Some(t) = ws.focused() {
                t.update(cx, |t, _| {
                    let _ = t.pane.write(s.as_bytes());
                });
            }
        });
    };
    type_in("mkdir -p /tmp/ember-demo && for n in alpha beta gamma; do print \"query $n\" > /tmp/ember-demo/$n.query; done\r", cx);
    wait(1200).await;
    type_in("find /tmp/ember-demo -name '*.query' -exec cat {} \\;\r", cx);
    wait(1200).await;
    type_in("ls -la /tmp/ember-demo; false\r", cx);
    wait(800).await;
    let _ = win.update(cx, |ws, w, cx| ws.split(true, w, cx));
    wait(1500).await;
    type_in("echo 'second pane'; uname -a\r", cx);
    wait(1500).await;
    if kind == "report" {
        let _ = win.update(cx, |ws, w, cx| {
            ws.command("focus_prev_pane", w, cx);
            ws.show(View::Report, w, cx)
        });
    }
}
