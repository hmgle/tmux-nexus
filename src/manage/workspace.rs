use anyhow::{Context, Result};

use super::{
    choose, client_switch_commands, command, destructive_tmux, pane_snapshot, prompt_text,
    resolve_action, rows, selected_rows, session_entries, window_entries, ManagerContext,
    PaneEntry, PaneSnapshot, Row, SessionEntry, WindowEntry,
};
use crate::tmux;

pub(super) fn run_session(action: Option<&str>, context: &ManagerContext) -> Result<()> {
    let config = &context.config;
    let actions = ["switch", "new", "rename", "detach", "kill"];
    let action = resolve_action(action, &actions, context, "session action> ")?;
    if action.is_empty() {
        return Ok(());
    }
    if action == "new" {
        if let Some(name) = prompt_text(context, "new session> ", "Enter a new session name")? {
            let mut commands = vec![command(["new-session", "-d", "-s", name.as_str()])];
            commands.extend(client_switch_commands(&name));
            context.tmux.run_commands(&commands)?;
        }
        return Ok(());
    }
    let mut sessions = session_entries(config)?;
    let current = tmux::try_stdout(["display-message", "-p", "#{session_id}"]).unwrap_or_default();
    if action == "switch" && !config.switch_current {
        sessions.retain(|session| session.row.id != current);
    }
    if action == "detach" {
        sessions.retain(|session| session.attached_clients > 0);
    }
    let rows: Vec<Row> = sessions.iter().map(|session| session.row.clone()).collect();
    let multi = matches!(action.as_str(), "detach" | "kill");
    let indexes = choose(
        &rows,
        context,
        "session> ",
        if multi {
            "TAB selects multiple sessions"
        } else {
            "Select a session"
        },
        multi,
        Some("session"),
    )?;
    if indexes.is_empty() {
        return Ok(());
    }
    let selected: Vec<&SessionEntry> = indexes.iter().map(|index| &sessions[*index]).collect();
    let client = &context.tmux;
    match action.as_str() {
        "switch" => {
            let commands = client_switch_commands(&selected[0].row.id);
            if commands.is_empty() {
                Ok(())
            } else {
                client.run_commands(&commands)
            }
        }
        "rename" => {
            if let Some(name) = prompt_text(context, "rename session> ", "Enter a new name")? {
                client.run([
                    "rename-session",
                    "-t",
                    selected[0].row.id.as_str(),
                    name.as_str(),
                ])?;
            }
            Ok(())
        }
        "detach" => destructive_tmux(
            context,
            "Detach selected session client(s)?",
            selected
                .iter()
                .map(|session| command(["detach-client", "-s", session.row.id.as_str()]))
                .collect(),
        ),
        "kill" => destructive_tmux(
            context,
            "Kill selected session(s)?",
            selected
                .iter()
                .map(|session| command(["kill-session", "-t", session.row.id.as_str()]))
                .collect(),
        ),
        _ => unreachable!(),
    }
}

pub(super) fn run_window(action: Option<&str>, context: &ManagerContext) -> Result<()> {
    let config = &context.config;
    let actions = ["switch", "link", "move", "swap", "rename", "kill"];
    let action = resolve_action(action, &actions, context, "window action> ")?;
    if action.is_empty() {
        return Ok(());
    }
    let mut windows = window_entries(config)?;
    let current = context
        .tmux
        .stdout(["display-message", "-p", "#{session_id}\t#{window_id}"])?;
    let (current_session, current_window) = current
        .trim_end()
        .split_once('\t')
        .context("tmux returned invalid current window identifiers")?;
    if matches!(action.as_str(), "link" | "move") {
        windows.retain(|window| window.session_id != current_session);
        let rows: Vec<Row> = windows.iter().map(|window| window.row.clone()).collect();
        let indexes = choose(
            &rows,
            context,
            "source window> ",
            "Select a source window",
            false,
            Some("window"),
        )?;
        if let Some(index) = indexes.first() {
            let window = &windows[*index];
            let command_name = if action == "link" {
                "link-window"
            } else {
                "move-window"
            };
            tmux::run([
                command_name,
                "-a",
                "-s",
                window.row.id.as_str(),
                "-t",
                current_session,
            ])?;
        }
        return Ok(());
    }
    if action == "switch" && !config.switch_current {
        windows.retain(|window| {
            window.session_id != current_session || window.window_id != current_window
        });
    }
    let rows: Vec<Row> = windows.iter().map(|window| window.row.clone()).collect();
    let indexes = choose(
        &rows,
        context,
        "window> ",
        if action == "kill" {
            "TAB selects multiple windows"
        } else {
            "Select a window"
        },
        action == "kill",
        Some("window"),
    )?;
    if indexes.is_empty() {
        return Ok(());
    }
    let selected: Vec<&WindowEntry> = indexes.iter().map(|index| &windows[*index]).collect();
    let client = &context.tmux;
    match action.as_str() {
        "switch" => {
            let mut commands = client_switch_commands(&selected[0].session_id);
            commands.push(command([
                "select-window",
                "-t",
                selected[0].row.id.as_str(),
            ]));
            client.run_commands(&commands)
        }
        "rename" => {
            if let Some(name) = prompt_text(context, "rename window> ", "Enter a new name")? {
                client.run([
                    "rename-window",
                    "-t",
                    selected[0].row.id.as_str(),
                    name.as_str(),
                ])?;
            }
            Ok(())
        }
        "swap" => {
            let source = selected[0].row.id.clone();
            let source_window = selected[0].window_id.clone();
            drop(selected);
            windows.retain(|window| window.window_id != source_window);
            let rows: Vec<Row> = windows.iter().map(|window| window.row.clone()).collect();
            let other = choose(
                &rows,
                context,
                "swap with> ",
                "Select another window",
                false,
                Some("window"),
            )?;
            if let Some(index) = other.first() {
                client.run([
                    "swap-window",
                    "-s",
                    source.as_str(),
                    "-t",
                    windows[*index].row.id.as_str(),
                ])?;
            }
            Ok(())
        }
        "kill" => destructive_tmux(
            context,
            "Unlink/kill selected window(s)?",
            selected
                .iter()
                .map(|window| command(["unlink-window", "-k", "-t", window.row.id.as_str()]))
                .collect(),
        ),
        _ => unreachable!(),
    }
}

