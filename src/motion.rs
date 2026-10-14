mod capture;
mod hints;
mod matching;
mod navigation;
mod rendering;
mod terminal;

use std::{
    collections::HashSet,
    fs, io,
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
    if tmux::try_stdout(["list-commands", "new-pane"]).is_some() {
        return run_overlay_window(
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

fn run_overlay_window(
    exe: &str,
    args: &MotionArgs,
    pattern: &str,
    target_window: &str,
    target_pane: &str,
    target_client: Option<&str>,
    snapshot_path: &Path,
) -> Result<()> {
    // tmux versions with floating panes unzoom/rezoom when a popup is
    // created or removed. A separate window leaves the source grid intact.
    let session = tmux::stdout(["display-message", "-p", "-t", target_pane, "#{session_id}"])?;
    let (width, height) = window_size(Some(target_window))?;
    let status_file = Builder::new().prefix("tnx_motion_status.").tempfile()?;
    let channel = format!("tnx-motion-{}", status_file.path().display());
    let start_channel = format!("{channel}-start");
    let done_channel = format!("{channel}-done");
    let mut tmux_command = vec!["tmux".to_string()];
    if let Ok(value) = std::env::var("TNX_TMUX_ARGS") {
        tmux_command.extend(shell_words::split(&value).context("failed to parse TNX_TMUX_ARGS")?);
    }
    let tmux_command = tmux_command
        .iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ");
    let overlay = overlay_command(
        exe,
        args,
        pattern,
        target_window,
        target_client,
        snapshot_path,
    );
    let shell_command = format!(
        "{tmux_command} wait-for {}; {overlay}; motion_status=$?; \
         printf '%s' \"$motion_status\" > {}; {tmux_command} wait-for -S {}",
        shell_quote(&start_channel),
        shell_quote(&status_file.path().to_string_lossy()),
        shell_quote(&done_channel),
    );
    let mut command = vec![
        "new-window".to_string(),
        "-d".into(),
        "-P".into(),
        "-F".into(),
        "#{window_id}".into(),
        "-t".into(),
        format!("{}:", session.trim()),
        "-n".into(),
        "motion".into(),
    ];
    if let Ok(value) = std::env::var("TNX_TMUX_ARGS") {
        command.extend(["-e".into(), format!("TNX_TMUX_ARGS={value}")]);
    }
    command.push(shell_command);
    let overlay_window = tmux::stdout(command)?.trim().to_string();
    let client = tmux::TmuxClient::from_env()?;
    let result = (|| {
        client.run_commands(&[
            strings(&[
                "set-option",
                "-w",
                "-t",
                &overlay_window,
                "remain-on-exit",
                "on",
            ]),
            strings(&[
                "set-option",
                "-w",
                "-t",
                &overlay_window,
                "window-size",
                "manual",
            ]),
            strings(&[
                "set-option",
                "-w",
                "-t",
                &overlay_window,
                "pane-border-status",
                "off",
            ]),
            strings(&[
                "resize-window",
                "-t",
                &overlay_window,
                "-x",
                &width.to_string(),
                "-y",
                &height.to_string(),
            ]),
            select_overlay_window(&overlay_window, target_client),
            strings(&["wait-for", "-S", &start_channel]),
            strings(&["wait-for", &done_channel]),
        ])?;
        let status = fs::read_to_string(status_file.path())?;
        anyhow::ensure!(
            status == "0",
            "motion overlay failed (exit status {status})"
        );
        Ok(())
    })();
    // Keep the overlay window until we have returned to the source; otherwise
    // tmux may select an unrelated window when the overlay process exits.
    let restore = client.run(select_overlay_window(target_window, target_client));
    let cleanup = client.run(["kill-window", "-t", &overlay_window]);
    result.and(restore).and(cleanup)
}

fn strings(args: &[&str]) -> Vec<std::ffi::OsString> {
    args.iter().map(std::ffi::OsString::from).collect()
}

fn select_overlay_window(window: &str, client: Option<&str>) -> Vec<std::ffi::OsString> {
    if let Some(client) = client {
        strings(&["switch-client", "-c", client, "-t", window])
    } else {
        strings(&["select-window", "-t", window])
    }
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
    let command = overlay_command(
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
    popup.push(command);
    popup
}

fn overlay_command(
    exe: &str,
    args: &MotionArgs,
    pattern: &str,
    target_window: &str,
    target_client: Option<&str>,
    snapshot_path: &Path,
) -> String {
    let mut command = vec![
        shell_quote(exe),
        "motion".into(),
        args.kind.as_str().into(),
        "--query".into(),
        shell_quote(pattern),
        "--target-window".into(),
        shell_quote(target_window),
        "--overlay".into(),
        "--snapshot".into(),
        shell_quote(&snapshot_path.to_string_lossy()),
    ];
    if let Some(target_client) = target_client {
        command.push("--target-client".into());
        command.push(shell_quote(target_client));
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

    command.join(" ")
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
}

fn run_overlay(screen: &mut AnsiScreen, overlay: Overlay<'_>) -> Result<()> {
    draw_all_panes(
        screen,
        overlay.panes,
        overlay.max_x,
        overlay.terminal_height,
        overlay.config,
    )?;
    draw_all_hints(screen, overlay.positions, overlay.terminal_height)?;

    let hints_chars: HashSet<char> = overlay.config.hints.chars().collect();
    let max_hint_len = overlay
        .hint_mapping
        .iter()
        .map(|target| target.hint.chars().count())
        .max()
        .unwrap_or_default();
    let mut key_sequence = String::new();
    let _raw = RawMode::new()?;
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
