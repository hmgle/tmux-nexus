# Motion overlays and tmux zoom

Motion uses a borderless popup on tmux without floating panes, and a prepared
temporary hint window when `list-commands new-pane` reports that command.
This capability check deliberately covers all floating-pane versions rather
than guessing which version is affected by a particular zoom regression.

## Why the window backend exists

Opening or closing `display-popup` on tmux master can temporarily unzoom a
tiled source pane. Restoring its zoom flag and dimensions does not guarantee
that its text survived: shrinking a screen whose cursor is near the top can
permanently discard lower rows from both the screen and history.

The minimal reproduction was verified on unmodified tmux master
[`9a37890133a5bf610cdca2d6036fdce971715322`](https://github.com/tmux/tmux/commit/9a37890133a5bf610cdca2d6036fdce971715322)
on 2026-10-14. In an isolated 96×24 attached terminal, create two vertically
stacked tiled panes, zoom the top pane, and leave this primary-screen fixture
idle with its cursor at the top left:

```sh
printf '\033[H\033[2JTOP\033[22;1HBOTTOM\033[H'
exec sleep 600
```

Open a 30×8 `display-popup -E` and inspect the source with
`capture-pane -p -S -`. `BOTTOM` disappears on opening. Redrawing the fixture
while the popup is open shows that closing independently loses it again.
The pane reports 96×23, zoom enabled, cursor (0,0), and no history before and
after each operation. Server logs show a 96×23 → 96×11 → 96×23 resize during
both opening and closing. This ASCII reproduction passes on tmux 3.4 and
3.7c; it does not require wide characters, plugins, or user configuration.

In that master revision,
[`cmd_display_popup_exec()`](https://github.com/tmux/tmux/blob/9a37890133a5bf610cdca2d6036fdce971715322/cmd-display-menu.c#L475)
and
[`server_destroy_pane()`](https://github.com/tmux/tmux/blob/9a37890133a5bf610cdca2d6036fdce971715322/server-fn.c#L423)
push and restore zoom around popup creation and destruction.
[`window_push_zoom()`](https://github.com/tmux/tmux/blob/9a37890133a5bf610cdca2d6036fdce971715322/window.c#L1123)
calls `window_unzoom()`, restoring the tiled layout and resizing its panes.
This explains why refreshing capture coordinates alone cannot recover the
lost content.

## Related upstream work

[tmux PR #5668, “Zoomed and floating panes”](https://github.com/tmux/tmux/pull/5668)
reworks zoom into per-pane stacking state instead of swapping the whole
layout. As checked on 2026-10-15, it is open and is not merged into the master
revision tested above. Its branch has not been validated here, so it should
be treated as related work rather than an available or confirmed fix.
Related upstream reports include
[#5258](https://github.com/tmux/tmux/issues/5258) and
[#5716](https://github.com/tmux/tmux/issues/5716); the latter concerns
wide-character reflow, which the ASCII reproduction does not exercise.

Before narrowing the capability check or returning to floating popups, verify
both opening and closing against the minimal fixture and the motion tests.
A version string or a restored zoom flag alone is insufficient evidence.

## Repository safeguards

The window backend prepares its complete hint frame before switching the
client, leaving the source pane's geometry untouched. Its supervisor removes
the hint window on completion, failure, client detachment, or a deliberate
window switch. Selection positions the source cursor before returning to it.
The older popup backend refreshes the original panes after opening, so hints
reflect any shell redraw that occurred during popup creation.

`tests/motion-integration.py` checks one- and two-character selection and
cancellation in tiled and zoomed layouts, including source grid, cursor,
dimensions, zoom state, and layout hooks. It also checks an idle primary
screen with text near the bottom and the cursor at the top, comparing the
complete source screen and history while hints are open and after exit.
`tests/motion-lifecycle.py` covers window ownership, first-frame readiness,
failure cleanup, and a single source redraw with the final cursor visible.

CI exercises the popup backend on the system tmux and the window backend on
pinned tmux 3.7c. To check another build, put it first in `PATH`, then run the
development checks in [README.md](../README.md#development). Run lifecycle
tests only for the window backend.
