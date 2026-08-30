use std::{
    collections::HashMap,
    env,
    ffi::{OsStr, OsString},
    fs::File,
    io::{ErrorKind, Write},
    path::PathBuf,
    process::{Command, Stdio},
    str::FromStr,
};

use anyhow::{Context, Result};
use clap::Args;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{
    tmux,
    util::{shell_quote, version_at_least},
};

mod clipboard;
mod process;
mod workspace;

const CATEGORIES: &[(&str, &str)] = &[
    ("history", "Search pane history"),
    ("copy-mode", "Run a copy-mode command"),
    ("session", "Manage sessions"),
    ("window", "Manage windows"),
    ("pane", "Manage panes"),
    ("command", "Insert a tmux command"),
    ("keybinding", "Run a key binding"),
    ("clipboard", "Paste clipboard history"),
    ("process", "Inspect or signal processes"),
    ("menu", "Run a configured command"),
];

#[derive(Clone, Debug, Default, Args)]
pub struct ManageArgs {
    #[arg(value_name = "CATEGORY")]
    category: Option<String>,
    #[arg(value_name = "ACTION")]
    action: Option<String>,
}

#[derive(Clone, Debug, Args)]
pub struct PreviewArgs {
    #[arg(long)]
    kind: String,
    #[arg(long)]
    data: PathBuf,
    #[arg(long)]
    row: Option<usize>,
}

#[derive(Clone, Debug)]
struct ManagerConfig {
    key: String,
    order: Vec<String>,
    fzf_options: String,
    preview: bool,
    preview_follow: bool,
    confirm: bool,
    switch_current: bool,
    session_format: Option<String>,
    window_format: Option<String>,
    pane_format: Option<String>,
    pane_remote: PaneRemoteMode,
    window_filter: Option<String>,
    menu: Option<String>,
    menu_popup: bool,
    menu_popup_width: String,
    menu_popup_height: String,
}

#[derive(Debug)]
struct ManagerContext {
    config: ManagerConfig,
    tmux: tmux::TmuxClient,
    picker: Picker,
}

#[derive(Clone, Debug)]
struct Picker {
    program: &'static str,
    options: Vec<OsString>,
    preview_follow: bool,
    tmux_popup: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PickerExit {
    Selected,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Row {
    id: String,
    display: String,
}

#[derive(Clone, Debug)]
struct SessionEntry {
    row: Row,
    attached_clients: usize,
}

#[derive(Clone, Debug)]
struct WindowEntry {
    row: Row,
    session_id: String,
    window_id: String,
}

#[derive(Clone, Debug)]
struct PaneEntry {
    row: Row,
    session_id: String,
    window_id: String,
    pane_id: String,
    pane_path: String,
    host: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum PaneRemoteMode {
    #[default]
    Auto,
    Always,
    Off,
}

impl FromStr for PaneRemoteMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "always" => Ok(Self::Always),
            "off" => Ok(Self::Off),
            _ => Err("expected auto, always, or off".into()),
        }
    }
}

impl PaneRemoteMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Always => "always",
            Self::Off => "off",
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum PreviewSpec<'a> {
    Rows(&'a str),
}

pub fn run(args: ManageArgs) -> Result<()> {
    ensure_program("tmux", "tmux is required for manage")?;
    if args.category.as_deref() == Some("history") {
        return run_history();
    }
    ensure_program("fzf", "fzf is required for manage")?;
    let config = ManagerConfig::load()?;
    let context = ManagerContext {
        picker: Picker::new(&config)?,
        tmux: tmux::TmuxClient::from_env()?,
        config,
    };
    let category = match args.category {
        Some(category) => category,
        None => select_category(&context)?,
    };
    if category.is_empty() {
        return Ok(());
    }
    match category.as_str() {
        "history" => run_history(),
        "copy-mode" => run_copy_mode(args.action.as_deref(), &context),
        "session" => workspace::run_session(args.action.as_deref(), &context),
        "window" => workspace::run_window(args.action.as_deref(), &context),
        "pane" => workspace::run_pane(args.action.as_deref(), &context),
        "command" => run_command(&context),
        "keybinding" => run_keybinding(&context),
        "clipboard" => clipboard::run(args.action.as_deref(), &context),
        "process" => process::run(args.action.as_deref(), &context),
        "menu" => run_menu(&context),
        other => anyhow::bail!("unknown manager category '{other}'"),
    }
}

pub fn preview(args: PreviewArgs) -> Result<()> {
    let rows: Vec<Row> = serde_json::from_reader(
        File::open(&args.data)
            .with_context(|| format!("failed to open preview data {}", args.data.display()))?,
    )?;
    let row = rows
        .get(args.row.context("row preview requires --row")?)
        .context("preview row is out of range")?;
    match args.kind.as_str() {
        "session" => print_tmux(["capture-pane", "-ep", "-t", &format!("{}:", row.id)]),
        "window" | "pane" => print_tmux(["capture-pane", "-ep", "-t", row.id.as_str()]),
        "buffer" => print_tmux(["show-buffer", "-b", row.id.as_str()]),
        "copyq" => clipboard::print_copyq_preview(&row.id),
        _ => Ok(()),
    }
}

