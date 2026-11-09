//! One readable config file (TOML). Every GUI setting lives here.
use crate::recorder::Mode;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The template's REPORT CONFIG block, key for key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ReportSettings {
    pub output_font_px: f32,
    pub output_line_height: f32,
    pub row_font_px: f32,
    pub timestamp_font_px: f32,
    pub toc_font_px: f32,
    pub maximum_indent_percent: f32,
    pub part_columns: u32,
    pub part_button_width_px: f32,
    pub part_button_height_px: f32,
    pub scroll_duration_ms: u32,
    pub output_block_chars: u32,
    pub collapse_outputs_initially: bool,
    pub sidebar_width_px: f32,
    /// Rows per pager segment in the report view (navigation only; nothing is split on disk).
    pub segment_rows: u32,
}

impl Default for ReportSettings {
    fn default() -> Self {
        Self {
            output_font_px: 13.0,
            output_line_height: 1.8,
            row_font_px: 14.0,
            timestamp_font_px: 13.0,
            toc_font_px: 13.0,
            maximum_indent_percent: 25.0,
            part_columns: 5,
            part_button_width_px: 48.0,
            part_button_height_px: 40.0,
            scroll_duration_ms: 220,
            output_block_chars: 32768,
            collapse_outputs_initially: false,
            sidebar_width_px: 300.0,
            segment_rows: 500,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub theme: String,
    pub opacity: f32,
    pub blur: bool,
    pub font_family: String,
    pub font_size: f32,
    pub line_height: f32,
    pub padding: f32,
    pub cursor_style: String,
    pub cursor_blink: bool,
    pub pane_border_px: f32,
    pub recording_light: String,
    pub default_mode: Mode,
    pub shell: String,
    pub slice_interval_ms: u64,
    pub slice_on_refresh: bool,
    pub data_dir: String,
    pub retention_max_gb: f64,
    pub retention_max_days: u64,
    pub index_stream_max_mb_per_row: u64,
    pub recompress_level: i32,
    pub export_part_mb: u64,
    pub report: ReportSettings,
    pub keybindings: BTreeMap<String, String>,
}

pub fn default_keybindings() -> BTreeMap<String, String> {
    [
        ("new_window", "cmd-n"),
        ("new_tab", "cmd-t"),
        ("close", "cmd-w"),
        ("split_right", "cmd-d"),
        ("split_down", "cmd-shift-d"),
        ("focus_next_pane", "cmd-]"),
        ("focus_prev_pane", "cmd-["),
        ("next_tab", "cmd-shift-]"),
        ("prev_tab", "cmd-shift-["),
        ("toggle_record_standard", "cmd-r"),
        ("toggle_record_streaming", "cmd-shift-r"),
        ("report_view", "cmd-1"),
        ("terminal_view", "cmd-2"),
        ("search_view", "cmd-shift-f"),
        ("search", "cmd-f"),
        ("palette", "cmd-k"),
        ("settings", "cmd-,"),
        ("copy", "cmd-c"),
        ("paste", "cmd-v"),
    ]
    .into_iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "Rosé Pine Dawn".into(),
            opacity: 0.86,
            blur: true,
            font_family: "JetBrains Mono".into(),
            font_size: 13.0,
            line_height: 1.35,
            padding: 12.0,
            cursor_style: "block".into(),
            cursor_blink: false,
            pane_border_px: 1.0,
            recording_light: "dot".into(),
            default_mode: Mode::Off,
            shell: "/bin/zsh".into(),
            slice_interval_ms: 5000,
            slice_on_refresh: true,
            data_dir: String::new(),
            retention_max_gb: 0.0,
            retention_max_days: 0,
            index_stream_max_mb_per_row: 0,
            recompress_level: 9,
            export_part_mb: 25,
            report: ReportSettings::default(),
            keybindings: default_keybindings(),
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default()
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
    pub fn data_root(&self) -> std::path::PathBuf {
        if self.data_dir.trim().is_empty() {
            crate::paths::data_dir()
        } else {
            std::path::PathBuf::from(shellexpand(&self.data_dir))
        }
    }
    /// The REPORT CONFIG block as shown in the report's config row.
    pub fn report_config_text(&self) -> String {
        format!(
            "# BEGIN REPORT CONFIG — editable report controls only\nEXPORT_PART_MB=\"{}\"\nREPORT_SETTINGS=\n{}\n# END REPORT CONFIG\n",
            self.export_part_mb,
            serde_json::to_string_pretty(&self.report).unwrap_or_default()
        )
    }
}

fn shellexpand(s: &str) -> String {
    match (s.strip_prefix("~/"), dirs::home_dir()) {
        (Some(rest), Some(h)) => h.join(rest).display().to_string(),
        _ => s.to_string(),
    }
}
