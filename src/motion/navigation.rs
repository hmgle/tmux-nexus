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

    let mut rows_remaining = line_no;
    let mut cursor_right = true_col;
    let first_non_empty = pane
        .lines
        .iter()
        .take(line_no.saturating_add(1))
        .position(|line| !line.is_empty());
    let anchor = first_non_empty.filter(|anchor| {
        *anchor > 0
            && (pane.lines[*anchor]
                .chars()
                .next()
                .is_some_and(|character| !character.is_whitespace())
                || *anchor == line_no)
    });
    if let Some(anchor) = anchor {
        commands.push(repeat_command(pane, anchor, "cursor-down"));
        // Empty rows leave cx == ox == 0, so tmux does not update lastsx.
        // On the next non-empty row, cx >= lastsx then triggers tmux's
        // snap-to-end-of-line behavior.  Back-to-indentation establishes a
        // known content cell on the first safe physical row without
        // start-of-line's wrap-aware jump to the beginning of the logical
        // line.
        commands.push(copy_command(pane, &["back-to-indentation"]));
        rows_remaining -= anchor;

        // When the target itself is the first non-empty row and starts with
        // spaces or a tab, back-to-indentation lands on the first content
        // cell.  true_col is measured from the physical row's start, so
        // advance only past the leading whitespace that was skipped.
        if anchor == line_no {
            let leading = pane.lines[anchor]
                .chars()
                .take_while(|character| matches!(character, ' ' | '\t'))
                .count();
            cursor_right = true_col.saturating_sub(leading);
        }

        // If the logical line begins above the visible pane with content,
        // tmux may still scroll while resolving the wrapped line.  Copy mode
        // has no primitive for a physical-row start that avoids this case.
    }
    if rows_remaining > 0 {
        commands.push(repeat_command(pane, rows_remaining, "cursor-down"));
    }
    if cursor_right > 0 {
        commands.push(repeat_command(pane, cursor_right, "cursor-right"));
    }
    commands
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
