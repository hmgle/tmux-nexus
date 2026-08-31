# tmux-nexus

[![ci](https://github.com/hmgle/tmux-nexus/actions/workflows/shellcheck.yml/badge.svg)](https://github.com/hmgle/tmux-nexus/actions/workflows/shellcheck.yml)

A unified tmux search, motion, and workspace manager. Search visible text and
scrollback across panes, jump with on-screen motion hints, and manage sessions,
windows, panes, buffers, processes, commands, and key bindings from fast fzf
pickers.

The `tnx` Rust backend handles capture, indexing, search, preview, actions,
motion, and workspace management. The repository-level `tnx` launcher selects
a local build or installs a checksum-verified release binary when needed.

## Requirements

- `tmux` 3.2+ recommended.
- `fzf` for the interactive picker.
- `curl` or `wget` to download the prebuilt backend (only on first run). No Rust
  toolchain is required to use the plugin; install one only to build from source.
- Optional: `fzf-tmux` for popup rendering, `rg` for user workflows, and one
  clipboard helper (`pbcopy`, `wl-copy`, `xclip`, `xsel`, or `clip.exe`).
- Optional manager integrations: `copyq` for system clipboard history and
  `pstree` for process trees. Without CopyQ, the manager uses tmux buffers.

## Install

With TPM:

```tmux
set -g @plugin 'hmgle/tmux-nexus'
```

The wrapper `tnx` resolves the `tnx` backend in this order:

1. `$TNX_BIN`, if set to an executable.
2. A locally built binary (`target/release/tnx` or `target/debug/tnx`).
3. A current, previously downloaded binary (`bin/tnx`).
4. An optimized `cargo run --release` build when a Rust toolchain is present.
5. Otherwise, a prebuilt release binary is downloaded for your platform via
   `scripts/install-binary.sh` (checksum-verified) into `bin/tnx`.

Local and downloaded binaries are used only when their version matches
`Cargo.toml` and they are newer than the Rust sources. A plain tagged TPM
install therefore needs no toolchain: the first launch fetches the matching
prebuilt binary and caches it. Untagged source checkouts are never paired with a
release binary automatically; build those with Cargo instead. To prefetch a
tagged release explicitly (or in a post-install hook):

```sh
bash ~/.tmux/plugins/tmux-nexus/scripts/install-binary.sh
```

Set `TNX_AUTO_DOWNLOAD=0` to disable the download fallback, or `TNX_BIN=/path/to/tnx`
to use your own binary. For development, build the backend once:

```sh
cargo build --release
```

Manual install:

```tmux
run-shell /path/to/tmux-nexus/tmux_nexus.tmux
```

## Update

When installed with TPM, `prefix + U` or `update_plugins` updates the Git
checkout. The wrapper ignores stale local or cached binaries on the next run,
then rebuilds an optimized release binary with Cargo or downloads the binary
for an exact release tag. To avoid compilation latency on the first shortcut,
build the backend immediately after updating:

```sh
~/.tmux/plugins/tpm/bin/update_plugins tmux-nexus
cargo build --release --manifest-path ~/.tmux/plugins/tmux-nexus/Cargo.toml
```

If you use the prebuilt release binary, refresh the cached binary explicitly:

```sh
~/.tmux/plugins/tpm/bin/update_plugins tmux-nexus
rm -f ~/.tmux/plugins/tmux-nexus/target/release/tnx \
      ~/.tmux/plugins/tmux-nexus/target/debug/tnx
bash ~/.tmux/plugins/tmux-nexus/scripts/install-binary.sh --force
```

Reloading `~/.tmux.conf` is usually not required unless the tmux plugin entry
script or tmux options changed.

## Usage

Use `tnx` as the standalone CLI entry point. From the plugin or source
directory, run it as `./tnx`; from elsewhere, pass the full path. The tmux
binding calls the same launcher internally.

```sh
./tnx                       # interactive, all panes
./tnx error                 # pre-filter to matching records
./tnx --scope session error # current session only
./tnx --scope pane          # current pane only
./tnx --history-lines 5000  # limit scrollback captured per pane
./tnx --action copy token   # copy selected text
./tnx --print panic         # non-interactive print
./tnx --regex 'error|panic' # regex search
./tnx motion s a            # visible-pane 1-char jump
./tnx motion s2 he          # visible-pane 2-char jump
./tnx manage                # full tmux workspace manager
./tnx manage pane switch    # direct pane switcher
./tnx manage clipboard      # CopyQ/tmux buffer history
./tnx doctor                # dependency/config diagnostics
```

Inside fzf:

| Key      | Action                                 |
| -------- | -------------------------------------- |
| `Enter`  | Run the configured default action      |
| `TAB`    | Multi-select results                   |
| `Ctrl-y` | Copy selected result text              |
| `Ctrl-s` | Send selected text to the current pane |
| `Ctrl-p` | Print selected text to stdout          |
| `ESC`    | Cancel                                 |

Motion mode:

| Key                      | Action                                                                                  |
| ------------------------ | --------------------------------------------------------------------------------------- |
| `Prefix+s`               | Prompt for one character, draw hints over visible panes, then jump to the selected hint |
| configured `motion2_key` | Prompt for two characters and jump to the selected matching pair                        |

The two-character binding is disabled by default. Set
`@tmux_nexus_motion2_key` to enable it.

Workspace manager:

| Category | Actions |
| --- | --- |
| `session` | switch, new, rename, detach, kill |
| `window` | switch, link, move, swap, rename, unlink/kill |
| `pane` | switch, break, join, swap, layout, kill, resize |
| `copy-mode` | select and execute copy-mode commands |
| `command` | insert a tmux command into `command-prompt` |
| `keybinding` | inspect and execute a configured key binding |
| `clipboard` | preview and paste CopyQ history or tmux buffers |
| `process` | top, pstree, TERM, KILL, INT, CONT, STOP, QUIT, HUP |
| `menu` | execute user-defined commands, optionally in a popup |

`Prefix+F` opens the manager. Destructive actions require confirmation by
default. TAB selects multiple targets for detach, kill, join, clipboard paste,
and process signals. Object actions use hidden tmux IDs, so custom formats may
contain spaces, colons, quotes, and duplicate display text safely.

## Configuration

Set options in tmux:

```tmux
set -g @tmux_nexus_launch_key "g"
set -g @tmux_nexus_pane_key "/"
set -g @tmux_nexus_default_action "jump"
set -g @tmux_nexus_scope "all"
set -g @tmux_nexus_prompt_query "0"
set -g @tmux_nexus_history_lines "0"
set -g @tmux_nexus_motion_key "s"
set -g @tmux_nexus_motion2_key "S"
set -g @tmux_nexus_motion_hints "asdghklqwertyuiopzxcvbnmfj;"
set -g @tmux_nexus_manager_key "F"
set -g @tmux_nexus_manager_fzf_options "-p -w 62% -h 38% -m"
set -g @tmux_nexus_manager_confirm "1"
```

Or use environment variables:

```sh
TNX_DEFAULT_ACTION=copy ./tnx token
TNX_TMUX_ARGS='-L work' ./tnx --scope session error
```

Supported values:

| Option / env var                                                | Default                       | Values                                                  |
| --------------------------------------------------------------- | ----------------------------- | ------------------------------------------------------- |
| `launch_key` / `TNX_LAUNCH_KEY`                                 | `g`                           | tmux prefix binding                                     |
| `pane_key` / `TNX_PANE_KEY`                                     | empty                         | tmux prefix binding for current-pane search             |
| `scope` / `TNX_SCOPE`                                           | `all`                         | `all`, `session`, `pane`                                |
| `include_history` / `TNX_INCLUDE_HISTORY`                       | `1`                           | `1` or `0`                                              |
| `history_lines` / `TNX_HISTORY_LINES`                           | `0`                           | `0` for all history, or a positive line count           |
| `case` / `TNX_CASE`                                             | `smart`                       | `smart`, `sensitive`, `insensitive`                     |
| `join_wraps` / `TNX_JOIN_WRAPS`                                 | `1`                           | `1` or `0`                                              |
| `skip_blank` / `TNX_SKIP_BLANK`                                 | `1`                           | `1` or `0`                                              |
| `preview` / `TNX_PREVIEW`                                       | `1`                           | `1` or `0`                                              |
| `prompt_query` / `TNX_PROMPT_QUERY`                             | `0`                           | `1` asks for a query before capturing panes             |
| `default_action` / `TNX_DEFAULT_ACTION`                         | `jump`                        | `jump`, `copy`, `send`, `print`                         |
| `fzf_options` / `TNX_FZF_OPTIONS`                               | empty                         | extra fzf arguments                                     |
| `motion_key` / `TNX_MOTION_KEY`                                 | `s`                           | tmux prefix binding for 1-character visible-pane motion |
| `motion2_key` / `TNX_MOTION2_KEY`                               | empty                         | tmux prefix binding for 2-character visible-pane motion |
| `motion_hints` / `TNX_MOTION_HINTS`                             | `asdghklqwertyuiopzxcvbnmfj;` | characters used for motion hints                        |
| `motion_case` / `TNX_MOTION_CASE`                               | `insensitive`                 | `smart`, `sensitive`, `insensitive`                     |
| `motion_smartsign` / `TNX_MOTION_SMARTSIGN`                     | `0`                           | `1` also matches shifted symbols such as `1` -> `!`     |
| `motion_copy_mode_no_prefix` / `TNX_MOTION_COPY_MODE_NO_PREFIX` | `0`                           | bind motion keys directly in copy-mode tables           |
| `motion_vertical_border` / `TNX_MOTION_VERTICAL_BORDER`         | `|`                           | vertical border used by the motion overlay               |
| `motion_horizontal_border` / `TNX_MOTION_HORIZONTAL_BORDER`     | `-`                           | horizontal border used by the motion overlay             |
| `motion_hint1_fg` / `TNX_MOTION_HINT1_FG`                       | `1;31`                        | SGR color for the first hint character                  |
| `motion_hint2_fg` / `TNX_MOTION_HINT2_FG`                       | `1;32`                        | SGR color for the second hint character                 |
| `motion_dim` / `TNX_MOTION_DIM`                                 | `2`                           | SGR color for dimmed pane borders                       |

Manager settings use `@tmux_nexus_manager_*` tmux options or
`TNX_MANAGER_*` environment variables:

| Setting | Default | Purpose |
| --- | --- | --- |
| `key` | `F` | tmux prefix binding; set it to an empty value to disable |
| `order` | `history|copy-mode|session|window|pane|command|keybinding|clipboard|process` | category order and visibility |
| `fzf_options` | `-p -w 62% -h 38%` | fzf-tmux popup/layout options |
| `preview`, `preview_follow` | `1`, `1` | preview visibility and follow mode |
| `confirm` | `1` | confirm destructive actions |
| `switch_current` | `0` | include the current object in switch lists |
| `session_format`, `window_format`, `pane_format` | tmux-aware defaults | custom list formats |
| `pane_remote` | `auto` | append an OSC 7 remote context to pane rows: `auto`, `always`, or `off` |
| `window_filter` | empty | tmux `list-windows -f` filter |
| `menu` | empty | legacy-compatible label/command pairs |
| `menu_popup` | `0` | run menu commands in a popup |
| `menu_popup_width`, `menu_popup_height` | `50%`, `50%` | menu popup size |

For migration, the manager also reads the corresponding `TMUX_FZF_*`
variables when no new setting is present. The precedence is `TNX_MANAGER_*`,
tmux option, legacy `TMUX_FZF_*`, then the default.

### Remote pane directories over SSH

The pane picker can show the working directory reported by the application in
the pane through OSC 7. This is useful for SSH sessions, where
`#{pane_current_path}` remains the local directory in which `ssh` was started.
When `#{pane_path}` contains a `file://host/path` URI whose host differs from
the tmux host, the default `pane_remote=auto` mode appends a compact context such
as `[server:/path-B]`. The context is part of the fzf display row, so it is
searchable. Local OSC 7 paths are hidden in `auto` mode; use
`@tmux_nexus_manager_pane_remote always` to show them or `off` to disable this
automatic context.

The automatic context is appended even when a custom `pane_format` is set. If
that custom format already includes `#{pane_path}`, the same information can
appear twice; set `@tmux_nexus_manager_pane_remote off` when the format manages
remote paths itself.

OSC 7 must be emitted by the shell running inside the pane. Configure it on the
local machine and on remote machines so the value is refreshed after entering
and leaving SSH. The value updates when a prompt is rendered and may otherwise
be stale. In particular, if only the remote shell emits OSC 7, the remote path
can remain visible after SSH exits until the local shell emits its next update.

For bash, the following hook preserves an existing `PROMPT_COMMAND` and URL
encodes the path:

```bash
__tmux_nexus_urlencode() {
    local LC_ALL=C value="$1" out= char i encoded
    for ((i = 0; i < ${#value}; i++)); do
        char=${value:i:1}
        case "$char" in
            [a-zA-Z0-9.~_-]|/) out+=$char ;;
            *) printf -v encoded '%%%02X' "'$char"; out+=$encoded ;;
        esac
    done
    printf '%s' "$out"
}

__tmux_nexus_osc7() {
    local status=$?
    printf '\033]7;file://%s%s\033\\' "$HOSTNAME" \
        "$(__tmux_nexus_urlencode "$PWD")"
    return "$status"
}

if declare -p PROMPT_COMMAND 2>/dev/null | grep -q 'declare -a'; then
    PROMPT_COMMAND+=(__tmux_nexus_osc7)
else
    PROMPT_COMMAND="__tmux_nexus_osc7${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
fi
```

For zsh, install the equivalent hook with `add-zsh-hook`:

```zsh
autoload -Uz add-zsh-hook
__tmux_nexus_urlencode() {
    local LC_ALL=C value="$1" out= char i encoded
    for ((i = 1; i <= ${#value}; i++)); do
        char=${value[i]}
        case "$char" in
            [a-zA-Z0-9.~_-]|/) out+=$char ;;
            *) printf -v encoded '%%%02X' "'$char"; out+=$encoded ;;
        esac
    done
    printf '%s' "$out"
}

__tmux_nexus_osc7() {
    local exit_status=$?
    printf '\e]7;file://%s%s\e\\' "$HOST" \
        "$(__tmux_nexus_urlencode "$PWD")"
    return "$exit_status"
}
add-zsh-hook precmd __tmux_nexus_osc7
```

Fish and several terminal integrations emit OSC 7 on `fish_prompt` already;
enable that integration on both sides of an SSH connection if it is disabled.
The payload should be a percent-encoded `file://host/path` URI.

OSC 7 is application-controlled text. tmux-nexus uses it only for display and
search; it is never passed to a shell or used as a local working directory.
Nested tmux sessions consume OSC 7 unless the inner tmux is configured to
forward it. On tmux 3.3 or newer, opt in with:

```tmux
set -as terminal-features ',*:osc7'
```

Only the active pane's path is forwarded by nested tmux. Other terminal
multiplexers may not forward OSC 7.

Menu entries use the tmux-fzf-compatible format:

```tmux
set -g @tmux_nexus_manager_menu "foo\necho Hello\n\nstatus\ntmux display-message ready\n"
```

Direct bindings do not need compatibility scripts:

```tmux
bind-key f run-shell -b "~/.tmux/plugins/tmux-nexus/tnx manage pane switch"
bind-key y run-shell -b "~/.tmux/plugins/tmux-nexus/tnx manage clipboard"
```

### Migrating from sainnhe/tmux-fzf

Remove `set -g @plugin 'sainnhe/tmux-fzf'`, keep the manager binding at its
default `Prefix+F`, and translate persistent configuration as follows:

```tmux
set -g @tmux_nexus_manager_fzf_options "-p -w 86% -h 58% -m"
set -g @tmux_nexus_manager_pane_format \
  "#{b:pane_current_path} #{=/-26/...:#{d:pane_current_path}} [#{pane_current_command}]"
```

Legacy environment variables remain supported, but the tmux options above are
preferred for new configurations.

CLI flags override configuration for that run.

`pane_key` is disabled by default so the plugin does not replace tmux's existing
binding. When enabled, the plugin resolves `tnx` relative to its
own installation directory, so the binding does not depend on TPM's path.

Motion hints use the configured characters as a prefix-free key set. When a
common match produces more targets than one- or two-character hints can cover,
farther targets use longer hints instead of being dropped.

When `prompt_query` is enabled for the tmux binding, pressing the launch key
opens a tmux prompt first. Empty input cancels without capturing pane history.
This is useful for large tmux servers where opening an unfiltered all-pane
picker would capture and index more scrollback than needed.

## How It Works

1. `capture` lists panes in scope and captures them in parallel, using
   `history_lines` when a scrollback limit is configured.
2. The Rust backend builds a structured temporary index: pane snapshots plus
   record IDs for searchable lines. Joined wrapped text retains the physical
   grid coordinate used by tmux copy mode.
3. Search filters the in-memory index using literal matching by default, or
   regex matching with `--regex`.
4. fzf displays compact rows while preview and actions resolve the selected
   record ID against the same snapshot.
5. Actions call tmux directly to jump, copy, send, or print. Jump actions
   translate the snapshot's absolute history line into tmux copy-mode's
   bottom-relative position, avoiding ambiguity when text is duplicated.

Exact positions reflect the pane geometry captured when the picker opens.
Output appended afterward does not shift those coordinates, but scrollback
truncation or a resize that reflows wrapped lines before selection can invalidate
the snapshot. tmux copy mode scrolls without moving the cursor row, so jumps
scroll relative to the row the pane is already using and then walk the remaining
rows. On tmux versions before 3.7, joined wrapped lines use text-based
positioning because capture wrap flags are unavailable.

Motion mode uses a separate visible-screen path. It lists the panes in the
current tmux window once, starts their independent `capture-pane` commands
concurrently, and preserves pane order while collecting the results. It then
searches the visible text and draws an ANSI hint overlay in a borderless tmux
popup. Selecting a target sends the ordered window, pane, copy-mode, and cursor
operations to tmux as one command sequence instead of starting a tmux process
for every cursor step.

## Development

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
env -u TMUX -u TMUX_PANE cargo test --locked
cargo build --locked
shellcheck -x --source-path=SCRIPTDIR \
  tnx tmux_nexus.tmux scripts/*.sh tests/*.sh
bash tests/launcher.sh
bash tests/install-binary.sh
env -u TMUX -u TMUX_PANE bash tests/shell-quoting.sh
env -u TMUX -u TMUX_PANE bash tests/tmux-integration.sh
env -u TMUX -u TMUX_PANE bash tests/manage-integration.sh
```

The tmux integration tests require `tmux`; picker workflows and CI also require
`fzf`. Every test server uses its own explicit tmux socket.

Use `./tnx doctor` to verify local dependencies and resolved
configuration.

## Inspired by

- [tmux-fzf](https://github.com/sainnhe/tmux-fzf)
- [tmux-easymotion](https://github.com/ddzero2c/tmux-easymotion)

## License

MIT. See [LICENSE](LICENSE).
