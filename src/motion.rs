mod capture;
mod hints;
mod matching;
mod navigation;
mod rendering;
mod terminal;
mod window;

use std::{
    collections::HashSet,
    io,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use capture::{init_panes, refresh_panes, window_size, MotionSnapshot};
use clap::{Args, ValueEnum};
use hints::{assign_hints_by_distance, hint_positions};
use matching::find_matches;
use navigation::move_to_match;
use rendering::{draw_all_hints, draw_all_panes, update_hints_display};
use serde::{Deserialize, Serialize};
use tempfile::Builder;
use terminal::{drain_pending_input, read_key, AnsiScreen, RawMode};

use crate::{config::Config, tmux, types::CaseMode, util::shell_quote};

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum MotionKind {
    S,
    S2,
}

impl MotionKind {
    pub fn pattern_len(self) -> usize {
        match self {
            Self::S => 1,
            Self::S2 => 2,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::S => "s",
            Self::S2 => "s2",
        }
    }
}

#[derive(Clone, Debug, Args)]
pub struct MotionArgs {
    #[arg(value_enum, default_value = "s")]
    pub kind: MotionKind,
    #[arg(value_name = "PATTERN")]
    pub pattern: Option<String>,
    #[arg(short = 'q', long = "query")]
    pub query: Option<String>,
    #[arg(long = "query-option", hide = true)]
    pub query_option: Option<String>,
    #[arg(long = "target-window", hide = true)]
    pub target_window: Option<String>,
    #[arg(long = "target-client", hide = true)]
    pub target_client: Option<String>,
    #[arg(long = "overlay", hide = true)]
    pub overlay: bool,
    #[arg(long = "snapshot", hide = true)]
    pub snapshot: Option<PathBuf>,
    #[arg(long, hide = true)]
    pub ready_title: Option<String>,
    #[arg(long = "case", value_enum)]
    pub case_mode: Option<CaseMode>,
    #[arg(long = "smartsign", conflicts_with = "no_smartsign")]
    pub smartsign: bool,
    #[arg(long = "no-smartsign", conflicts_with = "smartsign")]
    pub no_smartsign: bool,
}

pub fn run(args: MotionArgs, config: &Config) -> Result<()> {
    let mut motion_config = MotionConfig::from_config(config);
    if let Some(case_mode) = args.case_mode {
        motion_config.case_mode = case_mode;
    }
    if args.smartsign {
        motion_config.smartsign = true;
    }
    if args.no_smartsign {
        motion_config.smartsign = false;
    }

    let Some(pattern) = resolve_pattern(&args, args.kind.pattern_len()) else {
        return Ok(());
    };
    let target_window = resolve_target_window(args.target_window.as_deref());
    let target_client = resolve_target_client(args.target_client.as_deref());

    let panes = if let Some(path) = args.snapshot.as_deref() {
        refresh_panes(MotionSnapshot::load(path)?.panes)?
    } else {
        init_panes(target_window.as_deref())?
    };
    if panes.is_empty() {
        tmux::display_message("tmux-nexus motion: no visible panes");
        return Ok(());
    }

    let matches = find_matches(
        &panes,
        &pattern,
        motion_config.case_mode,
        motion_config.smartsign,
        motion_config.tab_mode,
    );
    if matches.is_empty() {
        tmux::display_message("tmux-nexus motion: no match");
        return Ok(());
    }
    if matches.len() == 1 {
        move_to_match(
            &panes,
            &matches[0],
            motion_config.tab_mode,
            target_client.as_deref(),
        )?;
        return Ok(());
    }
    if !args.overlay {
        let snapshot_file = Builder::new()
            .prefix("tnx_motion.")
            .suffix(".json")
            .tempfile()?;
        MotionSnapshot {
            panes: panes.clone(),
        }
        .save(snapshot_file.path())?;
        return run_popup(
            &args,
            &pattern,
            target_window.as_deref(),
            target_popup_pane(&panes).as_deref(),
            target_client.as_deref(),
            snapshot_file.path(),
        );
    }

    let current = panes
        .iter()
        .find(|pane| pane.active)
        .context("active pane not found")?;
    let cursor_y = current.start_y + current.cursor_y;
    let cursor_x = current.start_x + current.cursor_x;
    let hint_mapping =
        assign_hints_by_distance(&panes, &matches, cursor_y, cursor_x, &motion_config.hints);
    let positions = hint_positions(&panes, &hint_mapping, motion_config.tab_mode);

    if positions.is_empty() {
        tmux::display_message("tmux-nexus motion: no drawable hints");
        return Ok(());
    }

    let (window_width, window_height) = window_size(target_window.as_deref())?;
    let terminal_height = window_height;
    let max_x = panes
        .iter()
        .map(|pane| pane.start_x + pane.width)
        .max()
        .unwrap_or(window_width);
    let mut screen = AnsiScreen::new(&motion_config);
    screen.init()?;
    let result = run_overlay(
        &mut screen,
        Overlay {
            panes: &panes,
            positions: &positions,
            hint_mapping: &hint_mapping,
            max_x,
            terminal_height,
            target_client: target_client.as_deref(),
            config: &motion_config,
            ready_title: args.ready_title.as_deref(),
        },
    );
    screen.cleanup()?;
    result
}

fn resolve_pattern(args: &MotionArgs, len: usize) -> Option<String> {
    let raw = args
        .query
        .as_deref()
        .or(args.pattern.as_deref())
        .map(str::to_string)
        .or_else(|| {
            args.query_option
                .as_deref()
                .and_then(tmux::show_option)
                .inspect(|_| {
                    if let Some(option) = args.query_option.as_deref() {
                        tmux::run_ignore(["set-option", "-gu", option]);
                    }
                })
        })?;

    let pattern: String = raw
        .chars()
        .filter(|ch| *ch != '\n' && *ch != '\r')
        .take(len)
        .collect();
    (!pattern.is_empty()).then_some(pattern)
}

fn resolve_target_window(explicit: Option<&str>) -> Option<String> {
    explicit
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| tmux::try_stdout(["display-message", "-p", "#{window_id}"]))
        .filter(|value| !value.is_empty())
}

