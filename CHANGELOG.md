# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed
- Motion hint windows are prepared before display and cleaned up when their
  pane exits, the client detaches, or the user changes windows. Cleanup no
  longer depends on shell completion signals, steals focus, or renumbers
  existing windows. Startup failures are reported to the originating client.
- Motion correctly falls back to popups when older tmux versions return an
  empty, successful response to the floating-pane capability check.
- Motion hints no longer shift zoomed pane content when opening or closing on
  tmux versions with floating panes. These versions use a temporary hint window
  that leaves the original pane dimensions intact and is removed on exit.
- Motion hints now refresh their pane content after the popup opens, avoiding
  stale jump coordinates when tmux temporarily unzooms a pane and the shell
  redraws cleared-screen output such as `git remote -v`.
- Motion jumps no longer move above matches when the visible viewport starts
  in the middle of a wrapped line.
- Motion jumps now preserve the target row when leading empty or space/tab-only
  rows cause tmux to snap to the end of a later line, regardless of tmux tab
  handling mode and including lines containing tabs or wide characters.
- Motion jumps now skip tab-indented rows as anchors, keeping the exact target
  row and column when the first visible content row is a tab-indented wrapped
  continuation.

## [0.8.1] - 2026-08-31

### Fixed
- The zsh OSC 7 hook now preserves the previous command status without
  assigning zsh's read-only `status` parameter.
- Installer archives now reject symlinks, hard links, and other non-regular
  members and enforce the fixed release member list before extraction.
- Jump actions now retain physical grid coordinates when wrapped text is joined,
  activate the target pane's window, include the bottom grid line, and handle
  session names containing `:` without target parsing ambiguity. tmux versions
  without capture wrap flags fall back to text positioning for joined lines.
- Jump actions no longer assume the target pane's cursor is on its last row.
  tmux copy mode only scrolls for `goto-line`, so panes left with the cursor
  higher up landed short by the difference.
- Joining wrapped lines now keeps the spaces at a wrap boundary, which flag
  captures previously trimmed before the segments were joined.
- `TNX_LAUNCH_KEY`, `TNX_PANE_KEY`, `TNX_MOTION_KEY`, and `TNX_MOTION2_KEY` now
  take precedence over their corresponding tmux options when the plugin creates
  bindings.
- Process signal fallbacks and privilege helpers now perform one final UID and
  process-start check immediately before delivery. Linux pidfds verify identity
  after opening to cover reuse before the pidfd became stable.
- Search filtering now reuses its combined-field buffer across records,
  avoiding a temporary allocation for every regex or separator-aware match.
- Documentation now matches the ASCII motion-overlay border defaults used by
  the implementation.

## [0.8.0] - 2026-08-30

### Added
- Manager pane rows can show and search remote working-directory context from
  OSC 7 (`pane_path`), with automatic host comparison, URI path decoding, and
  bounded display width. Configure `pane_remote` as `auto`, `always`, or `off`.
- Documented shell hooks and nested tmux forwarding for OSC 7 SSH directories.
- Host comparison distinguishes same-named hosts in different domains, and
  malformed percent escapes remain visible while valid URI path components are
  decoded.

## [0.7.2] - 2026-07-12

### Changed
- When no current backend binary exists, the `tnx` wrapper now builds with
  Cargo's optimized release profile instead of leaving a debug binary for
  normal tmux use.

### Tests
- Added launcher coverage that verifies the automatic Cargo fallback selects
  the release profile.

## [0.7.1] - 2026-07-12

### Changed
- Manager workflows cache fzf/tmux capability probes and preserve typed
  workspace listings, reducing repeated subprocess calls while keeping actions
  tied to the objects shown by the picker.
- Explicit `tnx manage history` launches skip manager-only picker setup, and
  multi-buffer pastes are sent through one ordered tmux invocation.
- Manager clipboard, process, and workspace adapters are split into focused
  modules to keep their system interactions isolated.

### Fixed
- Process signals revalidate numeric UID and process start identity before
  delivery. Linux uses process start ticks and pidfds to close PID-reuse races;
  other platforms retain the portable fallback.
- Manager picker cancellation remains a normal no-op while unexpected fzf
  exits are reported as failures.
- Large process selections are revalidated in bounded batches instead of
  risking platform command-line limits.

### Tests
- Added integration coverage for CopyQ's basic `count`/`read` compatibility
  path and for manager cancellation versus picker failure.

## [0.7.0] - 2026-07-12

### Added
- Renamed the project to `tmux-nexus`, the CLI and release binary to `tnx`,
  configuration variables to `TNX_*`, and tmux options to `@tmux_nexus_*` to
  reflect the expanded search, motion, and workspace-management scope.
- Added a `Prefix+F` workspace manager covering tmux-fzf session, window,
  pane, copy-mode, command, keybinding, clipboard, process, preview, multi-select,
  and user-menu workflows.
- Added `tnx manage [category] [action]` for direct bindings and
  `@tmux_nexus_manager_*` / `TNX_MANAGER_*` configuration.