impl ManagerConfig {
    fn load() -> Result<Self> {
        let options: HashMap<String, String> = tmux::show_options("@tmux_nexus_manager_")?
            .into_iter()
            .filter_map(|(name, value)| {
                name.strip_prefix("@tmux_nexus_manager_")
                    .map(|key| (key.to_string(), value))
            })
            .collect();
        let get = |name: &str, env_name: &str, legacy: &str| {
            env::var(env_name)
                .ok()
                .filter(|value| !value.is_empty())
                .or_else(|| options.get(name).cloned())
                .or_else(|| env::var(legacy).ok().filter(|value| !value.is_empty()))
        };
        let boolean = |name: &str, env_name: &str, legacy: &str, default: bool| -> Result<bool> {
            match get(name, env_name, legacy) {
                None => Ok(default),
                Some(value) => parse_bool(&value)
                    .with_context(|| format!("invalid manager setting {env_name}/manager_{name}")),
            }
        };
        let mut order: Vec<String> = get("order", "TNX_MANAGER_ORDER", "TMUX_FZF_ORDER")
            .unwrap_or_else(|| {
                "history|copy-mode|session|window|pane|command|keybinding|clipboard|process".into()
            })
            .split('|')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        let menu = get("menu", "TNX_MANAGER_MENU", "TMUX_FZF_MENU");
        let key = env::var("TNX_MANAGER_KEY")
            .ok()
            .or_else(|| tmux::show_option_allow_empty("@tmux_nexus_manager_key"))
            .or_else(|| env::var("TMUX_FZF_LAUNCH_KEY").ok())
            .unwrap_or_else(|| "F".into());
        if menu.is_some() && !order.iter().any(|item| item == "menu") {
            order.push("menu".into());
        }
        Ok(Self {
            key,
            order,
            fzf_options: get("fzf_options", "TNX_MANAGER_FZF_OPTIONS", "TMUX_FZF_OPTIONS")
                .unwrap_or_else(|| "-p -w 62% -h 38%".into()),
            preview: boolean("preview", "TNX_MANAGER_PREVIEW", "TMUX_FZF_PREVIEW", true)?,
            preview_follow: boolean(
                "preview_follow",
                "TNX_MANAGER_PREVIEW_FOLLOW",
                "TMUX_FZF_PREVIEW_FOLLOW",
                true,
            )?,
            confirm: boolean("confirm", "TNX_MANAGER_CONFIRM", "", true)?,
            switch_current: boolean(
                "switch_current",
                "TNX_MANAGER_SWITCH_CURRENT",
                "TMUX_FZF_SWITCH_CURRENT",
                false,
            )?,
            session_format: get(
                "session_format",
                "TNX_MANAGER_SESSION_FORMAT",
                "TMUX_FZF_SESSION_FORMAT",
            ),
            window_format: get(
                "window_format",
                "TNX_MANAGER_WINDOW_FORMAT",
                "TMUX_FZF_WINDOW_FORMAT",
            ),
            pane_format: get(
                "pane_format",
                "TNX_MANAGER_PANE_FORMAT",
                "TMUX_FZF_PANE_FORMAT",
            ),
            pane_remote: get("pane_remote", "TNX_MANAGER_PANE_REMOTE", "")
                .map(|value| {
                    value.parse().map_err(|error: String| {
                        anyhow::anyhow!(
                            "invalid manager setting TNX_MANAGER_PANE_REMOTE/manager_pane_remote: {error}"
                        )
                    })
                })
                .transpose()?
                .unwrap_or_default(),
            window_filter: get(
                "window_filter",
                "TNX_MANAGER_WINDOW_FILTER",
                "TMUX_FZF_WINDOW_FILTER",
            ),
            menu,
            menu_popup: boolean(
                "menu_popup",
                "TNX_MANAGER_MENU_POPUP",
                "TMUX_FZF_MENU_POPUP",
                false,
            )?,
            menu_popup_width: get(
                "menu_popup_width",
                "TNX_MANAGER_MENU_POPUP_WIDTH",
                "TMUX_FZF_MENU_POPUP_WIDTH",
            )
            .unwrap_or_else(|| "50%".into()),
            menu_popup_height: get(
                "menu_popup_height",
                "TNX_MANAGER_MENU_POPUP_HEIGHT",
                "TMUX_FZF_MENU_POPUP_HEIGHT",
            )
            .unwrap_or_else(|| "50%".into()),
        })
    }
}

impl Picker {
    fn new(config: &ManagerConfig) -> Result<Self> {
        let use_tmux = tmux::have("fzf-tmux") && env::var_os("TMUX").is_some();
        let fzf_version = tmux::command_version("fzf", &["--version"]).unwrap_or_default();
        let tmux_version = tmux::command_version("tmux", &["-V"]).unwrap_or_default();
        let tmux_popup = version_at_least(&tmux_version, 3, 2, 0);
        let parsed = shell_words::split(&config.fzf_options)
            .context("failed to parse manager fzf options")?;
        let options = if use_tmux {
            let options = without_multi(parsed);
            if tmux_popup && version_at_least(&fzf_version, 0, 23, 0) {
                options
            } else {
                without_popup_options(options)
            }
        } else {
            without_tmux_options(without_multi(parsed))
        };
        Ok(Self {
            program: if use_tmux { "fzf-tmux" } else { "fzf" },
            options: options.into_iter().map(OsString::from).collect(),
            preview_follow: config.preview_follow && version_at_least(&fzf_version, 0, 24, 4),
            tmux_popup,
        })
    }

    fn command(&self) -> Command {
        let mut command = Command::new(self.program);
        command.args(&self.options);
        command
    }
}