fn resolve_target_client(explicit: Option<&str>) -> Option<String> {
    explicit
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| tmux::try_stdout(["display-message", "-p", "#{client_name}"]))
        .filter(|value| !value.is_empty())
}

fn run_popup(
    args: &MotionArgs,
    pattern: &str,
    target_window: Option<&str>,
    target_pane: Option<&str>,
    target_client: Option<&str>,
    snapshot_path: &Path,
) -> Result<()> {
    let target_window = target_window.context("motion target window not found")?;
    let target_pane = target_pane.context("motion target pane not found")?;
    let exe = std::env::current_exe().context("failed to resolve current executable")?;
    // Older list-commands treats the argument as a glob and succeeds even
    // when nothing matches. Exit status alone is not a capability check.
    if tmux::try_stdout(["list-commands", "new-pane"])
        .is_some_and(|commands| commands.lines().any(|line| line.starts_with("new-pane ")))
    {
        return window::run(
            exe.to_string_lossy().as_ref(),
            args,
            pattern,
            target_window,
            target_pane,
            target_client,
            snapshot_path,
        );
    }
    tmux::run(popup_command(
        exe.to_string_lossy().as_ref(),
        args,
        pattern,
        target_window,
        target_pane,
        target_client,
        snapshot_path,
    ))
}

fn target_popup_pane(panes: &[Pane]) -> Option<String> {
    panes
        .iter()
        .find(|pane| pane.active)
        .or_else(|| panes.first())
        .map(|pane| pane.pane_id.clone())
}

fn popup_command(
    exe: &str,
    args: &MotionArgs,
    pattern: &str,
    target_window: &str,
    target_pane: &str,
    target_client: Option<&str>,
    snapshot_path: &Path,
) -> Vec<String> {
    let command = overlay_args(
        exe,
        args,
        pattern,
        target_window,
        target_client,
        snapshot_path,
    );
    let mut popup = vec![
        "display-popup".to_string(),
        "-E".to_string(),
        "-B".to_string(),
        "-w".to_string(),
        "100%".to_string(),
        "-h".to_string(),
        "100%".to_string(),
        "-t".to_string(),
        target_pane.to_string(),
    ];
    if let Some(target_client) = target_client {
        popup.push("-c".to_string());
        popup.push(target_client.to_string());
    }
    popup.push(
        command
            .iter()
            .map(|arg| shell_quote(arg))
            .collect::<Vec<_>>()
            .join(" "),
    );
    popup
}

fn overlay_args(
    exe: &str,
    args: &MotionArgs,
    pattern: &str,
    target_window: &str,
    target_client: Option<&str>,
    snapshot_path: &Path,
) -> Vec<String> {
    let mut command = vec![
        exe.to_string(),
        "motion".into(),
        args.kind.as_str().into(),
        "--query".into(),
        pattern.to_string(),
        "--target-window".into(),
        target_window.to_string(),
        "--overlay".into(),
        "--snapshot".into(),
        snapshot_path.to_string_lossy().into_owned(),
    ];
    if let Some(target_client) = target_client {
        command.push("--target-client".into());
        command.push(target_client.to_string());
    }
    if let Some(case_mode) = args.case_mode {
        command.push("--case".into());
        command.push(case_mode.to_string());
    }
    if args.smartsign {
        command.push("--smartsign".into());
    }
    if args.no_smartsign {
        command.push("--no-smartsign".into());
    }

    command
}

