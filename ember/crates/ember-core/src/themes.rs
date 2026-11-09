//! Built-in themes (mostly light, pastel) and Ghostty-format theme files.
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Theme {
    pub name: String,
    pub background: u32,
    pub foreground: u32,
    pub cursor: u32,
    pub selection: u32,
    pub palette: [u32; 16],
    /// Report-view ink/paper/accent, derived from the terminal colours unless set.
    pub accent: u32,
    pub light: bool,
}

fn hex(s: &str) -> Option<u32> {
    u32::from_str_radix(s.trim().trim_start_matches('#'), 16).ok()
}

fn t(name: &str, bg: u32, fg: u32, cursor: u32, sel: u32, accent: u32, p: [u32; 16]) -> Theme {
    let light = luminance(bg) > 0.5;
    Theme { name: name.into(), background: bg, foreground: fg, cursor, selection: sel, palette: p, accent, light }
}

pub fn luminance(c: u32) -> f32 {
    let (r, g, b) = (((c >> 16) & 255) as f32, ((c >> 8) & 255) as f32, (c & 255) as f32);
    (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0
}

pub fn builtins() -> Vec<Theme> {
    vec![
        t("Rosé Pine Dawn", 0xfaf4ed, 0x575279, 0x9893a5, 0xdfdad9, 0xd7827e, [0xf2e9e1, 0xb4637a, 0x286983, 0xea9d34, 0x56949f, 0x907aa9, 0xd7827e, 0x575279, 0x9893a5, 0xb4637a, 0x286983, 0xea9d34, 0x56949f, 0x907aa9, 0xd7827e, 0x575279]),
        t("Catppuccin Latte", 0xeff1f5, 0x4c4f69, 0xdc8a78, 0xacb0be, 0x8839ef, [0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xacb0be, 0x6c6f85, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xbcc0cc]),
        t("Nord Light", 0xe5e9f0, 0x2e3440, 0x5e81ac, 0xd8dee9, 0x5e81ac, [0x3b4252, 0xbf616a, 0x8fa173, 0xc5a565, 0x5e81ac, 0xb48ead, 0x6fa3b0, 0xe5e9f0, 0x4c566a, 0xbf616a, 0x8fa173, 0xc5a565, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4]),
        t("Solarized Light", 0xfdf6e3, 0x657b83, 0x586e75, 0xeee8d5, 0x268bd2, [0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5, 0x002b36, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3]),
        t("Ethereal Mist", 0xf7f5ff, 0x3b3552, 0xb79cff, 0xe6defc, 0xb79cff, [0x3b3552, 0xff7a9c, 0x3fbf9f, 0xe8a33d, 0x7b8cff, 0xc77dff, 0x3ab8c9, 0xe9e6f5, 0x7a7394, 0xff97b2, 0x5fd6b5, 0xf2bd62, 0x9aa7ff, 0xd8a2ff, 0x6ad1dd, 0xffffff]),
        t("Ethereal Peach", 0xfff6f0, 0x4a3b36, 0xff9f80, 0xffe2d4, 0xff8f6b, [0x4a3b36, 0xe0566e, 0x5aa57a, 0xd99a2b, 0x5d7fd6, 0xc06ad1, 0x3aa6a8, 0xf3e6df, 0x8c7a73, 0xff7a8f, 0x77c495, 0xf0b54a, 0x7c9cf0, 0xd98ae8, 0x5cc3c4, 0xffffff]),
        t("Ethereal Mint", 0xf2fbf7, 0x2f4a40, 0x3fcf9f, 0xd4f2e6, 0x18b98a, [0x2f4a40, 0xe0607e, 0x23a36f, 0xc9971c, 0x3f7fd6, 0xa86cd6, 0x149fa6, 0xe3f1ea, 0x6f8a80, 0xf07e98, 0x3fc98f, 0xe1b23a, 0x5f9cf0, 0xc08ae8, 0x35c0c6, 0xffffff]),
        t("Neon Sorbet", 0xfdf7ff, 0x2a1f3d, 0xff3fa4, 0xf6d8ff, 0xff3fa4, [0x2a1f3d, 0xff3f6c, 0x00c48c, 0xffa000, 0x3d7bff, 0xff3fa4, 0x00b8d4, 0xf0e6f5, 0x6c5c80, 0xff6b8e, 0x22e0a6, 0xffbf3f, 0x6b9bff, 0xff73c0, 0x2fd6ee, 0xffffff]),
        t("Neon Lagoon", 0xf3fdff, 0x14303a, 0x00b8d9, 0xcff4fb, 0x00a3c4, [0x14303a, 0xf2456e, 0x00b386, 0xf5a300, 0x2f6cf0, 0xb84df0, 0x00a3c4, 0xe1f2f6, 0x5a7680, 0xff6f90, 0x1fd6a3, 0xffc23d, 0x5c90ff, 0xd27cff, 0x2cc9e6, 0xffffff]),
        t("Pastel Lilac", 0xf8f4fc, 0x3e3450, 0x9b7fd4, 0xe8dff5, 0x9b7fd4, [0x3e3450, 0xd4607f, 0x5aa88a, 0xcf9a3a, 0x6f84d4, 0x9b7fd4, 0x4aa3b5, 0xeee8f5, 0x7d7390, 0xe8809a, 0x7cc4a6, 0xe6b65c, 0x8fa1ec, 0xb79cec, 0x6cbfcf, 0xffffff]),
        t("Paper Mono", 0xffffff, 0x000000, 0x000000, 0xe6e6e6, 0x1a05a2, [0x000000, 0xb00020, 0x1b7a3a, 0x8a6500, 0x1a05a2, 0x7a1fa2, 0x00707a, 0xd0d0d0, 0x555555, 0xd0002a, 0x249a4a, 0xa67c00, 0x3d2bd0, 0x9a3fc2, 0x009aa6, 0xffffff]),
        t("Ethereal Sky", 0xf2f7ff, 0x23324d, 0x5b8cff, 0xd9e6ff, 0x7692ff, [0x23324d, 0xe25a7a, 0x34a37e, 0xd39a2c, 0x5b8cff, 0xa774e8, 0x2aa6c2, 0xe3eaf6, 0x66748f, 0xf07c96, 0x55c39c, 0xebb64f, 0x7fa6ff, 0xc095f5, 0x4cc2db, 0xffffff]),
        t("Catppuccin Mocha", 0x1e1e2e, 0xcdd6f4, 0xf5e0dc, 0x45475a, 0xcba6f7, [0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de, 0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8]),
        t("Nord", 0x2e3440, 0xd8dee9, 0xd8dee9, 0x434c5e, 0x88c0d0, [0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0, 0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4]),
        t("Rosé Pine", 0x191724, 0xe0def4, 0x524f67, 0x403d52, 0xebbcba, [0x26233a, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4, 0x6e6a86, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4]),
    ]
}

/// Parses a Ghostty theme file (`palette = N=#rrggbb`, `background = …`, `foreground = …`, …).
pub fn parse_ghostty(name: &str, text: &str) -> Option<Theme> {
    let base = builtins().into_iter().find(|t| t.name == "Paper Mono")?;
    let mut th = Theme { name: name.into(), ..base };
    let mut seen = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "background" => th.background = hex(v)?,
            "foreground" => th.foreground = hex(v)?,
            "cursor-color" => th.cursor = hex(v)?,
            "selection-background" => th.selection = hex(v)?,
            "palette" => {
                let (i, c) = v.split_once('=')?;
                let i: usize = i.trim().parse().ok()?;
                if i < 16 {
                    th.palette[i] = hex(c)?;
                }
            }
            _ => continue,
        }
        seen = true;
    }
    th.accent = th.palette[4];
    th.light = luminance(th.background) > 0.5;
    seen.then_some(th)
}