pub(super) fn run_pane(action: Option<&str>, context: &ManagerContext) -> Result<()> {
    let config = &context.config;
    let actions = [
        "switch", "break", "join", "swap", "layout", "kill", "resize",
    ];
    let action = resolve_action(action, &actions, context, "pane action> ")?;
    if action.is_empty() {
        return Ok(());
    }
    if action == "layout" {
        let layouts = rows(&[
            "even-horizontal",
            "even-vertical",
            "main-horizontal",
            "main-vertical",
            "tiled",
        ]);
        if let Some(layout) = selected_rows(
            &layouts,
            context,
            "layout> ",
            "Select a layout",
            false,
            None,
        )?
        .first()
        {
            tmux::run(["select-layout", layout.id.as_str()])?;
        }
        return Ok(());
    }
    if action == "resize" {
        return resize_pane(context);
    }
    let PaneSnapshot {
        mut panes,
        current_session,
        current_pane,
    } = pane_snapshot(config, &context.tmux)?;
    if action == "join" || (action == "switch" && !config.switch_current) {
        panes.retain(|pane| pane.pane_id != current_pane);
    }
    let rows: Vec<Row> = panes.iter().map(|pane| pane.row.clone()).collect();
    let multi = matches!(action.as_str(), "join" | "kill");
    let indexes = choose(
        &rows,
        context,
        "pane> ",
        if multi {
            "TAB selects multiple panes"
        } else {
            "Select a pane"
        },
        multi,
        Some("pane"),
    )?;
    if indexes.is_empty() {
        return Ok(());
    }
    let selected: Vec<&PaneEntry> = indexes.iter().map(|index| &panes[*index]).collect();
    let client = &context.tmux;
    match action.as_str() {
        "switch" => {
            let mut commands = client_switch_commands(&selected[0].session_id);
            commands.push(command([
                "select-window",
                "-t",
                selected[0].window_id.as_str(),
            ]));
            commands.push(command(["select-pane", "-t", selected[0].pane_id.as_str()]));
            client.run_commands(&commands)
        }
        "break" => client.run([
            "break-pane",
            "-d",
            "-s",
            selected[0].pane_id.as_str(),
            "-t",
            &format!("{current_session}:"),
        ]),
        "join" => {
            let commands = selected
                .iter()
                .map(|pane| {
                    command([
                        "move-pane",
                        "-s",
                        pane.pane_id.as_str(),
                        "-t",
                        current_pane.as_str(),
                    ])
                })
                .collect::<Vec<_>>();
            client.run_commands(&commands)
        }
        "swap" => {
            let source = selected[0].pane_id.clone();
            drop(selected);
            panes.retain(|pane| pane.pane_id != source);
            let rows: Vec<Row> = panes.iter().map(|pane| pane.row.clone()).collect();
            let other = choose(
                &rows,
                context,
                "swap with> ",
                "Select another pane",
                false,
                Some("pane"),
            )?;
            if let Some(index) = other.first() {
                client.run([
                    "swap-pane",
                    "-s",
                    source.as_str(),
                    "-t",
                    panes[*index].pane_id.as_str(),
                ])?;
            }
            Ok(())
        }
        "kill" => destructive_tmux(
            context,
            "Kill selected pane(s)?",
            selected
                .iter()
                .map(|pane| command(["kill-pane", "-t", pane.pane_id.as_str()]))
                .collect(),
        ),
        _ => unreachable!(),
    }
}

fn resize_pane(context: &ManagerContext) -> Result<()> {
    let directions = rows(&["left", "right", "up", "down"]);
    let Some(direction) = selected_rows(
        &directions,
        context,
        "resize> ",
        "Select a direction",
        false,
        None,
    )?
    .first()
    .cloned() else {
        return Ok(());
    };
    let sizes = if matches!(direction.id.as_str(), "left" | "right") {
        rows(&["1", "2", "3", "5", "10", "20", "30"])
    } else {
        rows(&["1", "2", "3", "5", "10", "15", "20"])
    };
    let Some(size) = selected_rows(
        &sizes,
        context,
        "size> ",
        "Select cells to adjust",
        false,
        None,
    )?
    .first()
    .cloned() else {
        return Ok(());
    };
    let flag = match direction.id.as_str() {
        "left" => "-L",
        "right" => "-R",
        "up" => "-U",
        _ => "-D",
    };
    tmux::run(["resize-pane", flag, size.id.as_str()])
}
