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
    raw_line_no: usize,
    exact_line_no: Option<usize>,
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
    let raw_line_no = history_line_no(pane.history_start_line, record.raw_line_no());
    let exact_line_no = record
        .grid_line_no()
        .map(|line_no| history_line_no(pane.history_start_line, line_no));
    perform(
        action,
        &ActionTarget {
            pane_id: &pane.pane_id,
            raw_line_no,
            exact_line_no,
            text,
        },
    )
}

pub fn execute_legacy(record: &LegacyRecord, action: ActionKind, config: &Config) -> Result<()> {
    let exact_line_no =
        (!config.join_wraps || tmux::capture_flags_supported()).then_some(record.line_no);
    perform(
        action,
        &ActionTarget {
            pane_id: &record.pane_id,
            raw_line_no: record.line_no,
            exact_line_no,
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
    if let Some(session_id) = tmux::try_stdout([
        "display-message",
        "-p",
        "-t",
        target.pane_id,
        "#{session_id}",
    ]) {
        tmux::run_ignore(["switch-client", "-t", session_id.as_str()]);
    }
    tmux::run(["select-window", "-t", target.pane_id]).context("failed to select target window")?;
    tmux::run(["select-pane", "-t", target.pane_id]).context("failed to select target pane")?;
    tmux::run(["copy-mode", "-t", target.pane_id]).context("failed to enter copy mode")?;

    if let Some(plan) = target
        .exact_line_no
        .and_then(|line_no| copy_plan(target.pane_id, line_no))
    {
        let scroll = plan.scroll.to_string();
        tmux::run([
            "send-keys",
            "-t",
            target.pane_id,
            "-X",
            "goto-line",
            scroll.as_str(),
        ])
        .context("failed to scroll to the selected history line")?;
        if plan.residual != 0 {
            let command = if plan.residual > 0 {
                "cursor-down"
            } else {
                "cursor-up"
            };
            let count = plan.residual.unsigned_abs().to_string();
            tmux::run([
                "send-keys",
                "-t",
                target.pane_id,
                "-X",
                "-N",
                count.as_str(),
                command,
            ])
            .context("failed to move to the selected history line")?;
        }
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

/// tmux copy mode's `goto-line` only sets the scroll offset; the cursor keeps
/// the row the pane left it on. An exact jump therefore needs the offset plus a
/// correction for the rows between that cursor row and the target row.
struct CopyPlan {
    scroll: usize,
    residual: isize,
}

fn copy_plan(pane_id: &str, history_line_no: usize) -> Option<CopyPlan> {
    let output = tmux::try_stdout([
        "display-message",
        "-p",
        "-t",
        pane_id,
        "#{history_size},#{pane_height},#{copy_cursor_y}",
    ])?;
    let mut fields = output.split(',');
    let history_size = fields.next()?.parse::<usize>().ok()?;
    let pane_height = fields.next()?.parse::<usize>().ok()?;
    let cursor_y = fields.next()?.parse::<usize>().ok()?;
    plan_from_bounds(history_size, pane_height, cursor_y, history_line_no)
}

fn plan_from_bounds(
    history_size: usize,
    pane_height: usize,
    cursor_y: usize,
    history_line_no: usize,
) -> Option<CopyPlan> {
    if history_line_no == 0 {
        return None;
    }
    let row = history_line_no - 1;
    if row >= history_size.checked_add(pane_height)? {
        return None;
    }

    // tmux reads a negative `goto-line` argument as the top of the history, so
    // clamp the offset here rather than letting it be reinterpreted.
    let scroll = (history_size + cursor_y)
        .saturating_sub(row)
        .min(history_size);
    let landed = history_size - scroll + cursor_y;
    Some(CopyPlan {
        scroll,
        residual: row as isize - landed as isize,
    })
}

fn history_line_no(history_start_line: usize, raw_line_no: usize) -> usize {
    history_start_line + raw_line_no
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
    use super::{history_line_no, plan_from_bounds};

    /// The absolute grid row the cursor ends on once the plan is applied.
    fn landed_row(
        history_size: usize,
        pane_height: usize,
        cursor_y: usize,
        history_line_no: usize,
    ) -> Option<isize> {
        let plan = plan_from_bounds(history_size, pane_height, cursor_y, history_line_no)?;
        Some((history_size - plan.scroll + cursor_y) as isize + plan.residual)
    }

    #[test]
    fn unbounded_capture_uses_snapshot_line_number() {
        assert_eq!(history_line_no(0, 42), 42);
    }

    #[test]
    fn limited_capture_restores_history_offset() {
        assert_eq!(history_line_no(1200, 25), 1225);
    }

    #[test]
    fn scroll_offset_counts_from_the_bottom_row() {
        let plan = plan_from_bounds(23, 8, 7, 20).unwrap();
        assert_eq!((plan.scroll, plan.residual), (11, 0));
        let plan = plan_from_bounds(23, 8, 7, 31).unwrap();
        assert_eq!((plan.scroll, plan.residual), (0, 0));
    }

    #[test]
    fn rows_past_the_bottom_grid_line_have_no_plan() {
        assert!(plan_from_bounds(23, 8, 7, 32).is_none());
        assert!(plan_from_bounds(23, 8, 7, 0).is_none());
    }

    /// `goto-line` never moves the cursor row, so a pane whose cursor sits above
    /// the last row needs the offset shifted by the same amount.
    #[test]
    fn scroll_offset_follows_the_pane_cursor_row() {
        assert_eq!(plan_from_bounds(93, 8, 0, 50).unwrap().scroll, 44);
        assert_eq!(plan_from_bounds(93, 8, 2, 50).unwrap().scroll, 46);
        assert_eq!(plan_from_bounds(93, 8, 7, 50).unwrap().scroll, 51);
    }

    #[test]
    fn clamped_offsets_are_corrected_by_cursor_motion() {
        // Above the cursor row at the top of the history: scrolling stops at
        // `history_size`, so the rest is walked upwards.
        let plan = plan_from_bounds(93, 8, 2, 1).unwrap();
        assert_eq!((plan.scroll, plan.residual), (93, -2));
        // Below the cursor row on the visible screen: scrolling stops at 0.
        let plan = plan_from_bounds(93, 8, 2, 99).unwrap();
        assert_eq!((plan.scroll, plan.residual), (0, 3));
    }

    #[test]
    fn every_reachable_row_lands_exactly() {
        for cursor_y in 0..8 {
            for history_line_no in 1..=101 {
                assert_eq!(
                    landed_row(93, 8, cursor_y, history_line_no),
                    Some(history_line_no as isize - 1),
                    "cursor_y {cursor_y}, line {history_line_no}"
                );
            }
        }
    }

    #[test]
    fn corrections_never_exceed_the_visible_screen() {
        for cursor_y in 0..8 {
            for history_line_no in 1..=101 {
                let plan = plan_from_bounds(93, 8, cursor_y, history_line_no).unwrap();
                assert!(
                    plan.residual.unsigned_abs() < 8,
                    "cursor_y {cursor_y}, line {history_line_no}: {}",
                    plan.residual
                );
            }
        }
    }

    #[test]
    fn panes_without_scrollback_stay_at_the_bottom() {
        for cursor_y in 0..8 {
            for history_line_no in 1..=8 {
                let plan = plan_from_bounds(0, 8, cursor_y, history_line_no).unwrap();
                assert_eq!(plan.scroll, 0);
                assert_eq!(
                    landed_row(0, 8, cursor_y, history_line_no),
                    Some(history_line_no as isize - 1)
                );
            }
        }
    }
}