- Added compatibility fallbacks for the corresponding `TMUX_FZF_*` variables.
- Added an empty manager key setting to disable the default `Prefix+F` binding.
- Added CopyQ clipboard history, tmux-buffer fallback, process signals, popup
  process inspection, and default confirmation for destructive actions.

### Changed
- Manager selections use hidden tmux object IDs rather than parsing formatted
  display text, making custom formats and special characters safe.

### Tests
- Added isolated manager integration coverage for pane switching, copy mode,
  buffers, key bindings, user menus, and process signalling.

### Changed
- Motion capture now starts visible-pane `capture-pane` processes concurrently
  and retains their original pane ordering when collecting output.
- Motion navigation now batches window selection, pane selection, copy-mode,
  and cursor movement into one ordered tmux command sequence, reducing process
  startup overhead for every jump.

### Tests
- Motion integration coverage now verifies that concurrent captures remain
  associated with the correct pane.

## [0.6.2] - 2026-07-11

### Fixed
- Leading-hyphen jump searches now remain compatible with tmux versions that
  pass copy-mode command arguments differently.

## [0.6.1] - 2026-07-11

### Fixed
- Jump searches now pass leading-hyphen pane text through to tmux literally,
  so results such as long-format file listings land on the selected line.

## [0.6.0] - 2026-07-11

### Added
- `@tmux_nexus_pane_key` can bind a prefix key to current-pane search
  without hard-coding the plugin installation path.

## [0.5.0] - 2026-07-10

### Added
- Visible-pane motion mode can now jump to matching text in the current tmux
  window with easymotion-style hints. `Prefix+s` starts one-character motion,
  and `@tmux_nexus_motion2_key` can enable two-character motion.
- Isolated tmux integration coverage now exercises visible-only duplicate
  jumps, limited history, wrapped lines, disappearing panes, and cross-pane
  motion navigation.

### Changed
- Motion overlays now render in a borderless tmux popup instead of a temporary
  tmux window.
- Search records now store compact pane and line coordinates instead of cloned
  text and metadata. Preview snapshots are split per pane and legacy capture is
  streamed, substantially reducing peak memory on large histories.
- Motion popup startup reuses its initial visible-pane snapshot, and overlay
  rendering batches terminal writes instead of repeating capture and flush
  work.
- Cached backends are accepted only when their version matches the source and
  they are newer than Rust inputs. Untagged source checkouts no longer download
  an unrelated release binary automatically.
- Motion internals are split into capture, matching, hint, rendering, terminal,
  and navigation modules.

### Fixed
- Motion hints can grow beyond two characters, so common matches no longer
  silently drop distant targets.
- Motion overlays restore tab cells as expanded screen spaces after hint
  filtering.
- Visible-only and limited-history jumps now preserve the absolute scrollback
  offset, including when the same text also appears earlier in history.
- Literal case-insensitive search now handles Unicode, while fzf cancellation,
  invocation errors, and prefilter queries retain their distinct semantics.
- Invalid configuration, malformed shell options, pane capture failures, jump
  failures, and tmux-buffer failures now return actionable errors instead of
  silently falling back or dropping data.
- Deferred tmux commands now quote paths and values safely, and fzf display
  fields are sanitized before rendering.
- Motion raw mode now restores terminal state on Ctrl-C and error paths, drains
  trailing escape bytes, and avoids drawing or restoring cells across pane and
  wide-character boundaries.

### Security
- Prebuilt installation now requires a valid checksum, rejects unsafe archive
  paths, serializes concurrent installs, and replaces the cached binary
  atomically only after every download and verification step succeeds.

## [0.4.1] - 2026-06-22

### Changed
- Loading configuration now reads all `@tmux_nexus_*` tmux options in
  one call and reuses the result, avoiding repeated `show-option` calls after
  config import.
- Legacy capture output is now streamed directly from captured pane text instead
  of building a full structured search index first.
- Literal search now checks record fields directly and avoids per-record
  searchable-text and lowercase string allocations for common queries.

### Fixed
- Literal and regex searches continue to match across location, command, window,
  and text fields when queries include explicit tab separators.
- Legacy TSV capture preserves raw capture-pane line numbers when blank lines
  are skipped, so limited-history output reports the original tmux line numbers.

## [0.4.0] - 2026-06-22

### Added
- `history_lines` / `TNX_HISTORY_LINES` and `--history-lines` can now limit how
  much scrollback is captured from each pane.
- `prompt_query` / `TNX_PROMPT_QUERY` can make the tmux binding ask for a query
  before capturing pane history; empty input cancels without indexing panes.

### Changed
- Record lookup now uses direct record IDs, avoiding quadratic candidate
  generation when opening the picker without an initial query.

### Fixed
- Documented `bash ./tnx` as the standalone CLI entry point
  instead of the non-existent `history-finder` command.
- `history_lines` now preserves the omitted scrollback offset so jump actions
  still target the selected line after a limited capture.
