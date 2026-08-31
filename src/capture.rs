use std::io::Write;

use anyhow::{Context, Result};
use rayon::prelude::*;

use crate::{
    config::Config,
    index::{PaneSnapshot, Record, SearchIndex, INDEX_VERSION},
    tmux,
    types::Scope,
};

#[derive(Clone, Debug)]
struct PaneInfo {
    session: String,
    window_index: String,
    pane_index: String,
    pane_id: String,
    command: String,
    window_name: String,
    history_size: usize,
}

#[derive(Debug)]
struct CapturedLine {
    text: String,
    grid_line_index: Option<usize>,
}

#[derive(Debug)]
struct CapturedPane {
    snapshot: PaneSnapshot,
    grid_line_indices: Vec<Option<usize>>,
}

pub fn build_index(config: &Config, target_pane: Option<&str>) -> Result<SearchIndex> {
    let panes = list_panes(config.scope, target_pane)?;
    let captured: Vec<CapturedPane> = panes
        .par_iter()
        .map(|pane| {
            capture_pane(pane, config)
                .with_context(|| format!("failed to capture pane {}", pane.pane_id))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut records = Vec::new();
    for (pane_index, pane) in captured.iter().enumerate() {
        for (line_index, line) in pane.snapshot.lines.iter().enumerate() {
            if config.skip_blank && line.trim().is_empty() {
                continue;
            }

            records.push(Record {
                pane_index,
                line_index,
                grid_line_index: pane.grid_line_indices[line_index],
            });
        }
    }

    let snapshots = captured.into_iter().map(|pane| pane.snapshot).collect();

    Ok(SearchIndex {
        version: INDEX_VERSION,
        panes: snapshots,
        records,
    })
}

pub fn write_legacy_tsv(
    config: &Config,
    target_pane: Option<&str>,
    writer: &mut impl Write,
) -> Result<()> {
    let panes = list_panes(config.scope, target_pane)?;
    for pane in &panes {
        let lines = capture_pane_lines(pane, config)
            .with_context(|| format!("failed to capture pane {}", pane.pane_id))?;
        write_legacy_pane(writer, pane, config, &lines)?;
    }
    Ok(())
}

fn list_panes(scope: Scope, target_pane: Option<&str>) -> Result<Vec<PaneInfo>> {
    let fmt = "#{session_name}\t#{window_index}\t#{pane_index}\t#{pane_id}\t#{pane_current_command}\t#{history_size}\t#{window_name}";
    let output = match scope {
        Scope::All => tmux::stdout(["list-panes", "-a", "-F", fmt])?,
        Scope::Session => tmux::stdout(["list-panes", "-s", "-F", fmt])?,
        Scope::Pane => {
            let pane = target_pane
                .map(ToOwned::to_owned)
                .or_else(|| std::env::var("TMUX_PANE").ok())
                .or_else(|| tmux::try_stdout(["display-message", "-p", "#{pane_id}"]));
            let Some(pane) = pane else {
                return Ok(Vec::new());
            };
            let all = tmux::stdout(["list-panes", "-a", "-F", fmt])?;
            return Ok(parse_panes(&all)
                .into_iter()
                .filter(|info| info.pane_id == pane)
                .collect());
        }
    };

    let panes = parse_panes(&output)
        .into_iter()
        .filter(|info| target_pane.is_none_or(|target| info.pane_id == target))
        .collect();
    Ok(panes)
}

fn parse_panes(output: &str) -> Vec<PaneInfo> {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(7, '\t');
            Some(PaneInfo {
                session: parts.next()?.to_string(),
                window_index: parts.next()?.to_string(),
                pane_index: parts.next()?.to_string(),
                pane_id: parts.next()?.to_string(),
                command: parts.next().unwrap_or_default().to_string(),
                history_size: parts.next().unwrap_or_default().parse().unwrap_or_default(),
                window_name: parts.next().unwrap_or_default().to_string(),
            })
        })
        .filter(|pane| !pane.pane_id.is_empty())
        .collect()
}

