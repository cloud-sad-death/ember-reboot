# Ember

A native, GPU-rendered terminal for macOS that records everything you run, cleanly sliced, and makes it instantly searchable. Its main view is a native rebuild of the blast30/stream2 report.

- **Rust end to end.** GPUI (Zed's GPU UI framework) draws the UI. Capture, storage, search and export live in `ember-core`.
- **Your own zsh, untouched.** Ember points `ZDOTDIR` at itself only long enough to load your real `.zshenv`, then puts it back. Your `.zprofile`, `.zshrc`, prompt, completions, aliases and functions load exactly as in any terminal. Its hooks are added on the first prompt.
- **Private in-band channel.** zsh hooks write invisible `OSC 7777` markers into the PTY stream, so they arrive in exact byte order with the output. Ember strips them before anything is drawn.

## Workspace

| Crate | What it is |
|---|---|
| `crates/ember-core` | PTY ownership, marker filter, recorder, loop slicer, streaming slicer, storage, search, keyword index, export, settings, themes, snippets |
| `crates/ember-app` | The GPUI app (`ember`): windows, tabs, splits, terminal panes, report view, search view, snippets, settings, ⌘K |
| `crates/ember-cli` | Headless recorder and tools (`ember-cli record`, `sessions`, `rows`, `show`, `search`, `export`, `index`, `recompress`, `prune`, `bench`) |

## Recording

- **Standard mode** gives one row per command, with date, time, folder, exit code and output. Loops get one row per target. The item boundary comes from the first of these sources to fire, per command:
  1. A zsh `DEBUG` trap. For each command a loop runs, it sends the command with its simple parameters expanded; command substitutions are never evaluated. A match on the loop verb starts a new row, so `for f in …; do cat $f; done` produces `cat /x/a`, `cat /x/b`, and so on.
  2. `find … -exec VERB {} \;` gets an item marker before each `-exec`, only while a recorded loop writes to the terminal. The row header is `VERB <path>`.
  3. blast30's own rule: a bare absolute path alone on its line, at bracket depth 0, starts an item. Depth tracking stops nested paths inside plist dumps from splitting items.
  - Loops hidden inside your functions are found the same way, through function resolution.
- **Streaming mode** gives one command header with timestamped time slices under it. Slices are cut every N seconds (5 by default), or at each full screen refresh for tools like `top`. For redrawing tools, the readable screen text is stored next to the raw bytes.
- **Storage**:
  - Each session is append-only zstd frame logs, `raw` for exact replay and `text` for search, plus a JSONL catalog and line marks.
  - Compression runs on worker threads, so recording never waits on disk.
  - Nothing rotates. A single row can be any size, and only the visible lines are decoded.
  - Data lives in `~/Library/Application Support/Ember` (configurable).

## Build

```bash
cargo test -p ember-core -- --test-threads=1   # includes real zsh-in-a-PTY tests
cargo build --release -p ember-app -p ember-cli
./scripts/bundle-macos.sh                       # dist/Ember.app + dist/Ember.dmg (unsigned)
```

The first launch of the unsigned app needs right-click → Open. GitHub Actions (`.github/workflows/macos.yml`) builds the `.dmg` on macOS, runs the tests and a 4 GB benchmark, and uploads screenshots.

## Settings

All settings are in one TOML file, `~/Library/Application Support/Ember/ember.toml`, and every one of them is also in the GUI. Ghostty theme files can be dropped into `…/Ember/themes/` or added with Settings → Import.
