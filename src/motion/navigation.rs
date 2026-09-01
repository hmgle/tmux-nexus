use std::ffi::OsString;

use anyhow::{Context, Result};

use super::{rendering::true_position, Match, Pane, TabMode};
use crate::tmux;

pub(super) fn move_to_match(
    panes: &[Pane],
    target: &Match,
    tab_mode: TabMode,
    target_client: Option<&str>,
) -> Result<()> {
    let pane = panes
        .get(target.pane_index)
        .context("motion target pane not found")?;
    let line = pane
        .lines
        .get(target.line_no)
        .context("motion target line not found")?;
    let true_col = true_position(line, target.visual_col, tab_mode);
    move_cursor(pane, target.line_no, true_col, target_client)
}

fn move_cursor(
    pane: &Pane,
    line_no: usize,
    true_col: usize,
    target_client: Option<&str>,
) -> Result<()> {
    let client = tmux::TmuxClient::from_env()?;
    move_cursor_with_client(&client, pane, line_no, true_col, target_client)
}

pub(super) fn move_cursor_with_client(
    client: &tmux::TmuxClient,
    pane: &Pane,
    line_no: usize,
    true_col: usize,
    target_client: Option<&str>,
) -> Result<()> {
    let commands = navigation_commands(pane, line_no, true_col, target_client);
    client
        .run_commands(&commands)
        .context("failed to move motion cursor")
}

fn navigation_commands(
    pane: &Pane,
    line_no: usize,
    true_col: usize,
    target_client: Option<&str>,
) -> Vec<Vec<OsString>> {
    let mut commands = Vec::new();
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
    commands.push(command(["select-pane", "-t", pane.pane_id.as_str()]));
    if !pane.copy_mode {
        commands.push(command(["copy-mode", "-t", pane.pane_id.as_str()]));
    }
    commands.push(copy_command(pane, &["top-line"]));

    // Motion captures do not include tmux's wrap flags. If the logical line
    // starts above the viewport and the captured first row is an empty wrapped
    // continuation, back-to-indentation can resolve above the capture. The
    // resulting physical target row is then undercounted; capture wrap flags
    // are required to eliminate this straddle limitation.
    let mut rows_remaining = line_no;
    let anchor = pane
        .lines
        .iter()
        .take(line_no.saturating_add(1))
        .position(|line| line.chars().any(|character| !is_blank(character)))
        .filter(|anchor| *anchor > 0);
    if let Some(anchor) = anchor {
        commands.push(repeat_command(pane, anchor, "cursor-down"));
        // Empty rows leave cx == ox == 0, so tmux does not update lastsx. On
        // the next non-empty row, cx >= lastsx && cx != px makes tmux snap to
        // that physical row's end. A following cursor-right then crosses into
        // the next row. Back-to-indentation is wrap-aware and starts at the
        // logical line start; with a non-straddling capture, all rows before
        // this anchor are blank, so its first content cell is on the anchor.
        commands.push(copy_command(pane, &["back-to-indentation"]));
        let leading: String = pane.lines[anchor]
            .chars()
            .take_while(|character| is_blank(*character))
            .collect();
        if !leading.is_empty() {
            if anchor < line_no && leading.contains('\t') {
                // A tab's padding makes cursor-left wrap before it reaches
                // column zero. The target still needs more vertical motion,
                // so use start-of-line here; the straddled-wrap limitation
                // above applies if this row is itself a continuation.
                commands.push(copy_command(pane, &["start-of-line"]));
            } else {
                // For spaces, moving left one source cell at a time reaches
                // column zero without crossing the anchor row. A tab is kept
                // as one source cell when the anchor is the target itself;
                // cursor-right then preserves its normal tab positioning.
                commands.push(repeat_command(pane, leading.chars().count(), "cursor-left"));
            }
        }
        rows_remaining -= anchor;
    }
    if rows_remaining > 0 {
        commands.push(repeat_command(pane, rows_remaining, "cursor-down"));
    }
    if true_col > 0 {
        commands.push(repeat_command(pane, true_col, "cursor-right"));
    }
    commands
}

fn is_blank(character: char) -> bool {
    matches!(character, ' ' | '\t')
}

fn repeat_command(pane: &Pane, count: usize, name: &str) -> Vec<OsString> {
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
