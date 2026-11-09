# Ember — Plan (v4)

Ember is a native, GPU-rendered terminal for macOS that the user lives in all day. It runs their own zsh untouched and turns the chaos of the terminal into cleanly organized, instantly searchable data.
Its main view is a native, non-HTML rebuild of the blast30/stream2 report design, built to move through gigabytes of output in a second.

## Who it's for
- The owner, as a daily driver on macOS, recording constantly. No accounts.
- It may become public later, so cross-platform stays possible.

## Architecture decision
| | A. Rust + Tauri (previous build) | B. Swift + libghostty (cmux, Muxy, Zentty) | C. Rust + GPUI + libghostty (Con) |
|---|---|---|---|
| How the terminal is drawn | Inside a browser view. Bytes cross a bridge into JavaScript and are parsed twice | The Ghostty engine draws directly with Metal | The Ghostty engine draws directly; the UI is drawn on the GPU by Zed's framework |
| Handling GB-scale output | Poor | Excellent | Excellent |
| Language for capture, storage and search | Rust | Swift, plus a Rust core | Rust end to end |
| Cross-platform later | Yes | No | Yes (Con ships macOS, with Windows and Linux in beta) |
| Buildable and testable in this workspace | Partly | Barely | Mostly |

- **Decision: C.** It keeps Rust, drops the web view entirely, and has a working open-source reference (Con, MIT). B is the fallback if GPUI blocks something critical. The previous build's slowness came from the web view, not from Rust.

## Core features and experience

### 1. Terminal
- Multiple windows, tabs, and split panes (side by side and stacked, resizable, keyboard-driven).
- The user's real interactive zsh: their own prompt, colours, completions, history, aliases and functions. No Ember prompt.
- Window transparency and blur, from opaque to nearly invisible.

### 2. Recording
- **Two modes, toggled per pane**: Standard and Streaming. A recording light in the pane is styled differently for each mode; off means nothing is captured.
- **Command boundaries: xtrace + PTY.**
  - Ember owns each pane's PTY. Every byte goes to the screen and, in parallel, to a recorder that never blocks typing.
  - xtrace reports what ran on a private channel that never appears on screen.
  - Light markers are used only where xtrace can't tell where something ends.
- **Standard mode**: the blast30 structure, kept in full.
  - One row per command, with exact date, time, folder, exit code, and full output beneath it.
  - **Loop slicing is a core feature, not an extra.** Loops, including loops hidden inside the user's own shell functions and aliases, are split into one row per target, each labelled with the resolved verb and target. For example, `find /target -name '*.query' -exec cat {} \;` becomes a run of rows: `cat /target/a.query`, `cat /target/b.query`, and so on. Each has its own timestamp and its own output directly beneath it.
  - The script's rules carry over exactly: function and alias resolution, `-exec` and `do`-body verb extraction, and a bare absolute path at bracket depth 0 starting a new item, with depth tracking so nested paths inside plist dumps don't split items falsely. Real loops will be used as test cases, because this is known to be hard.
- **Streaming mode**: the stream2 concept, slightly looser, built for long-running and repeating commands like `top`, `nettop`, `tcpdump` and `log stream`.
  - The command's provenance is simple: one command header, such as `nettop -P`, started at a given time.
  - Beneath it, the output is cut into **time slices**. Each slice is a sub-row with its own timestamp, by default every few seconds (adjustable) or at each full screen refresh for screen-redrawing tools like `top`.
  - Any moment can be traced back to the command, and the output from that moment found right there.
  - Everything is saved. For screen-redrawing tools, Ember stores the raw stream and also the readable text of each screen, so `top` output is searchable as plain text rather than as cursor codes.
  - Line-based streams like `tcpdump` are saved line by line under their slices.

### 3. Report view: the template, rebuilt natively (the primary view)
A one-to-one native rebuild of the blast30/stream2 report design. Same structure and the same ergonomics, but rendered by Ember instead of a browser and never limited by file size.
- **Sticky index sidebar**: every command and every loop item or stream slice, grouped like the template's index. Resizable with a drag handle, and the font size is adjustable. It gives a bird's-eye view of the whole session; clicking any entry jumps straight to that row.
- **Fast-scroll rail with thumb**: the template's custom scroll rail. Dragging the thumb moves through a multi-GB session instantly, landing on command boundaries (input, then output).
- **Header strip**: host pills (macOS version and build, model, memory and chip, host, firmware, zsh version) plus the date and time.
- **Per-row controls, exactly as designed**:
  - delete from report, with a confirm dialog
  - copy header
  - copy output
  - shift-click to copy header + output
  - fold or unfold
  - collapse all or expand all
  - save as snippet
- **Row anatomy**: date and time strip, then the command or target header, then the output in monospace. A non-zero exit code is marked. Colours come from the active theme.
- **Config row**: the template's report-settings row at the end of each session.
- **Parts become a pager over the data**: the template's parts lane stays as a navigation aid (jump to segment N of a huge session). It is not a file split, because nothing internal rotates.
- **Live**: rows that are still recording, or still streaming, grow in place while open.
- **Report settings** in the GUI: every value from the template's REPORT CONFIG block (output, row, timestamp and index font sizes; line height; maximum indent; part columns and button size; scroll duration; collapse-on-open).

### 4. Search: ultra-fast, across everything
- **A search bar in the report view** searches the current session or everything. Results jump to the exact row and line, with hits highlighted.
- **A separate Search view** lists every hit across all sessions with command, time and host, and opens each in the report view in place.
- **What can be searched**:
  - plain text and regex, ripgrep-style, scanning compressed history in parallel across all cores
  - a background full-text index for instant keyword hits
  - filters by date, host, folder, mode, command and exit status