fn write_legacy_pane(
    writer: &mut impl Write,
    pane: &PaneInfo,
    config: &Config,
    lines: &[CapturedLine],
) -> Result<()> {
    let location = pane_location(pane);
    let line_offset = history_start_line(pane, config);

    for (line_index, line) in lines.iter().enumerate() {
        if config.skip_blank && line.text.trim().is_empty() {
            continue;
        }
        let raw_line_index = line.grid_line_index.unwrap_or(line_index);

        writeln!(
            writer,
            "{}\t{}\t{}\t{}\t{}\t{}",
            pane.pane_id,
            location,
            pane.command,
            pane.window_name,
            line_offset + raw_line_index + 1,
            line.text
        )?;
    }
    Ok(())
}

fn capture_pane(pane: &PaneInfo, config: &Config) -> Result<CapturedPane> {
    let captured = capture_pane_lines(pane, config)?;
    let grid_line_indices = captured.iter().map(|line| line.grid_line_index).collect();
    let lines = captured.into_iter().map(|line| line.text).collect();

    Ok(CapturedPane {
        snapshot: PaneSnapshot {
            location: pane_location(pane),
            pane_id: pane.pane_id.clone(),
            command: pane.command.clone(),
            window_name: pane.window_name.clone(),
            history_start_line: history_start_line(pane, config),
            lines,
        },
        grid_line_indices,
    })
}

fn capture_pane_lines(pane: &PaneInfo, config: &Config) -> Result<Vec<CapturedLine>> {
    if config.join_wraps && tmux::capture_flags_supported() {
        if let Ok(output) = capture_pane_output(pane, config, false, true) {
            if let Some(lines) = parse_flagged_capture(&output) {
                return Ok(lines);
            }
        }
    }

    let output = capture_pane_output(pane, config, config.join_wraps, false)?;
    Ok(output
        .lines()
        .enumerate()
        .map(|(line_index, line)| CapturedLine {
            text: line.trim_end_matches('\r').to_string(),
            grid_line_index: (!config.join_wraps).then_some(line_index),
        })
        .collect())
}

fn parse_flagged_capture(output: &str) -> Option<Vec<CapturedLine>> {
    let mut lines = Vec::new();
    let mut pending: Option<CapturedLine> = None;

    for (grid_line_index, line) in output.lines().enumerate() {
        let (flags, text) = line.trim_end_matches('\r').split_once(' ')?;
        if flags.is_empty()
            || !flags
                .chars()
                .all(|flag| flag == '-' || flag.is_ascii_uppercase())
        {
            return None;
        }

        let current = pending.get_or_insert_with(|| CapturedLine {
            text: String::new(),
            grid_line_index: Some(grid_line_index),
        });
        current.text.push_str(text);
        if !flags.contains('W') {
            lines.push(pending.take().expect("pending capture line exists"));
        }
    }
    if let Some(line) = pending {
        lines.push(line);
    }
    Some(lines)
}

fn capture_pane_output(
    pane: &PaneInfo,
    config: &Config,
    join_wraps: bool,
    include_flags: bool,
) -> Result<String> {
    let mut args = vec!["capture-pane".to_string(), "-p".to_string()];
    if join_wraps {
        args.push("-J".to_string());
    }
    if include_flags {
        args.push("-F".to_string());
    }
    if config.include_history {
        args.extend([
            "-S".to_string(),
            history_start(config),
            "-E".to_string(),
            "-".to_string(),
        ]);
    }
    args.extend(["-t".to_string(), pane.pane_id.clone()]);

    tmux::stdout(args)
}

fn pane_location(pane: &PaneInfo) -> String {
    format!("{}:{}.{}", pane.session, pane.window_index, pane.pane_index)
}

fn history_start(config: &Config) -> String {
    config
        .history_lines
        .map(|lines| format!("-{lines}"))
        .unwrap_or_else(|| "-".to_string())
}

