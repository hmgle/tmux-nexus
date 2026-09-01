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
    _tab_mode: TabMode,
) -> Result<()> {
    let commands = navigation_commands(pane, line_no, target_client);
    client
        .run_commands(&commands)
        .context("failed to move motion cursor")?;

    // `cursor-down` can retain tmux's last-column value when it crosses an
    // empty row (on tmux 3.6+).  Read the resulting grid column and return to
    // column zero with `cursor-left` before moving to the target.  Using
    // `start-of-line` here would be incorrect: on a wrapped continuation row
    // it jumps to the beginning of the logical line and changes the row (and
    // scroll position) we just selected.
    let cursor_x = copy_cursor_x(client, pane)?;
    if cursor_x == 0 && true_col == 0 {
        return Ok(());
    }

    let mut horizontal = Vec::with_capacity(2);
    if cursor_x > 0 {
        let columns = cursor_x.to_string();
        horizontal.push(copy_command(pane, &["-N", columns.as_str(), "cursor-left"]));
    }
    if true_col > 0 {
        let columns = true_col.to_string();
        horizontal.push(copy_command(
            pane,
            &["-N", columns.as_str(), "cursor-right"],
        ));
    }
    client
        .run_commands(&horizontal)
        .context("failed to move motion cursor horizontally")
}

fn navigation_commands(
    pane: &Pane,
    line_no: usize,
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
    if line_no > 0 {
        let rows = line_no.to_string();
        commands.push(copy_command(pane, &["-N", rows.as_str(), "cursor-down"]));
    }
    commands
}

fn copy_cursor_x(client: &tmux::TmuxClient, pane: &Pane) -> Result<usize> {
    let output = client.stdout([
        "display-message",
        "-p",
        "-t",
        pane.pane_id.as_str(),
        "#{copy_cursor_x}",
    ])?;
    output
        .trim()
        .parse()
        .context("malformed tmux copy cursor column")
}

fn copy_command(pane: &Pane, args: &[&str]) -> Vec<OsString> {
    let mut values = command(["send-keys", "-X", "-t", pane.pane_id.as_str()]);
    values.extend(args.iter().map(OsString::from));
    values
}

fn command<const N: usize>(args: [&str; N]) -> Vec<OsString> {
    args.into_iter().map(OsString::from).collect()
}
