use std::ffi::OsString;

use anyhow::{Context, Result};

use super::{rendering::true_position, Match, Pane, TabMode};
use crate::tmux;

pub(super) fn move_to_match(
    panes: &[Pane],
    target: &Match,
    tab_mode: TabMode,
    target_client: Option<&str>,
    activate_window: bool,
) -> Result<()> {
    let pane = panes
        .get(target.pane_index)
        .context("motion target pane not found")?;
    let line = pane
        .lines
        .get(target.line_no)
        .context("motion target line not found")?;
    let true_col = true_position(line, target.visual_col, tab_mode);
    move_cursor(
        pane,
        target.line_no,
        true_col,
        target_client,
        activate_window,
    )
}

fn move_cursor(
    pane: &Pane,
    line_no: usize,
    true_col: usize,
    target_client: Option<&str>,
    activate_window: bool,
) -> Result<()> {
    let client = tmux::TmuxClient::from_env()?;
    move_cursor_with_client(
        &client,
        pane,
        line_no,
        true_col,
        target_client,
        activate_window,
    )
}

pub(super) fn move_cursor_with_client(
    client: &tmux::TmuxClient,
    pane: &Pane,
    line_no: usize,
    true_col: usize,
    target_client: Option<&str>,
    activate_window: bool,
) -> Result<()> {
    let commands = navigation_commands(pane, line_no, true_col, target_client, activate_window);
    client
        .run_commands(&commands)
        .context("failed to move motion cursor")
}

fn navigation_commands(
    pane: &Pane,
    line_no: usize,
    true_col: usize,
    target_client: Option<&str>,
    activate_window: bool,
) -> Vec<Vec<OsString>> {
    let mut commands = Vec::new();
    commands.push(command(["select-pane", "-t", pane.pane_id.as_str()]));
    if !pane.copy_mode {
        commands.push(command(["copy-mode", "-t", pane.pane_id.as_str()]));
    }
    commands.push(copy_command(pane, &["top-line"]));

    let mut rows_remaining = line_no;
    // A blank-only first row leaves tmux's lastsx at zero, so later
    // cursor-down calls snap to each content row's end. A first row with
    // content updates lastsx on the initial cursor-down and needs no anchor.
    let needs_anchor = line_no > 0
        && pane
            .lines
            .first()
            .is_some_and(|line| line.chars().all(is_blank));
    let anchor = needs_anchor.then(|| {
        // Anchor on the first row whose leading blanks are spaces only. Rows
        // with a leading tab are skipped: back-to-indentation lands on their
        // first content cell, but cursor-left cannot retreat through tab
        // padding to column zero, and the leftover column would offset the
        // rows still below. Without such a row the target anchors itself; no
        // further cursor-down follows, so its retreat stays on that row.
        pane.lines
            .iter()
            .take(line_no.saturating_add(1))
            .position(|line| is_tab_free_content_row(line.as_str()))
            .unwrap_or(line_no)
    });
    if let Some(anchor) = anchor {
        commands.push(repeat_command(pane, anchor, "cursor-down"));
        // Empty rows leave cx == ox == 0, so tmux does not update lastsx. On
        // the next non-empty row, cx >= lastsx && cx != px makes tmux snap to
        // that physical row's end. A following cursor-right then crosses into
        // the next row. Back-to-indentation is wrap-aware and starts at the
        // logical line start; with a non-straddling capture, all rows before
        // this anchor are blank, so its first content cell is on the anchor.
        // Motion captures do not include tmux's wrap flags, so straddled
        // wraps can undercount the physical target row: back-to-indentation
        // resolves either above the capture (the first captured row is an
        // empty wrapped continuation of a line that starts higher) or onto
        // an earlier row of the anchor's own logical line. Capture wrap
        // flags are required to eliminate this.
        commands.push(copy_command(pane, &["back-to-indentation"]));
        // cursor-left crosses to the previous row when a step starts at
        // column zero or on tab padding anchored there. Retreating exactly
        // the leading blank count never starts such a step — each blank is
        // one step, a tab's padding included — so a match on a leading tab
        // stops on the tab run's last cell.
        let line = &pane.lines[anchor];
        let leading = leading_blanks(line);
        // An all-blank anchor row (reachable via a pure-space pattern) skips
        // the retreat: back-to-indentation finds no content cell, so the
        // cursor keeps the snapped end column — row correct, column off by
        // up to one tab width. Accepted: no reliable landing exists.
        if leading > 0 && leading < line.chars().count() {
            commands.push(repeat_command(pane, leading, "cursor-left"));
        }
        rows_remaining -= anchor;
    }
    if rows_remaining > 0 {
        commands.push(repeat_command(pane, rows_remaining, "cursor-down"));
    }
    if true_col > 0 {
        commands.push(repeat_command(pane, true_col, "cursor-right"));
    }
    // Hint windows return through their supervisor so switching back and
    // unlinking the overlay can share one redraw of the positioned source.
    if !activate_window {
        return commands;
    }
    if let Some(target_client) = target_client {
        commands.push(command([
            "switch-client",
            "-c",
            target_client,
            "-t",
            pane.window_id.as_str(),
        ]));
    } else {
        commands.push(command(["select-window", "-t", pane.window_id.as_str()]));
    }
    commands
}

fn is_blank(character: char) -> bool {
    matches!(character, ' ' | '\t')
}

pub(super) fn is_tab_free_content_row(line: &str) -> bool {
    // take_while would swallow the first content character while testing
    // its predicate, so skip spaces explicitly and inspect what follows.
    line.chars()
        .find(|character| *character != ' ')
        .is_some_and(|character| !is_blank(character))
}

fn leading_blanks(line: &str) -> usize {
    line.chars()
        .take_while(|character| is_blank(*character))
        .count()
}

fn repeat_command(pane: &Pane, count: usize, name: &str) -> Vec<OsString> {
    // tmux rejects `-N 0` ("repeat count too small") with a nonzero exit
    // that fails the whole batched sequence; a zero count is a no-op, and
    // run_commands skips empty commands.
    if count == 0 {
        return Vec::new();
    }
    let count = count.to_string();
    copy_command(pane, &["-N", count.as_str(), name])
}

fn copy_command(pane: &Pane, args: &[&str]) -> Vec<OsString> {
    let mut values = command(["send-keys", "-X", "-t", pane.pane_id.as_str()]);
    values.extend(args.iter().map(OsString::from));
    values
}

fn command<const N: usize>(args: [&str; N]) -> Vec<OsString> {
    args.into_iter().map(OsString::from).collect()
}