fn history_start_line(pane: &PaneInfo, config: &Config) -> usize {
    if !config.include_history {
        return pane.history_size;
    }
    config
        .history_lines
        .map(|lines| pane.history_size.saturating_sub(lines))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        history_start, history_start_line, parse_flagged_capture, write_legacy_pane, CapturedLine,
        PaneInfo,
    };
    use crate::{
        config::Config,
        types::{ActionKind, CaseMode, Scope, SearchMode},
    };

    fn config(history_lines: Option<usize>) -> Config {
        Config {
            launch_key: "g".into(),
            motion_key: "s".into(),
            motion2_key: String::new(),
            motion_copy_mode_no_prefix: false,
            scope: Scope::All,
            include_history: true,
            history_lines,
            case_mode: CaseMode::Smart,
            join_wraps: true,
            skip_blank: true,
            preview: true,
            prompt_query: false,
            default_action: ActionKind::Jump,
            fzf_options: String::new(),
            search_mode: SearchMode::Literal,
            motion_hints: "asdghklqwertyuiopzxcvbnmfj;".into(),
            motion_case_mode: CaseMode::Insensitive,
            motion_smartsign: false,
            motion_vertical_border: "|".into(),
            motion_horizontal_border: "-".into(),
            motion_hint1_fg: "1;31".into(),
            motion_hint2_fg: "1;32".into(),
            motion_dim: "2".into(),
        }
    }

    fn pane(history_size: usize) -> PaneInfo {
        PaneInfo {
            session: "s".into(),
            window_index: "1".into(),
            pane_index: "0".into(),
            pane_id: "%1".into(),
            command: "zsh".into(),
            window_name: "main".into(),
            history_size,
        }
    }

    #[test]
    fn unlimited_history_starts_at_top() {
        assert_eq!(history_start(&config(None)), "-");
    }

    #[test]
    fn limited_history_uses_negative_line_count() {
        assert_eq!(history_start(&config(Some(5000))), "-5000");
    }

    #[test]
    fn unlimited_history_has_no_line_offset() {
        assert_eq!(history_start_line(&pane(10_000), &config(None)), 0);
    }

    #[test]
    fn limited_history_tracks_omitted_scrollback_lines() {
        assert_eq!(history_start_line(&pane(10_000), &config(Some(5000))), 5000);
    }

    #[test]
    fn limited_history_offset_saturates_for_short_history() {
        assert_eq!(history_start_line(&pane(100), &config(Some(5000))), 0);
    }

    #[test]
    fn visible_only_capture_starts_after_scrollback() {
        let mut config = config(Some(5000));
        config.include_history = false;
        assert_eq!(history_start_line(&pane(10_000), &config), 10_000);
    }

    #[test]
    fn legacy_tsv_keeps_raw_line_numbers_when_skipping_blanks() {
        let mut output = Vec::new();
        let lines = [
            CapturedLine {
                text: "alpha".into(),
                grid_line_index: Some(0),
            },
            CapturedLine {
                text: String::new(),
                grid_line_index: Some(1),
            },
            CapturedLine {
                text: "beta".into(),
                grid_line_index: Some(2),
            },
        ];
        write_legacy_pane(&mut output, &pane(1300), &config(Some(100)), &lines).unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "%1\ts:1.0\tzsh\tmain\t1201\talpha\n%1\ts:1.0\tzsh\tmain\t1203\tbeta\n"
        );
    }

    #[test]
    fn flagged_capture_joins_text_but_preserves_grid_coordinates() {
        let lines = parse_flagged_capture(
            "- before\nW 012345678901234567890123456789\n- -IN-A-NARROW-PANE\n- after\n",
        )
        .unwrap();

        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[1].text,
            "012345678901234567890123456789-IN-A-NARROW-PANE"
        );
        assert_eq!(lines[1].grid_line_index, Some(1));
        assert_eq!(lines[2].text, "after");
        assert_eq!(lines[2].grid_line_index, Some(3));
    }
}
