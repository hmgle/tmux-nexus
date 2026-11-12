# Manager popup startup

The pane switcher follows this path:

```text
prefix + f -> tmux run-shell -b -> tnx launcher -> Rust manager
           -> read configuration and pane snapshot -> fzf --tmux
           -> native tmux popup -> selected pane
```

The manager uses native fzf popups (fzf 0.60.3+ and tmux 3.3+). It does not run
the `fzf-tmux` Bash wrapper or probe program versions on each invocation.
Existing `-p -w 86% -h 58%` settings are translated to
`--tmux=center,86%,58%` inside the manager. New configurations can use the native
option directly.

fzf 0.60.3 is the supported and CI-tested release baseline. Older native-popup
implementations can fail with `bad variable name` when the environment contains
exported Bash functions (`BASH_FUNC_*%%`). Upstream fixed function export in
[4161403](https://github.com/junegunn/fzf/commit/4161403a1d6286f6ba7898b1f22f30d01d85b8dc).
The terminal regression includes such a function. The manager retains its direct
native path without runtime version probes or compatibility wrappers.

Configuration, including an explicitly empty manager key, is read in one tmux
query. Pane enumeration and the current-pane identifiers share a second tmux
client invocation. The list still uses live pane information and the configured
display format; it does not cache pane names, paths, or commands.

The launcher parses manifest and binary versions with Bash builtins. An explicit
`TNX_BIN` override is executed before inspecting the checkout. Normal launches
still check the binary version and source freshness before selecting a backend.

For pane previews, each fzf row carries a hidden pane ID. The preview invokes
`tmux capture-pane` directly against that ID using the configured tmux socket.
It does not write a JSON snapshot or start another tnx process. Other manager
preview categories retain their snapshot implementation.

The list now occupies approximately 50% of the popup width, with a wrapping
preview on the right taking 50%. Previously, the list had 40% and the preview
60%. The overall popup dimensions stay controlled by `--tmux`:

```text
Before: [ list 40% ][            preview 60%            ]
After:  [       list 50%       ][      preview 50%      ]
```

Set `@tmux_nexus_manager_preview_width` to change the right preview's percentage
on the next invocation, without rebuilding. For example, `40%` gives the list
60% and the preview 40%. `TNX_MANAGER_PREVIEW_WIDTH` overrides the tmux option.
Both accept integers from 1 to 99 with an optional `%` suffix; the default is
`50%`.

## Terminal verification and measurements

Run the attached-client regression with an isolated tmux server:

```sh
cargo build --release --locked
TNX_BIN="$PWD/target/release/tnx" TNX_TEST_SAMPLES=20 TNX_TEST_PANES=143 \
  uv run --no-project python tests/manage-popup-integration.py
```

The test sends the actual prefix binding through a PTY and measures until the
`pane>` prompt appears in terminal output, polling at 5 ms intervals. It checks
preview content, cancellation, and switching to the previewed pane. A failing
`fzf-tmux` stub verifies that the manager does not use the wrapper. The socket
path contains both spaces and a quote to exercise preview command quoting.

Local Linux measurements on 2026-11-12, with tmux next-3.9, fzf 0.60 (devel),
143 panes, a 160x44 terminal, and the same 86%x58% popup and custom pane format.

These startup measurements preceded the layout adjustment and used a 60%-wide
preview on both versions.

| Keypress to prompt | Before (0748ddb) | After |
| --- | ---: | ---: |
| First invocation in the test server | 258.2 ms | 112.3 ms |
| Median of 20 invocations | 140.6 ms | 90.8 ms |
| 95th percentile of 20 invocations | 258.2 ms | 113.3 ms |

The median decreased by about 35%. These are sequential local runs with warm
filesystem caches, not a reproduction of an intermittent stall after a long
idle period. They include tmux scheduling and terminal rendering; system load
and terminal settings can change the results.