fn picker_exit(code: Option<i32>) -> PickerExit {
    match code {
        Some(0) => PickerExit::Selected,
        Some(1) | Some(130) => PickerExit::Cancelled,
        _ => PickerExit::Failed,
    }
}

fn select_category(context: &ManagerContext) -> Result<String> {
    let config = &context.config;
    let in_copy_mode =
        tmux::try_stdout(["display-message", "-p", "#{pane_in_mode}"]).as_deref() == Some("1");
    let rows: Vec<Row> = config
        .order
        .iter()
        .filter(|category| category.as_str() != "copy-mode" || in_copy_mode)
        .filter(|category| category.as_str() != "menu" || config.menu.is_some())
        .filter_map(|category| {
            CATEGORIES
                .iter()
                .find(|(name, _)| name == category)
                .map(|(name, description)| Row::new(*name, format!("{name:<12} {description}")))
        })
        .collect();
    Ok(
        choose(&rows, context, "manage> ", "Select a feature", false, None)?
            .first()
            .map(|index| rows[*index].id.clone())
            .unwrap_or_default(),
    )
}

fn run_history() -> Result<()> {
    let status = Command::new(env::current_exe()?).arg("search").status()?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("history search exited with status {status}")
    }
}

fn run_copy_mode(action: Option<&str>, context: &ManagerContext) -> Result<()> {
    const COMMANDS: &[(&str, &str, bool)] = &[
        ("append-selection", "Append selection to clipboard", false),
        (
            "append-selection-and-cancel",
            "Append selection and cancel",
            false,
        ),
        ("back-to-indentation", "Move to indentation", false),
        ("begin-selection", "Begin selection", false),
        ("bottom-line", "Move to bottom line", false),
        ("cancel", "Cancel copy mode", false),
        ("clear-selection", "Clear selection", false),
        ("copy-end-of-line", "Copy to end of line", false),
        (
            "copy-end-of-line-and-cancel",
            "Copy to end and cancel",
            false,
        ),
        (
            "copy-pipe-end-of-line",
            "Copy to end through a shell command",
            true,
        ),
        ("copy-line", "Copy line", false),
        ("copy-line-and-cancel", "Copy line and cancel", false),
        ("copy-selection", "Copy selection", false),
        (
            "copy-selection-and-cancel",
            "Copy selection and cancel",
            false,
        ),
        ("cursor-down", "Move cursor down", false),
        ("cursor-left", "Move cursor left", false),
        ("cursor-right", "Move cursor right", false),
        ("cursor-up", "Move cursor up", false),
        ("end-of-line", "Move to end of line", false),
        ("goto-line", "Go to line", true),
        ("history-bottom", "Scroll to history bottom", false),
        ("history-top", "Scroll to history top", false),
        ("jump-again", "Repeat last jump", false),
        ("jump-backward", "Jump backward to text", true),
        ("jump-forward", "Jump forward to text", true),
        ("jump-to-mark", "Jump to mark", false),
        ("middle-line", "Move to middle line", false),
        ("next-matching-bracket", "Next matching bracket", false),
        ("next-paragraph", "Next paragraph", false),
        ("next-word", "Next word", false),
        ("page-down", "Page down", false),
        ("page-up", "Page up", false),
        (
            "previous-matching-bracket",
            "Previous matching bracket",
            false,
        ),
        ("previous-paragraph", "Previous paragraph", false),
        ("previous-word", "Previous word", false),
        ("rectangle-toggle", "Toggle rectangle selection", false),
        ("refresh-from-pane", "Refresh from pane", false),
        ("search-again", "Repeat search", false),
        ("search-backward", "Search backward", true),
        ("search-forward", "Search forward", true),
        ("select-line", "Select line", false),
        ("select-word", "Select word", false),
        ("start-of-line", "Move to start of line", false),
        ("top-line", "Move to top line", false),
    ];
    let selected = if let Some(action) = action {
        COMMANDS.iter().find(|(name, _, _)| name == &action)
    } else {
        let choices: Vec<Row> = COMMANDS
            .iter()
            .map(|(name, description, _)| Row::new(*name, format!("{name:<34} {description}")))
            .collect();
        choose(
            &choices,
            context,
            "copy-mode> ",
            "Select a copy-mode command",
            false,
            None,
        )?
        .first()
        .and_then(|index| COMMANDS.get(*index))
    };
    let Some((name, _, requires_arg)) = selected else {
        return Ok(());
    };
    if *requires_arg {
        if let Some(value) = prompt_text(context, &format!("{name}> "), "Enter command argument")? {
            tmux::run(["send-keys", "-X", name, value.as_str()])?;
        }
    } else {
        tmux::run(["send-keys", "-X", name])?;
    }
    Ok(())
}

fn run_command(context: &ManagerContext) -> Result<()> {
    let output = tmux::stdout(["list-commands"])?;
    let rows: Vec<Row> = output
        .lines()
        .filter_map(|line| line.split_whitespace().next().map(|id| Row::new(id, line)))
        .collect();
    if let Some(row) =
        selected_rows(&rows, context, "command> ", "Select a command", false, None)?.first()
    {
        tmux::run(["command-prompt", "-I", row.id.as_str()])?;
    }
    Ok(())
}

fn run_keybinding(context: &ManagerContext) -> Result<()> {
    let output = tmux::stdout(["list-keys"])?;
    let rows: Vec<Row> = output
        .lines()
        .enumerate()
        .map(|(index, line)| Row::new(index.to_string(), line))
        .collect();
    let selected = selected_rows(
        &rows,
        context,
        "binding> ",
        "Select a key binding",
        false,
        None,
    )?;
    let Some(row) = selected.first() else {
        return Ok(());
    };
    execute_binding(&row.display)
}