- **Indexing applies to both modes, so every row and slice is searchable.** The only adjustable limit is how much of very high-volume streams goes into the instant keyword index. Ripgrep-style search still covers 100% of the saved output either way. Default: everything is indexed.

### 5. Storage at scale
- **Format**: each session is an append-only log of compressed blocks (zstd, seekable), plus a catalog of every row and slice: times, command, resolved target, folder, host, exit code and position.
- **Exact replay**: raw bytes are kept, so colours replay exactly.
- **No internal rotation**: a single row can be 50 GB. The view decodes only what is on screen.
- **Compression**: fast live compression, with tighter recompression in the background. Typical terminal text shrinks 5–10×.
- **Location**: Ember's own data folder (`~/Library/Application Support/Ember`). It can be moved, for example to an external disk. Nothing goes to `~/fx`.
- **Retention**: size and age limits, pinned sessions that are never pruned, and archiving.

### 6. Export
- **HTML** in the blast30/stream2 format, the same file opened in a browser today. Part rotation happens only here, sized for browsers (default 25 MB, adjustable).
- **JSON** alongside, plus plain or ANSI text.
- **Scope**: a single row, a session, a search result, or a date range.

### 7. Snippets
- Save, tag, search, copy and insert into the active pane.
- Create them from any report row or directly. Available from the ⌘K palette.

### 8. Themes and settings
- **Built-in themes**: about 15, mostly light — Catppuccin, Nord, Rosé Pine Dawn, Solarized Light, and pastel/neon "ethereal" sets.
- **Adding themes**: drop in Ghostty-format theme files, or use Settings → Import.
- **A GUI settings pane covers everything**:
  - colours and theme
  - opacity and blur
  - fonts
  - padding
  - cursor style
  - pane borders
  - recording-light style
  - report-view settings
  - streaming slice interval
  - storage, retention and indexing
  - export part size
  - keybindings
- Every setting is also stored in one readable config file.

## User flow
1. Open Ember. The user's normal zsh prompt appears, transparent and themed.
2. Split panes and toggle Standard or Streaming per pane. The light comes on.
3. Run a loop over thousands of targets. Each target lands as its own labelled row.
4. Run `nettop`. It lands as one header with timestamped slices beneath it.
5. Open the report view: the sticky index shows everything, the fast-scroll rail moves through gigabytes, and rows can be copied, folded, deleted or saved.
6. Search across all history, jump to the exact row and line, and export to HTML/JSON when needed.

## UI/UX feel
- Minimal chrome. The terminal fills the window, and heavy transparency is normal.
- Light-first pastel palettes by default.
- The report view keeps the template's monoblock design, dense and calm, rendered natively and instantly.

## Implementation phases
- **Phase 1 (MVP, built now)**:
  - Terminal: windows, tabs and splits, the user's own zsh, transparency and blur.
  - Look: built-in and imported themes, and the full settings pane.
  - Recording: Standard (with loop slicing) and Streaming (with time slices), plus the recording light.
  - Report view: the native template rebuild (sticky index, fast-scroll rail, row controls, pager, live rows).
  - Search: the search bar and Search view.
  - The storage engine.
  - HTML/JSON export, snippets and ⌘K.
  - The GitHub Actions `.dmg` build.
- **Phase 2**:
  - tags, pins, linked rows, saved searches
  - auto-record rules per folder or command
  - more loop patterns
  - recompression tiers and retention tools
  - session restore
  - Markdown and combined exports
- **Phase 3**:
  - Linux and Windows builds
  - auto-update
  - signing and notarisation for public release

## Testing and verification
- **This workspace (Linux)**: the recorder, loop slicing, streaming slices, storage, search and export are built and tested for real. Tests use an actual zsh in a PTY, the user's own loop patterns, and multi-GB synthetic streams for speed and memory benchmarks. The UI is shown on a virtual display for screenshots where GPUI allows.
- **GitHub Actions on macOS**: builds the `.dmg` and runs the same tests on a real Mac. It publishes screenshots and benchmark results that can be reviewed here.
- **The user's Mac**: the final check on feel, transparency, multi-window use and real workloads.
- **Phase 1 targets**:
  - recording adds no visible lag
  - sustained recording of at least 200 MB/s
  - a 5 GB session opens and fast-scrolls in under 1 second
  - keyword search over 50 GB of history in seconds

## Assumptions
- **Branch C (Rust + GPUI + libghostty)** replaces Tauri. Swift + libghostty is the fallback.
- **Con-terminal as reference**: being MIT-licensed, it may be used as a reference for embedding libghostty in GPUI. Ember's code is its own.
- **GitHub repo and token**:
  - The user links GitHub through the platform's GitHub connection so code can be pushed to their repo. The repo is new or reset; the old `ember` repo is not reused.
  - Reading CI results needs a personal access token, collected later through the secure secrets card.
- **The `.dmg` is unsigned at first**: the first launch needs right-click → Open. Signing needs an Apple Developer account ($99/yr) and is deferred to Phase 3.
- **No Python dependency**: the script's report engine and copier are rebuilt inside the app.
- **The report view is the primary view; HTML is export-only.** A separate Search view is included.
- **Streaming time slices default to every 5 seconds**, or each screen refresh for screen-redrawing tools. The interval is adjustable.
- **Recording is off by default** in new panes. It can be made the default in Settings.
- **The template's commented-out features stay off**: the description field and the accent selector.
- **zsh only** in Phase 1.
