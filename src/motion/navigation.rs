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
    move_cursor(pane, target.line_no, true_col, target_client, tab_mode)
}

fn move_cursor(
    pane: &Pane,
    line_no: usize,
    true_col: usize,
    target_client: Option<&str>,
    tab_mode: TabMode,
) -> Result<()> {
    let client = tmux::TmuxClient::from_env()?;
    move_cursor_with_client(&client, pane, line_no, true_col, target_client, tab_mode)
}

pub(super) fn move_cursor_with_client(
    client: &tmux::TmuxClient,
    pane: &Pane,
    line_no: usize,
    true_col: usize,
    target_client: Option<&str>,
    tab_mode: TabMode,
) -> Result<()> {
    let mut commands = navigation_commands(pane, line_no, target_client, tab_mode);
    if true_col > 0 {
        let columns = true_col.to_string();
        commands.push(copy_command(
            pane,
            &["-N", columns.as_str(), "cursor-right"],
        ));
    }
    client
        .run_commands(&commands)
        .context("failed to move motion cursor")
}

fn navigation_commands(
    pane: &Pane,
    line_no: usize,
    target_client: Option<&str>,
    tab_mode: TabMode,
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
    if tab_mode == TabMode::PositionAware {
        let first_non_empty = pane
            .lines
            .iter()
            .position(|line| !line.is_empty())
            .unwrap_or_default();
        if first_non_empty > 0 && first_non_empty <= line_no {
            let first = first_non_empty.to_string();
            commands.push(copy_command(pane, &["-N", first.as_str(), "cursor-down"]));
            // A leading empty row can retain tmux's previous last-column
            // value on tmux 3.6+. Reset it only after reaching a non-empty
            // physical row. Calling `start-of-line` on the first visible row
            // would be unsafe because it may be a wrapped continuation and
            // change the viewport row we are targeting.
            commands.push(copy_command(pane, &["start-of-line"]));
            rows_remaining -= first_non_empty;
        }
    }
    if rows_remaining > 0 {
        let rows = rows_remaining.to_string();
        commands.push(copy_command(pane, &["-N", rows.as_str(), "cursor-down"]));
    }
    commands
}

fn copy_command(pane: &Pane, args: &[&str]) -> Vec<OsString> {
    let mut values = command(["send-keys", "-X", "-t", pane.pane_id.as_str()]);
    values.extend(args.iter().map(OsString::from));
    values
}

fn command<const N: usize>(args: [&str; N]) -> Vec<OsString> {
    args.into_iter().map(OsString::from).collect()
}