fn execute_binding(line: &str) -> Result<()> {
    let parts = shell_words::split(line).context("failed to parse tmux key binding")?;
    if parts.first().map(String::as_str) != Some("bind-key") {
        anyhow::bail!("unsupported key binding output: {line}");
    }
    let mut index = 1;
    let mut copy_mode = false;
    while index < parts.len() {
        match parts[index].as_str() {
            "-r" | "-n" => index += 1,
            "-T" | "-N" => {
                if parts[index] == "-T"
                    && parts
                        .get(index + 1)
                        .is_some_and(|value| value.starts_with("copy-mode"))
                {
                    copy_mode = true;
                }
                index += 2;
            }
            _ => break,
        }
    }
    index += 1;
    if index >= parts.len() {
        anyhow::bail!("binding has no command: {line}");
    }
    if copy_mode {
        tmux::run_ignore(["copy-mode"]);
    }
    tmux::run(parts[index..].iter().map(String::as_str))
}

fn run_menu(context: &ManagerContext) -> Result<()> {
    let config = &context.config;
    let Some(raw) = config.menu.as_deref() else {
        tmux::display_message("tmux-nexus: manager menu is not configured");
        return Ok(());
    };
    let entries = parse_menu(raw)?;
    let rows: Vec<Row> = entries
        .iter()
        .enumerate()
        .map(|(index, (label, _))| Row::new(index.to_string(), label))
        .collect();
    let selected = choose(
        &rows,
        context,
        "menu> ",
        "Select a configured command",
        false,
        None,
    )?;
    let Some(index) = selected.first() else {
        return Ok(());
    };
    let command = &entries[*index].1;
    if config.menu_popup {
        tmux::run([
            "display-popup",
            "-E",
            "-w",
            config.menu_popup_width.as_str(),
            "-h",
            config.menu_popup_height.as_str(),
            "-d",
            "#{pane_current_path}",
            command.as_str(),
        ])
    } else {
        tmux::run([
            "run-shell",
            "-b",
            "-c",
            "#{pane_current_path}",
            command.as_str(),
        ])
    }
}

fn parse_menu(raw: &str) -> Result<Vec<(String, String)>> {
    let normalized = raw.replace("\\n", "\n");
    let lines: Vec<&str> = normalized.lines().collect();
    let mut entries = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        while index < lines.len() && lines[index].is_empty() {
            index += 1;
        }
        if index >= lines.len() {
            break;
        }
        let label = lines[index].trim();
        index += 1;
        if label == "nil--" {
            break;
        }
        let command = lines
            .get(index)
            .context("manager menu entry is missing a command")?
            .trim();
        index += 1;
        if label.is_empty() || command.is_empty() {
            anyhow::bail!("manager menu labels and commands must not be empty");
        }
        if entries.iter().any(|(existing, _)| existing == label) {
            anyhow::bail!("duplicate manager menu label '{label}'");
        }
        entries.push((label.to_string(), command.to_string()));
    }
    Ok(entries)
}

fn session_entries(config: &ManagerConfig) -> Result<Vec<SessionEntry>> {
    let display = config
        .session_format
        .as_deref()
        .map(|format| format!("#S: {format}"))
        .unwrap_or_else(|| {
            "#{session_name}: #{session_windows} windows #{?session_attached,[attached],[detached]}"
                .into()
        });
    let output = tmux::stdout([
        "list-sessions",
        "-F",
        &format!("#{{session_id}}\t#{{session_attached}}\t{display}"),
    ])?;
    Ok(output.lines().filter_map(parse_session_entry).collect())
}

fn parse_session_entry(line: &str) -> Option<SessionEntry> {
    let mut fields = line.splitn(3, '\t');
    let id = fields.next()?;
    let attached_clients = fields.next()?.parse().ok()?;
    let display = fields.next().unwrap_or_default();
    Some(SessionEntry {
        row: Row::new(id, display),
        attached_clients,
    })
}

fn window_entries(config: &ManagerConfig) -> Result<Vec<WindowEntry>> {
    let display = config
        .window_format
        .as_deref()
        .map(|format| format!("#S:#{{window_index}}: {format}"))
        .unwrap_or_else(|| {
            "#S:#{window_index}: #{window_name} #{?window_active,[active],[inactive]}".into()
        });
    let format = format!("#{{session_id}}\t#{{window_id}}\t{display}");
    let mut args = vec!["list-windows", "-a"];
    if let Some(filter) = config.window_filter.as_deref() {
        args.extend(["-f", filter]);
    }
    args.extend(["-F", format.as_str()]);
    let output = tmux::stdout(args)?;
    Ok(output.lines().filter_map(parse_window_entry).collect())
}

fn window_target(session_id: &str, window_id: &str) -> String {
    format!("{session_id}:{window_id}")
}

fn parse_window_entry(line: &str) -> Option<WindowEntry> {
    let mut fields = line.splitn(3, '\t');
    let session_id = fields.next()?.to_string();
    let window_id = fields.next()?.to_string();
    let display = fields.next().unwrap_or_default();
    Some(WindowEntry {
        row: Row::new(window_target(&session_id, &window_id), display),
        session_id,
        window_id,
    })
}