/// Built-ins plus every Ghostty theme file in `<data>/themes`.
pub fn all(data_dir: &Path) -> Vec<Theme> {
    let mut v = builtins();
    if let Ok(rd) = std::fs::read_dir(data_dir.join("themes")) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().trim_end_matches(".conf").to_string();
            if let Some(t) = std::fs::read_to_string(e.path()).ok().and_then(|s| parse_ghostty(&name, &s)) {
                v.retain(|x| x.name != t.name);
                v.push(t);
            }
        }
    }
    v
}

/// Settings → Import: copies a Ghostty theme file into the themes folder.
pub fn import(data_dir: &Path, file: &Path) -> anyhow::Result<Theme> {
    let text = std::fs::read_to_string(file)?;
    let name = file.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Imported".into());
    let th = parse_ghostty(&name, &text).ok_or_else(|| anyhow::anyhow!("not a Ghostty theme file"))?;
    std::fs::create_dir_all(data_dir.join("themes"))?;
    std::fs::write(data_dir.join("themes").join(&name), text)?;
    Ok(th)
}

#[cfg(test)]
mod tst {
    #[test]
    fn ghostty() {
        let th = super::parse_ghostty("x", "palette = 1=#ff0000\nbackground = #fafafa\nforeground = 101010\n").unwrap();
        assert_eq!(th.palette[1], 0xff0000);
        assert!(th.light);
        assert_eq!(super::builtins().len(), 15);
    }
}
