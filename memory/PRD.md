# Ember — PRD / status

## Original problem statement
Ember: a native, GPU-rendered macOS terminal (daily driver) that runs the user's own zsh untouched and turns terminal output into organised, instantly searchable data. Its primary view is a native rebuild of the blast30/stream2 report template (assets in /app/reference). Plan v4 was approved: Rust + GPUI (+ libghostty as the intended terminal engine), Standard mode with loop slicing, Streaming mode with time slices, native report view, search, storage at scale, HTML/JSON export, snippets, themes and settings, and a GitHub Actions .dmg build.

## Architecture (as built, /app/ember)
- `crates/ember-core` (Rust): PTY ownership (portable-pty); in-band OSC 7777 marker filter; zsh integration (ZDOTDIR passthrough, preexec/precmd hooks, a DEBUG-trap item channel for loops, a find -exec item marker); LoopSlicer (blast30 rules); StreamSlicer (time slices and screen-refresh slices, with vt100 screen text); storage (zstd frame logs for raw and text, compressed on worker threads, a JSONL catalog, line marks); parallel search (regex/plain) and a keyword index; HTML/JSON export in the template format (with part rotation) and text/ANSI export; settings (TOML); 15 themes plus a Ghostty theme parser; snippets; retention; recompression.
- `crates/ember-app` (GPUI 0.2.2): windows, tabs, resizable splits, terminal panes (alacritty_terminal grid painted by GPUI), a recording light per pane, the report view (virtualised per line, sticky index, fast-scroll rail, row controls, pager, live), the Search view, Settings, Snippets, and the ⌘K palette.
- `crates/ember-cli`: record, sessions, rows, show, search, export, index, recompress, prune, bench.
- `.github/workflows/macos.yml` and `scripts/bundle-macos.sh` build an unsigned .dmg on macOS CI.

## Implemented (2026-06)
- Core engine. All tests pass, including real zsh in a PTY with loops hidden in the user's own functions.
- Benchmark here: 227 MB/s recording, 14.7× compression, 1 ms random seek, about 1.1 GB/s scan search.
- The GUI compiles on Linux and runs headless end to end: panes, splits and recording were verified by the sessions it created. Rendering could not be screenshotted on the Linux virtual display.

## Deviations / not yet verified
- The terminal engine is alacritty_terminal, not libghostty (libghostty embedding is macOS/Xcode-only and cannot be built here). It sits behind TerminalView, so it can be swapped.
- UI look and feel is unverified until macOS CI runs (needs GitHub connected) or the user tries it on their Mac.

## CI fix (2026-06)
- Root cause of empty GitHub Actions tab: workflow was at `ember/.github/workflows/macos.yml` but repo root is `/app`. GitHub only discovers workflows at `<root>/.github/workflows/`.
- Fix: moved workflow to `/app/.github/workflows/macos.yml` with `defaults.run.working-directory: ember`, `Swatinem/rust-cache` `workspaces: ember`, and artifact paths `ember/dist/Ember.dmg`, `ember/bench.txt`, `ember/shot-*.png`. Nested `ember/.github` removed.
- Target: Apple Silicon only (`macos-15` runner, arm64; `macos-14` was retired by GitHub on 2026-11-02). Local validation: core tests pass, ember-cli builds, `bench` runs. ember-app (GPUI/Metal) compile only verifiable on the Mac runner.
- Note: `ember/Cargo.lock` is untracked; committing it improves build reproducibility.
- macOS CI test fix: `pty_zsh.rs` cwd assertion now canonicalizes `home.path()` (macOS resolves tempdirs through `/var`→`/private/var`; was comparing raw vs resolved). No-op on Linux. Also `runs-on: macos-15`, `checkout@v5`.

## Backlog
- P0: push to GitHub → CI .dmg + screenshots; fix macOS compile/visual issues.
- P0: swap in libghostty (or keep alacritty if it is fast enough).
- P1: Phase 2 items from the plan (tags, pins UI, linked rows, saved searches, auto-record rules, session restore, Markdown export).
- P2: Linux/Windows builds, auto-update, signing.