fn pane_entries(config: &ManagerConfig) -> Result<Vec<PaneEntry>> {
    let display = config
        .pane_format
        .as_deref()
        .map(|format| format!("#S:#{{window_index}}.#{{pane_index}}: {format}"))
        .unwrap_or_else(|| "#S:#{window_index}.#{pane_index}: [#{window_name}:#{pane_title}] #{pane_current_command} [#{pane_width}x#{pane_height}] [history #{history_size}/#{history_limit}] #{?pane_active,[active],[inactive]}".into());
    let format = format!(
        "#{{session_id}}\t#{{window_id}}\t#{{pane_id}}\t#{{pane_path}}\t#{{host}}\t{display}"
    );
    let output = tmux::stdout(["list-panes", "-a", "-F", format.as_str()])?;
    Ok(output
        .lines()
        .filter_map(parse_pane_entry)
        .map(|mut pane| {
            if let Some(context) =
                render_pane_context(&pane.pane_path, &pane.host, config.pane_remote)
            {
                pane.row.display.push_str(" [");
                pane.row.display.push_str(&context);
                pane.row.display.push(']');
            }
            pane
        })
        .collect())
}

fn parse_pane_entry(line: &str) -> Option<PaneEntry> {
    let mut fields = line.splitn(6, '\t');
    let session_id = fields.next()?.to_string();
    let window_id = fields.next()?.to_string();
    let pane_id = fields.next()?.to_string();
    let pane_path = fields.next().unwrap_or_default().to_string();
    let host = fields.next().unwrap_or_default().to_string();
    let display = fields.next().unwrap_or_default();
    Some(PaneEntry {
        row: Row::new(&pane_id, display),
        session_id,
        window_id,
        pane_id,
        pane_path,
        host,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PanePath<'a> {
    authority: &'a str,
    path: &'a str,
}

const REMOTE_CONTEXT_MAX_WIDTH: usize = 48;

fn render_pane_context(pane_path: &str, host: &str, mode: PaneRemoteMode) -> Option<String> {
    if mode == PaneRemoteMode::Off {
        return None;
    }

    let parsed = parse_pane_path(pane_path)?;
    let local = is_local_authority(parsed.authority, host);
    if mode == PaneRemoteMode::Auto && local {
        return None;
    }

    // Decoding can reintroduce tabs/control characters; the picker sanitizes
    // row display text before passing it to fzf.
    let path = percent_decode(parsed.path).unwrap_or_else(|| parsed.path.to_string());
    let context = if parsed.authority.is_empty() {
        path
    } else {
        format!("{}:{}", parsed.authority, path)
    };
    Some(truncate_display(&context, REMOTE_CONTEXT_MAX_WIDTH))
}

fn parse_pane_path(value: &str) -> Option<PanePath<'_>> {
    let value = value.strip_prefix("file://")?;
    let separator = value.find('/')?;
    Some(PanePath {
        authority: &value[..separator],
        path: &value[separator..],
    })
}

fn is_local_authority(authority: &str, host: &str) -> bool {
    let authority = authority.trim_end_matches('.');
    if authority.is_empty() || authority.eq_ignore_ascii_case("localhost") {
        return true;
    }

    let host = host.trim_end_matches('.');
    if host.is_empty() {
        return false;
    }

    authority.eq_ignore_ascii_case(host)
        || authority.eq_ignore_ascii_case(short_hostname(host))
        || short_hostname(authority).eq_ignore_ascii_case(host)
}

fn short_hostname(host: &str) -> &str {
    host.split('.').next().unwrap_or(host)
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            output.push(bytes[index]);
            index += 1;
            continue;
        }

        if index + 2 >= bytes.len() {
            output.push(b'%');
            index += 1;
            continue;
        }

        let Some(high) = (bytes[index + 1] as char).to_digit(16) else {
            output.push(b'%');
            index += 1;
            continue;
        };
        let Some(low) = (bytes[index + 2] as char).to_digit(16) else {
            output.push(b'%');
            index += 1;
            continue;
        };
        output.push((high * 16 + low) as u8);
        index += 3;
    }
    String::from_utf8(output).ok()
}

fn truncate_display(value: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(value) <= max_width {
        return value.to_string();
    }
    if max_width <= 3 {
        return ".".repeat(max_width);
    }

    let content_width = max_width - 3;
    let left_width = content_width.div_ceil(2);
    let right_width = content_width / 2;
    let left = take_graphemes(value, left_width, false);
    let right = take_graphemes(value, right_width, true);
    format!("{left}...{right}")
}

fn take_graphemes(value: &str, max_width: usize, from_end: bool) -> String {
    let graphemes: Vec<&str> = value.graphemes(true).collect();
    let iterator: Box<dyn Iterator<Item = &&str>> = if from_end {
        Box::new(graphemes.iter().rev())
    } else {
        Box::new(graphemes.iter())
    };
    let mut selected = Vec::new();
    let mut width = 0;
    for grapheme in iterator {
        let grapheme_width = UnicodeWidthStr::width(*grapheme);
        if width + grapheme_width > max_width {
            break;
        }
        selected.push(*grapheme);
        width += grapheme_width;
    }
    if from_end {
        selected.into_iter().rev().collect()
    } else {
        selected.concat()
    }
}

fn resolve_action(
    action: Option<&str>,
    actions: &[&str],
    context: &ManagerContext,
    prompt: &str,
) -> Result<String> {
    if let Some(action) = action {
        if actions.contains(&action) {
            return Ok(action.to_string());
        }
        anyhow::bail!("unknown action '{action}'");
    }
    let rows = rows(actions);
    Ok(
        choose(&rows, context, prompt, "Select an action", false, None)?
            .first()
            .map(|index| rows[*index].id.clone())
            .unwrap_or_default(),
    )
}