- Prompted tmux searches now store the query under a client-specific temporary
  option, avoiding accidental reuse across concurrent prompts.

## [0.3.1] - 2026-06-22

### Fixed
- `Ctrl-y` and other fzf expect-key exits no longer fail with `BrokenPipe` when
  fzf closes before all candidates have been written.
- Copy now tries every available system clipboard helper before falling back to
  the tmux buffer, so an unusable helper such as `wl-copy` no longer blocks a
  working fallback such as `xclip`.
- The tmux-buffer fallback message now distinguishes between missing clipboard
  helpers and helpers that were present but failed.
- The release workflow now builds `x86_64-apple-darwin` on GitHub's supported
  `macos-15-intel` runner instead of the retired `macos-13` image.
- Release notes are now extracted from this changelog and published once per
  release, avoiding duplicated generated `Full Changelog` lines.

## [0.3.0] - 2026-06-21

### Changed
- Reworked the implementation around a Rust `tnx` backend. The tmux plugin file
  and legacy shell paths remain as compatibility wrappers, but capture, search,
  preview, and actions now run through structured Rust code.
- Capture now runs panes in parallel and stores one structured pane snapshot per
  search. fzf preview and result actions resolve selected record IDs against the
  snapshot instead of repeatedly re-capturing full scrollback.
- Search now supports built-in literal matching by default and `--regex` for
  regular expressions, with smart/sensitive/insensitive case handling.

### Added
- `tnx doctor` / `bash ./tnx doctor` diagnostics for tmux, fzf,
  fzf-tmux, ripgrep, clipboard support, and resolved configuration.
- fzf action shortcuts: `Ctrl-y` copy, `Ctrl-s` send, and `Ctrl-p` print, while
  `Enter` continues to use the configured default action.
- Rust CI coverage for formatting, unit tests, and build verification.
- Prebuilt `tnx` binaries published to GitHub Releases (Linux/macOS, x86_64 and
  aarch64) via a tag-triggered `release` workflow.
- `scripts/install-binary.sh` downloads and checksum-verifies the prebuilt binary
  for the current platform into `bin/tnx`. `tnx` calls it
  automatically when no Rust toolchain is available, so TPM installs work without
  `cargo`. Opt out with `TNX_AUTO_DOWNLOAD=0`, or point `TNX_BIN` at your own
  binary.

## [0.2.0] - 2026-06-20

### Fixed
- `@tmux_nexus_*` options are now honoured by the key binding (and the
  CLI). They were previously exported from the transient plugin-load shell and
  lost before the picker ran, so only `launch_key` ever took effect. All options
  are now resolved at run time in `scripts/utils.sh`.
- `--case sensitive` with the ripgrep backend no longer mis-parses its flags. It
  used to emit a stray `-- ` that made `rg` treat the query as a filename and
  match the literal `--` instead, also corrupting the TAB record format.
- The `preview` option (`@tmux_nexus_preview` / `TNX_PREVIEW`) is now
  respected; the preview was previously always shown.
- `--print` with a query now runs fully non-interactively (no picker), matching
  its documented "scriptable, no UI" behaviour; it previously still launched fzf.
- The first-load notice now actually fires once. Its guard tested the exit
  status of `show-option -gqv`, which always succeeds, so the message (and the
  `@tnx_loaded` marker) never appeared; it now tests the option's value.

### Changed
- The fzf preview caches each pane's capture for the picker's lifetime instead
  of re-capturing the full scrollback on every keystroke, and matches the
  index's wrap-join setting so the displayed line numbers line up.
- Relaxed the documented requirement from bash 4+ to bash 3.2+ (no bash-4-only
  features are used; works with the macOS system bash).

### Added
- `bash ./tnx --version` / `bash ./tnx -V`.

### Removed
- Unused internal helpers (`tnx_tmux_socket_args`, `tnx_fzf_bin`,
  `tnx_tmux_quote`) and a dead smart-case placeholder.

## [0.1.0] - 2026-06-20

### Added
- Initial release.
- Search the visible content and full scrollback history of every tmux pane
  (across all sessions), with `all` / `session` / `pane` scope control.
- Interactive fuzzy finder (fzf) with a live source-pane preview that highlights
  the matched line.
- Four result actions: `jump` (switch to pane + copy-mode search to the match),
  `copy` (tmux buffer + system clipboard), `send` (type into current pane), and
  `print` (to stdout).
- Pre-filtering backend selection (`auto` / `rg` / `grep`) with smart-case
  support, so large histories stay responsive.
- TPM plugin entry point with a configurable launch key, plus a standalone
  `tnx` CLI wrapper.
- Full configuration via either tmux `@tmux_nexus_*` options or `TNX_*`
  environment variables.

### Notes
- The `jump` action locates the match by text via copy-mode `search-forward`
  (with regex metacharacters escaped) rather than by line number, so it is
  robust to blank-line skipping and line-wrap joining in the index.