#[derive(Clone, Debug)]
struct MotionConfig {
    hints: String,
    case_mode: CaseMode,
    smartsign: bool,
    tab_mode: TabMode,
    vertical_border: String,
    horizontal_border: String,
    hint1_fg: String,
    hint2_fg: String,
    dim: String,
}

impl MotionConfig {
    fn from_config(config: &Config) -> Self {
        Self {
            hints: config.motion_hints.clone(),
            case_mode: config.motion_case_mode,
            smartsign: config.motion_smartsign,
            tab_mode: detect_tmux_tab_mode(),
            vertical_border: config.motion_vertical_border.clone(),
            horizontal_border: config.motion_horizontal_border.clone(),
            hint1_fg: config.motion_hint1_fg.clone(),
            hint2_fg: config.motion_hint2_fg.clone(),
            dim: config.motion_dim.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TabMode {
    Fixed,
    PositionAware,
}

fn detect_tmux_tab_mode() -> TabMode {
    tmux::command_version("tmux", &["-V"])
        .as_deref()
        .map(tmux_tab_mode_from_version)
        .unwrap_or(TabMode::Fixed)
}

fn tmux_tab_mode_from_version(version: &str) -> TabMode {
    let lower = version.to_ascii_lowercase();
    if lower.contains("master") || lower.contains("openbsd-") {
        return TabMode::Fixed;
    }

    parse_tmux_major_minor(version)
        .filter(|version| *version >= (3, 6))
        .map(|_| TabMode::PositionAware)
        .unwrap_or(TabMode::Fixed)
}

fn parse_tmux_major_minor(version: &str) -> Option<(usize, usize)> {
    let bytes = version.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() {
        if !bytes[idx].is_ascii_digit() {
            idx += 1;
            continue;
        }

        let major_start = idx;
        while idx < bytes.len() && bytes[idx].is_ascii_digit() {
            idx += 1;
        }
        if idx >= bytes.len() || bytes[idx] != b'.' {
            continue;
        }
        let major = version[major_start..idx].parse().ok()?;
        idx += 1;
        let minor_start = idx;
        while idx < bytes.len() && bytes[idx].is_ascii_digit() {
            idx += 1;
        }
        if minor_start == idx {
            continue;
        }
        let minor = version[minor_start..idx].parse().ok()?;
        return Some((major, minor));
    }
    None
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Pane {
    window_id: String,
    pane_id: String,
    active: bool,
    start_y: usize,
    height: usize,
    start_x: usize,
    width: usize,
    copy_mode: bool,
    scroll_position: isize,
    cursor_y: usize,
    cursor_x: usize,
    lines: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Match {
    pane_index: usize,
    line_no: usize,
    visual_col: usize,
}

#[derive(Clone, Debug)]
struct HintTarget {
    hint: String,
    target: Match,
}

#[derive(Clone, Debug)]
struct HintPosition {
    screen_y: usize,
    screen_x: usize,
    original: String,
    hint: String,
}

struct Overlay<'a> {
    panes: &'a [Pane],
    positions: &'a [HintPosition],
    hint_mapping: &'a [HintTarget],
    max_x: usize,
    terminal_height: usize,
    target_client: Option<&'a str>,
    config: &'a MotionConfig,
    ready_title: Option<&'a str>,
}

fn run_overlay(screen: &mut AnsiScreen, overlay: Overlay<'_>) -> Result<()> {
    let _raw = RawMode::new()?;
    draw_all_panes(
        screen,
        overlay.panes,
        overlay.max_x,
        overlay.terminal_height,
        overlay.config,
    )?;
    draw_all_hints(screen, overlay.positions, overlay.terminal_height)?;
    if let Some(title) = overlay.ready_title {
        screen.set_title(title)?;
    }
    screen.refresh()?;

    let hints_chars: HashSet<char> = overlay.config.hints.chars().collect();
    let max_hint_len = overlay
        .hint_mapping
        .iter()
        .map(|target| target.hint.chars().count())
        .max()
        .unwrap_or_default();
    let mut key_sequence = String::new();
    let mut stdin = io::stdin();
    loop {
        let ch = read_key(&mut stdin)?;
        if ch == '\u{1b}' {
            drain_pending_input(stdin.as_raw_fd(), 30);
            return Ok(());
        }
        if !hints_chars.contains(&ch) {
            return Ok(());
        }

        key_sequence.push(ch);
        if let Some(target) = overlay
            .hint_mapping
            .iter()
            .find(|target| target.hint == key_sequence)
            .map(|target| &target.target)
        {
            move_to_match(
                overlay.panes,
                target,
                overlay.config.tab_mode,
                overlay.target_client,
            )?;
            return Ok(());
        }
        if key_sequence.chars().count() >= max_hint_len {
            return Ok(());
        }
        update_hints_display(screen, overlay.positions, &key_sequence)?;
    }
}

#[cfg(test)]
mod tests;