fn selected_rows<'a>(
    rows: &'a [Row],
    context: &ManagerContext,
    prompt: &str,
    header: &str,
    multi: bool,
    preview_kind: Option<&str>,
) -> Result<Vec<&'a Row>> {
    Ok(choose(rows, context, prompt, header, multi, preview_kind)?
        .into_iter()
        .filter_map(|index| rows.get(index))
        .collect())
}

fn choose(
    rows: &[Row],
    context: &ManagerContext,
    prompt: &str,
    header: &str,
    multi: bool,
    preview_kind: Option<&str>,
) -> Result<Vec<usize>> {
    choose_with_preview(
        rows,
        context,
        prompt,
        header,
        multi,
        preview_kind.map(PreviewSpec::Rows),
    )
}

fn choose_with_preview(
    rows: &[Row],
    context: &ManagerContext,
    prompt: &str,
    header: &str,
    multi: bool,
    preview_spec: Option<PreviewSpec<'_>>,
) -> Result<Vec<usize>> {
    if rows.is_empty() {
        tmux::display_message("tmux-nexus: no manager items available");
        return Ok(Vec::new());
    }
    let config = &context.config;
    let mut data = matches!(preview_spec, Some(PreviewSpec::Rows(_)))
        .then(NamedTempFile::new)
        .transpose()?;
    if let Some(data) = data.as_mut() {
        serde_json::to_writer(&mut *data, rows)?;
        data.flush()?;
    }
    let mut command = context.picker.command();
    command.args([
        "--delimiter",
        "\t",
        "--with-nth",
        "2..",
        "--layout=reverse",
        "--info=inline",
        "--tiebreak=index",
        "--prompt",
        prompt,
        "--header",
        header,
    ]);
    command.arg(if multi { "-m" } else { "+m" });
    if let Some(preview_spec) = preview_spec {
        let exe = env::current_exe()?;
        let preview = match preview_spec {
            PreviewSpec::Rows(kind) => {
                let data = data
                    .as_ref()
                    .context("manager preview data is unavailable")?;
                format!(
                    "{} manage-preview --kind {} --data {} --row {{1}}",
                    shell_quote(&exe.to_string_lossy()),
                    shell_quote(kind),
                    shell_quote(&data.path().to_string_lossy())
                )
            }
        };
        let follow = if context.picker.preview_follow {
            ":follow"
        } else {
            ""
        };
        let hidden = if config.preview { "" } else { ":hidden" };
        command.args([
            "--preview-window",
            &format!("right:60%:wrap{follow}{hidden}"),
            "--preview",
            preview.as_str(),
        ]);
    }
    command.stdin(Stdio::piped()).stdout(Stdio::piped());
    let mut child = command.spawn().context("failed to start manager fzf")?;
    if let Some(stdin) = child.stdin.as_mut() {
        for (index, row) in rows.iter().enumerate() {
            let result = writeln!(stdin, "{}\t{}", index, sanitize(&row.display));
            if let Err(error) = result {
                if error.kind() != ErrorKind::BrokenPipe {
                    return Err(error.into());
                }
                break;
            }
        }
    }
    let output = child.wait_with_output()?;
    match picker_exit(output.status.code()) {
        PickerExit::Selected => String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| {
                line.split('\t')
                    .next()
                    .context("fzf returned an empty row")?
                    .parse::<usize>()
                    .context("fzf returned an invalid row")
            })
            .collect(),
        PickerExit::Cancelled => Ok(Vec::new()),
        PickerExit::Failed => anyhow::bail!("manager fzf exited with status {}", output.status),
    }
}

fn prompt_text(context: &ManagerContext, prompt: &str, header: &str) -> Result<Option<String>> {
    let mut command = context.picker.command();
    command.args([
        "+m",
        "--print-query",
        "--phony",
        "--no-sort",
        "--prompt",
        prompt,
        "--header",
        header,
    ]);
    command.stdin(Stdio::piped()).stdout(Stdio::piped());
    let mut child = command.spawn()?;
    if let Some(stdin) = child.stdin.as_mut() {
        writeln!(stdin, "Press Enter to accept the query")?;
    }
    let output = child.wait_with_output()?;
    match picker_exit(output.status.code()) {
        PickerExit::Selected => {}
        PickerExit::Cancelled => return Ok(None),
        PickerExit::Failed => {
            anyhow::bail!("manager fzf exited with status {}", output.status)
        }
    }
    let query = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    Ok((!query.is_empty()).then_some(query))
}

fn without_multi(options: Vec<String>) -> Vec<String> {
    options
        .into_iter()
        .filter(|option| !is_multi_option(option))
        .collect()
}

fn is_multi_option(option: &str) -> bool {
    if matches!(option, "-m" | "+m" | "--multi" | "--no-multi") || option.starts_with("--multi=") {
        return true;
    }
    option
        .strip_prefix("-m")
        .is_some_and(|count| !count.is_empty() && count.bytes().all(|byte| byte.is_ascii_digit()))
}

fn without_tmux_options(options: Vec<String>) -> Vec<String> {
    without_layout_options(options, true, false)
}

fn without_popup_options(options: Vec<String>) -> Vec<String> {
    without_layout_options(options, false, true)
}

