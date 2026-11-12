# Manager popup startup

The pane switcher follows this path:

```text
prefix + f -> tmux run-shell -b -> tnx launcher -> Rust manager
           -> read configuration and pane snapshot -> fzf --tmux
           -> native tmux popup -> selected pane
```

The manager uses native fzf popups (fzf 0.53+ and tmux 3.3+). It does not run
the `fzf-tmux` Bash wrapper or probe program versions on each invocation.
Existing `-p -w 86% -h 58%` settings are translated to
`--tmux=center,86%,58%` inside the manager. New configurations can use the native
option directly.

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
143 panes, a 160x44 terminal, and the same 86%x58% popup and custom pane format:

| Keypress to prompt | Before (0748ddb) | After |
| --- | ---: | ---: |
| First invocation in the test server | 258.2 ms | 112.3 ms |
| Median of 20 invocations | 140.6 ms | 90.8 ms |
| 95th percentile of 20 invocations | 258.2 ms | 113.3 ms |

The median decreased by about 35%. These are sequential local runs with warm
filesystem caches, not a reproduction of an intermittent stall after a long
idle period. They include tmux scheduling and terminal rendering; system load
and terminal settings can change the results.
