use std::sync::OnceLock;

use anyhow::{Context, Result};

use crate::{
    config::Config,
    index::{LegacyRecord, SearchIndex},
    tmux,
    types::ActionKind,
    util::trim_prefix_chars,
};

struct ActionTarget<'a> {
    pane_id: &'a str,
    location: &'a str,
    raw_line_no: usize,
    text: &'a str,
}

pub fn execute(
    index: &SearchIndex,
    record_id: usize,
    action: ActionKind,
    _config: &Config,
) -> Result<()> {
    let record = index
        .record(record_id)
        .with_context(|| format!("record {record_id} not found"))?;
    let pane = index
        .pane_for(record)
        .context("pane not found for record")?;
    let text = index
        .text_for(record)
        .context("text not found for record")?;
    let history_line_no = history_line_no(pane.history_start_line, record.raw_line_no());
    perform(
        action,
        &ActionTarget {
            pane_id: &pane.pane_id,
            location: pane.location(),
            raw_line_no: history_line_no,
            text,
        },
    )
}

pub fn execute_legacy(record: &LegacyRecord, action: ActionKind) -> Result<()> {
    perform(
        action,
        &ActionTarget {
            pane_id: &record.pane_id,
            location: &record.location,
            raw_line_no: record.line_no,
            text: &record.text,
        },
    )
}

fn perform(action: ActionKind, target: &ActionTarget<'_>) -> Result<()> {
    match action {
        ActionKind::Jump => jump(target),
        ActionKind::Copy => copy_text(target.text),
        ActionKind::Send => {
            tmux::run(["send-keys", "-l", "--", target.text])?;
            Ok(())
        }
        ActionKind::Print => {
            println!("{}", target.text);
            Ok(())
        }
    }
}

fn jump(target: &ActionTarget<'_>) -> Result<()> {
    let (session, window) = split_location(target.location);
    if !session.is_empty() {
        tmux::run_ignore(["switch-client", "-t", session]);
    }
    if !session.is_empty() && !window.is_empty() {
        let window_target = format!("{session}:{window}");
        tmux::run(["select-window", "-t", window_target.as_str()])
            .context("failed to select target window")?;
    }
    tmux::run(["select-pane", "-t", target.pane_id]).context("failed to select target pane")?;
    tmux::run(["copy-mode", "-t", target.pane_id]).context("failed to enter copy mode")?;

    if let Some(copy_line) = exact_copy_line(target.pane_id, target.raw_line_no) {
        let line = copy_line.to_string();
        tmux::run([
            "send-keys",
            "-t",
            target.pane_id,
            "-X",
            "goto-line",
            line.as_str(),
        ])
        .context("failed to jump to selected history line")?;
        return Ok(());
    }

    let approx = target.raw_line_no.saturating_sub(5).max(1).to_string();
    tmux::run([
        "send-keys",
        "-t",
        target.pane_id,
        "-X",
        "goto-line",
        approx.as_str(),
    ])
    .context("failed to position near selected history line")?;

    let needle = trim_prefix_chars(target.text, 80);
    let search_text = needle.trim_start_matches('-');
    if search_text.is_empty() {
        let raw = target.raw_line_no.to_string();
        tmux::run([
            "send-keys",
            "-t",
            target.pane_id,
            "-X",
            "goto-line",
            raw.as_str(),
        ])
        .context("failed to jump to selected history line")?;
    } else {
        tmux::run([
            "send-keys",
            "-t",
            target.pane_id,
            "-X",
            "search-forward-text",
            search_text,
        ])
        .context("failed to search for selected history text")?;
    }

    Ok(())
}

fn exact_copy_line(pane_id: &str, history_line_no: usize) -> Option<usize> {
    let output = tmux::try_stdout([
        "display-message",
        "-p",
        "-t",
        pane_id,
        "#{history_size},#{pane_height}",
    ])?;
    let (history_size, pane_height) = output.split_once(',')?;
    let history_size = history_size.parse::<usize>().ok()?;
    let pane_height = pane_height.parse::<usize>().ok()?;
    copy_line_from_bounds(history_size, pane_height, history_line_no)
}

fn copy_line_from_bounds(
    history_size: usize,
    pane_height: usize,
    history_line_no: usize,
) -> Option<usize> {
    let last_line = history_size.checked_add(pane_height)?;
    (history_line_no > 0 && history_line_no < last_line).then_some(last_line - history_line_no)
}

fn history_line_no(history_start_line: usize, raw_line_no: usize) -> usize {
    history_start_line + raw_line_no
}

fn split_location(location: &str) -> (&str, &str) {
    let Some((session, rest)) = location.rsplit_once(':') else {
        return ("", "");
    };
    let window = rest.split('.').next().unwrap_or_default();
    (session, window)
}

fn copy_text(text: &str) -> Result<()> {
    tmux::run(["set-buffer", "--", text]).context("failed to update tmux buffer")?;

    let mut tried_clipboard = false;
    for command in clipboard_commands() {
        tried_clipboard = true;
        if tmux::write_to_command(command.program, command.args, text).is_ok() {
            return Ok(());
        }
    }

    let reason = if tried_clipboard {
        "system clipboard helper failed"
    } else {
        "no system clipboard found"
    };
    tmux::display_message(&format!("tmux-nexus: copied to tmux buffer ({reason})"));
    Ok(())
}

struct ClipboardCommand {
    program: &'static str,
    args: &'static [&'static str],
}

fn clipboard_commands() -> &'static [ClipboardCommand] {
    static COMMANDS: OnceLock<Vec<ClipboardCommand>> = OnceLock::new();
    COMMANDS.get_or_init(|| {
        [
            ClipboardCommand {
                program: "pbcopy",
                args: &[],
            },
            ClipboardCommand {
                program: "wl-copy",
                args: &[],
            },
            ClipboardCommand {
                program: "xclip",
                args: &["-selection", "clipboard"],
            },
            ClipboardCommand {
                program: "xsel",
                args: &["--clipboard", "--input"],
            },
            ClipboardCommand {
                program: "clip.exe",
                args: &[],
            },
        ]
        .into_iter()
        .filter(|command| tmux::have(command.program))
        .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::{copy_line_from_bounds, history_line_no, split_location};

    #[test]
    fn unbounded_capture_uses_snapshot_line_number() {
        assert_eq!(history_line_no(0, 42), 42);
    }

    #[test]
    fn limited_capture_restores_history_offset() {
        assert_eq!(history_line_no(1200, 25), 1225);
    }

    #[test]
    fn exact_copy_line_counts_from_the_bottom() {
        assert_eq!(copy_line_from_bounds(23, 8, 20), Some(11));
        assert_eq!(copy_line_from_bounds(23, 8, 30), Some(1));
        assert_eq!(copy_line_from_bounds(23, 8, 31), None);
    }

    #[test]
    fn location_parser_preserves_colons_in_session_names() {
        assert_eq!(split_location("foo:bar:2.1"), ("foo:bar", "2"));
    }
}