fn without_layout_options(
    options: Vec<String>,
    include_split: bool,
    preserve_separator: bool,
) -> Vec<String> {
    let mut result = Vec::new();
    let mut iter = options.into_iter();
    while let Some(option) = iter.next() {
        if option == "--" {
            if preserve_separator {
                result.push(option);
            }
            result.extend(iter);
            break;
        }
        if is_tmux_layout_option(&option, include_split) {
            if tmux_layout_option_takes_value(&option)
                && iter
                    .as_slice()
                    .first()
                    .is_some_and(|value| is_tmux_layout_value(value))
            {
                let _ = iter.next();
            }
            continue;
        }
        result.push(option);
    }
    result
}

fn is_tmux_layout_option(option: &str, include_split: bool) -> bool {
    if option == "--popup" || option.starts_with("--popup=") {
        return true;
    }
    let Some(flag) = option
        .strip_prefix('-')
        .and_then(|value| value.chars().next())
    else {
        return false;
    };
    matches!(flag, 'p' | 'w' | 'h' | 'x' | 'y')
        || (include_split && matches!(flag, 'd' | 'u' | 'l' | 'r'))
}

fn tmux_layout_option_takes_value(option: &str) -> bool {
    option == "--popup"
        || matches!(
            option,
            "-p" | "-w" | "-h" | "-x" | "-y" | "-d" | "-u" | "-l" | "-r"
        )
}

fn is_tmux_layout_value(value: &str) -> bool {
    !value.is_empty()
        && (value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'%' | b','))
            || (value.len() == 1 && value.as_bytes()[0].is_ascii_uppercase()))
}

fn destructive_tmux(
    context: &ManagerContext,
    prompt: &str,
    commands: Vec<Vec<OsString>>,
) -> Result<()> {
    let config = &context.config;
    let client = &context.tmux;
    if !config.confirm {
        return client.run_commands(&commands);
    }
    let command = commands
        .iter()
        .map(|args| {
            args.iter()
                .map(|arg| shell_quote(&arg.to_string_lossy()))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join(" ; ");
    client.run([
        "confirm-before",
        "-p",
        &format!("{prompt} (y/n)"),
        command.as_str(),
    ])
}

pub fn doctor_summary() -> Result<String> {
    let config = ManagerConfig::load()?;
    Ok(format!(
        "key={} order={} preview={} confirm={} pane_remote={} copyq={} pstree={} privilege={}",
        config.key,
        config.order.join("|"),
        config.preview,
        config.confirm,
        config.pane_remote.as_str(),
        tmux::have("copyq"),
        tmux::have("pstree"),
        if tmux::have("sudo") {
            "sudo"
        } else if tmux::have("doas") {
            "doas"
        } else {
            "none"
        }
    ))
}

fn command<const N: usize>(args: [&str; N]) -> Vec<OsString> {
    args.into_iter().map(OsString::from).collect()
}

fn client_switch_commands(session: &str) -> Vec<Vec<OsString>> {
    if env::var_os("TMUX").is_some() {
        vec![command(["switch-client", "-t", session])]
    } else {
        Vec::new()
    }
}

fn rows(values: &[&str]) -> Vec<Row> {
    values
        .iter()
        .map(|value| Row::new(*value, *value))
        .collect()
}

impl Row {
    fn new(id: impl Into<String>, display: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            display: display.into(),
        }
    }
}

fn parse_bool(value: &str) -> Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => anyhow::bail!("expected a boolean, got '{value}'"),
    }
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character == '\t' || character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn print_tmux<I, S>(args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    print!("{}", tmux::stdout(args)?);
    Ok(())
}

fn ensure_program(program: &str, message: &str) -> Result<()> {
    if tmux::have(program) {
        Ok(())
    } else {
        anyhow::bail!(message.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        parse_bool, parse_menu, parse_pane_entry, parse_session_entry, parse_window_entry,
        percent_decode, picker_exit, render_pane_context, truncate_display, without_multi,
        without_popup_options, without_tmux_options, PaneRemoteMode, PickerExit,
    };

    #[test]
    fn parses_remote_context_modes() {
        assert_eq!("auto".parse(), Ok(PaneRemoteMode::Auto));
        assert_eq!("ALWAYS".parse(), Ok(PaneRemoteMode::Always));
        assert_eq!("off".parse(), Ok(PaneRemoteMode::Off));
        assert!("sometimes".parse::<PaneRemoteMode>().is_err());
    }

    #[test]
    fn parses_legacy_menu() {
        let menu = parse_menu("foo\\necho hello\\n\\nbar\\nprintf world\\n\\nnil--\\n\\n").unwrap();
        assert_eq!(menu[0], ("foo".into(), "echo hello".into()));
        assert_eq!(menu[1], ("bar".into(), "printf world".into()));
    }

    #[test]
    fn rejects_duplicate_menu_labels() {
        assert!(parse_menu("foo\none\n\nfoo\ntwo\n").is_err());
    }

    #[test]
    fn strips_popup_options_for_plain_fzf() {
        assert_eq!(
            without_tmux_options(vec![
                "-p".into(),
                "80%,60%".into(),
                "-w80%".into(),
                "-d".into(),
                "40%".into(),
                "--".into(),
                "--ansi".into()
            ]),
            vec!["--ansi"]
        );
    }

    #[test]
    fn keeps_split_options_when_popup_is_unsupported() {
        assert_eq!(
            without_popup_options(vec![
                "-p80%,60%".into(),
                "-w".into(),
                "80%".into(),
                "-d".into(),
                "40%".into(),
                "--".into(),
                "-p".into(),
            ]),
            vec!["-d", "40%", "--", "-p"]
        );
    }

    #[test]
    fn manager_controls_multi_selection() {
        assert_eq!(
            without_multi(vec![
                "-m".into(),
                "-m3".into(),
                "--multi=4".into(),
                "--ansi".into(),
            ]),
            vec!["--ansi"]
        );
    }

    #[test]
    fn parses_boolean_values() {
        assert!(parse_bool("yes").unwrap());
        assert!(!parse_bool("off").unwrap());
        assert!(parse_bool("maybe").is_err());
    }

    #[test]
    fn classifies_manager_picker_exit_codes() {
        assert_eq!(picker_exit(Some(0)), PickerExit::Selected);
        assert_eq!(picker_exit(Some(1)), PickerExit::Cancelled);
        assert_eq!(picker_exit(Some(130)), PickerExit::Cancelled);
        assert_eq!(picker_exit(Some(2)), PickerExit::Failed);
        assert_eq!(picker_exit(None), PickerExit::Failed);
    }

    #[test]
    fn parses_typed_workspace_entries_with_tabs_in_display() {
        let session = parse_session_entry("$1\t2\twork\tquoted").unwrap();
        assert_eq!(session.row.id, "$1");
        assert_eq!(session.attached_clients, 2);
        assert_eq!(session.row.display, "work\tquoted");

        let window = parse_window_entry("$1\t@2\twork:1\tname").unwrap();
        assert_eq!(window.session_id, "$1");
        assert_eq!(window.window_id, "@2");
        assert_eq!(window.row.id, "$1:@2");
        assert_eq!(window.row.display, "work:1\tname");

        let pane = parse_pane_entry("$1\t@2\t%3\t\tglehost\twork:1.0\ttitle").unwrap();
        assert_eq!(pane.session_id, "$1");
        assert_eq!(pane.window_id, "@2");
        assert_eq!(pane.pane_id, "%3");
        assert_eq!(pane.row.display, "work:1.0\ttitle");
        assert!(pane.pane_path.is_empty());
        assert_eq!(pane.host, "glehost");
    }

    #[test]
    fn normalizes_remote_pane_paths() {
        assert_eq!(render_pane_context("", "host", PaneRemoteMode::Auto), None);
        assert_eq!(
            render_pane_context("junk; rm -rf /", "host", PaneRemoteMode::Auto),
            None
        );
        assert_eq!(
            render_pane_context("file://server/path-B", "glehost", PaneRemoteMode::Auto),
            Some("server:/path-B".into())
        );
        assert_eq!(
            render_pane_context("file://glehost/x", "glehost", PaneRemoteMode::Auto),
            None
        );
        assert_eq!(
            render_pane_context(
                "file://glehost.example.com/x",
                "glehost",
                PaneRemoteMode::Auto
            ),
            None
        );
        assert_eq!(
            render_pane_context(
                "file://glehost/x",
                "glehost.example.com",
                PaneRemoteMode::Auto
            ),
            None
        );
        assert_eq!(
            render_pane_context(
                "file://web1.prod.example.com/x",
                "web1.dev.example.com",
                PaneRemoteMode::Auto
            ),
            Some("web1.prod.example.com:/x".into())
        );
        assert_eq!(
            render_pane_context(
                "file://dev.customer.example.net/x",
                "dev.corp.example.com",
                PaneRemoteMode::Auto
            ),
            Some("dev.customer.example.net:/x".into())
        );
        assert_eq!(
            render_pane_context("file:///only/path", "glehost", PaneRemoteMode::Auto),
            None
        );
        assert_eq!(
            render_pane_context("file://localhost/x", "glehost", PaneRemoteMode::Auto),
            None
        );
        assert_eq!(
            render_pane_context(
                "file://srv/a%20b/%E4%B8%AD",
                "glehost",
                PaneRemoteMode::Auto
            ),
            Some("srv:/a b/中".into())
        );
        assert_eq!(
            render_pane_context("file://srv/a%2", "glehost", PaneRemoteMode::Auto),
            Some("srv:/a%2".into())
        );
        assert_eq!(
            render_pane_context("file://srv/x", "glehost", PaneRemoteMode::Off),
            None
        );
        assert_eq!(
            render_pane_context("file://glehost/x", "glehost", PaneRemoteMode::Always),
            Some("glehost:/x".into())
        );
    }

    #[test]
    fn percent_decoding_preserves_malformed_but_rejects_non_utf8_values() {
        assert_eq!(percent_decode("a%20b/%E4%B8%AD"), Some("a b/中".into()));
        assert_eq!(percent_decode("a%20b/100%/c"), Some("a b/100%/c".into()));
        assert_eq!(percent_decode("trailing%"), Some("trailing%".into()));
        assert_eq!(percent_decode("bad%zz"), Some("bad%zz".into()));
        assert_eq!(percent_decode("bad%C3%28"), None);
    }

    #[test]
    fn truncates_remote_context_by_display_width() {
        let truncated = truncate_display("0123456789abcdefghijklmnopqrstuvwxyz", 11);
        assert_eq!(truncated, "0123...wxyz");
        assert!(unicode_width::UnicodeWidthStr::width(truncated.as_str()) <= 11);
        assert!(truncated.contains("..."));
        assert_eq!(truncate_display("abcdef", 3), "...");
        assert_eq!(truncate_display("abcdef", 2), "..");
    }
}
