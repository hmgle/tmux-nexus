use std::{
    ffi::OsString,
    fs::File,
    io::{self, BufWriter, Write},
    path::PathBuf,
};

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use tempfile::Builder;

use crate::{
    action, capture,
    config::{Config, ConfigOverrides},
    fzf, index, manage, motion, preview, search, tmux,
    types::{ActionKind, CaseMode, Scope, SearchMode},
};

#[derive(Parser)]
#[command(
    name = "tnx",
    version,
    about = "Search, navigate, and manage tmux workspaces"
)]
struct App {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Search(SearchArgs),
    Capture(CaptureArgs),
    Preview(PreviewArgs),
    Action(ActionArgs),
    Motion(motion::MotionArgs),
    Manage(manage::ManageArgs),
    #[command(hide = true)]
    ManagePreview(manage::PreviewArgs),
    Doctor,
}

#[derive(Clone, Debug, Default, Args)]
struct SearchArgs {
    #[arg(value_name = "QUERY")]
    positional_query: Option<String>,
    #[arg(short = 'q', long = "query")]
    query: Option<String>,
    #[arg(short = 's', long = "scope", value_enum)]
    scope: Option<Scope>,
    #[arg(long = "action", value_enum)]
    action: Option<ActionKind>,
    #[arg(long = "case", value_enum)]
    case_mode: Option<CaseMode>,
    #[arg(long = "no-history", conflicts_with = "history")]
    no_history: bool,
    #[arg(long = "history", conflicts_with = "no_history")]
    history: bool,
    #[arg(long = "history-lines", value_name = "LINES")]
    history_lines: Option<usize>,
    #[arg(long = "no-join")]
    no_join: bool,
    #[arg(long = "no-skip-blank")]
    no_skip_blank: bool,
    #[arg(short = 't', long = "target")]
    target_pane: Option<String>,
    #[arg(long = "print")]
    print: bool,
    #[arg(long = "regex", conflicts_with = "literal")]
    regex: bool,
    #[arg(long = "literal")]
    literal: bool,
    #[arg(long = "no-preview")]
    no_preview: bool,
    #[arg(long = "query-option", hide = true)]
    query_option: Option<String>,
    #[arg(long = "require-query", hide = true)]
    require_query: bool,
}

#[derive(Clone, Debug, Default, Args)]
struct CaptureArgs {
    #[arg(value_name = "OUTPUT")]
    positional_output: Option<PathBuf>,
    #[arg(short = 's', long = "scope", value_enum)]
    scope: Option<Scope>,
    #[arg(long = "no-history", conflicts_with = "history")]
    no_history: bool,
    #[arg(long = "history", conflicts_with = "no_history")]
    history: bool,
    #[arg(long = "history-lines", value_name = "LINES")]
    history_lines: Option<usize>,
    #[arg(long = "no-join")]
    no_join: bool,
    #[arg(long = "no-skip-blank")]
    no_skip_blank: bool,
    #[arg(short = 't', long = "target")]
    target_pane: Option<String>,
    #[arg(short = 'o', long = "output")]
    output: Option<PathBuf>,
}

#[derive(Clone, Debug, Args)]
struct PreviewArgs {
    #[arg(long = "index")]
    index: Option<PathBuf>,
    #[arg(long = "record-id")]
    record_id: Option<usize>,
    #[arg(long = "pane-index", hide = true)]
    pane_index: Option<usize>,
    #[arg(long = "line-index", hide = true)]
    line_index: Option<usize>,
    #[arg(long = "query")]
    query: Option<String>,
    #[arg(value_name = "LEGACY_RECORD")]
    legacy_record: Option<String>,
}

#[derive(Clone, Debug, Args)]
struct ActionArgs {
    #[arg(long = "action", value_enum)]
    action: Option<ActionKind>,
    #[arg(long = "index")]
    index: Option<PathBuf>,
    #[arg(long = "record-id")]
    record_id: Option<usize>,
    #[arg(long = "record")]
    legacy_record: Option<String>,
}

impl SearchArgs {
    fn resolved_query(&self) -> Option<String> {
        if let Some(query) = self
            .query
            .as_deref()
            .or(self.positional_query.as_deref())
            .filter(|value| !value.is_empty())
        {
            return Some(query.to_string());
        }

        self.query_option
            .as_deref()
            .and_then(read_query_option)
            .filter(|value| !value.is_empty())
    }

    fn overrides(&self) -> ConfigOverrides {
        ConfigOverrides {
            scope: self.scope,
            include_history: history_override(self.history, self.no_history),
            history_lines: history_lines_override(self.history_lines),
            case_mode: self.case_mode,
            join_wraps: self.no_join.then_some(false),
            skip_blank: self.no_skip_blank.then_some(false),
            preview: self.no_preview.then_some(false),
            default_action: self
                .action
                .or_else(|| self.print.then_some(ActionKind::Print)),
            search_mode: self
                .regex
                .then_some(SearchMode::Regex)
                .or_else(|| self.literal.then_some(SearchMode::Literal)),
        }
    }
}

impl CaptureArgs {
    fn overrides(&self) -> ConfigOverrides {
        ConfigOverrides {
            scope: self.scope,
            include_history: history_override(self.history, self.no_history),
            history_lines: history_lines_override(self.history_lines),
            join_wraps: self.no_join.then_some(false),
            skip_blank: self.no_skip_blank.then_some(false),
            ..ConfigOverrides::default()
        }
    }

    fn output_path(&self) -> Option<&PathBuf> {
        self.output.as_ref().or(self.positional_output.as_ref())
    }
}

pub fn run() -> Result<()> {
    let app = App::parse_from(normalize_args());
    match app.command {
        Command::Search(args) => run_search(args),
        Command::Capture(args) => run_capture(args),
        Command::Preview(args) => run_preview(args),
        Command::Action(args) => run_action(args),
        Command::Motion(args) => run_motion(args),
        Command::Manage(args) => manage::run(args),
        Command::ManagePreview(args) => manage::preview(args),
        Command::Doctor => run_doctor(),
    }
}

fn normalize_args() -> Vec<OsString> {
    let mut args: Vec<OsString> = std::env::args_os().collect();
    if args.len() == 1 {
        args.push("search".into());
        return args;
    }

    let first = args[1].to_string_lossy();
    let known = matches!(
        first.as_ref(),
        "search"
            | "capture"
            | "preview"
            | "action"
            | "motion"
            | "manage"
            | "manage-preview"
            | "doctor"
            | "help"
    );
    let root_flag = matches!(first.as_ref(), "-h" | "--help" | "-V" | "--version");
    if !known && !root_flag {
        args.insert(1, "search".into());
    }
    args
}

fn run_search(args: SearchArgs) -> Result<()> {
    ensure_tmux()?;
    let config = Config::load(&args.overrides())?;
    let query = args.resolved_query();
    if args.require_query && query.is_none() {
        return Ok(());
    }
    let index = capture::build_index(&config, args.target_pane.as_deref())?;

    if index.records.is_empty() {
        tmux::display_message("tmux-nexus: no pane content to search");
        return Ok(());
    }

    let record_ids = search::filter_record_ids(&index, query.as_deref(), &config)?;
    if record_ids.is_empty() {
        let msg = query
            .as_deref()
            .map(|query| format!("tmux-nexus: no matches for '{query}'"))
            .unwrap_or_else(|| "tmux-nexus: no matches".to_string());
        tmux::display_message(&msg);
        return Ok(());
    }

    if config.default_action == ActionKind::Print && query.is_some() {
        for record_id in record_ids {
            action::execute(&index, record_id, ActionKind::Print, &config)?;
        }
        return Ok(());
    }

    ensure_fzf()?;
    let preview_dir = Builder::new().prefix("tnx_preview.").tempdir()?;
    if config.preview {
        index.save_preview_panes(preview_dir.path())?;
    }
    let picked = fzf::run_picker(&index, &record_ids, &config, preview_dir.path())?;
    for record_id in picked.record_ids {
        action::execute(&index, record_id, picked.action, &config)?;
    }
    Ok(())
}

fn run_capture(args: CaptureArgs) -> Result<()> {
    ensure_tmux()?;
    let config = Config::load(&args.overrides())?;
    if let Some(path) = args.output_path() {
        let file =
            File::create(path).with_context(|| format!("failed to create {}", path.display()))?;
        let mut writer = BufWriter::new(file);
        capture::write_legacy_tsv(&config, args.target_pane.as_deref(), &mut writer)?;
        writer
            .flush()
            .with_context(|| format!("failed to write {}", path.display()))?;
    } else {
        let stdout = io::stdout();
        let mut writer = stdout.lock();
        capture::write_legacy_tsv(&config, args.target_pane.as_deref(), &mut writer)?;
        writer.flush()?;
    }
    Ok(())
}

fn run_preview(args: PreviewArgs) -> Result<()> {
    if let (Some(index_path), Some(pane_index), Some(line_index)) =
        (args.index.as_ref(), args.pane_index, args.line_index)
    {
        let pane = index::SearchIndex::load_preview_pane(index_path, pane_index)?;
        return preview::print_pane_preview(&pane, line_index);
    }

    if let (Some(index_path), Some(record_id)) = (args.index.as_ref(), args.record_id) {
        let index = index::SearchIndex::load(index_path)?;
        return preview::print_index_preview(&index, record_id, args.query.as_deref());
    }

    if let Some(raw) = args.legacy_record.as_deref() {
        let record = index::LegacyRecord::parse(raw).context("malformed legacy record")?;
        return preview::print_legacy_preview(&record);
    }

    anyhow::bail!("preview requires --index + --record-id or a legacy record");
}

fn run_action(args: ActionArgs) -> Result<()> {
    let config = Config::load(&ConfigOverrides::default())?;
    let action = args.action.unwrap_or(config.default_action);

    if let (Some(index_path), Some(record_id)) = (args.index.as_ref(), args.record_id) {
        let index = index::SearchIndex::load(index_path)?;
        return action::execute(&index, record_id, action, &config);
    }

    if let Some(raw) = args.legacy_record.as_deref() {
        let record = index::LegacyRecord::parse(raw).context("malformed legacy record")?;
        return action::execute_legacy(&record, action, &config);
    }

    anyhow::bail!("action requires --index + --record-id or --record");
}

fn run_motion(args: motion::MotionArgs) -> Result<()> {
    let target_client = args.target_client.clone();
    let overlay = args.overlay;
    let supervised = args.ready_title.is_some();
    let result = (|| {
        ensure_tmux()?;
        let config = Config::load(&ConfigOverrides::default())?;
        motion::run(args, &config)
    })();
    if overlay && supervised {
        if let (Err(error), Ok(pane)) = (&result, std::env::var("TMUX_PANE")) {
            tmux::run_ignore([
                "set-option",
                "-p",
                "-t",
                &pane,
                "@tnx_motion_error",
                &format!("{error:#}"),
            ]);
        }
    } else if !overlay {
        if let Err(error) = &result {
            let message = format!("tmux-nexus motion: {error:#}");
            if let Some(client) = target_client.as_deref() {
                tmux::run_ignore(["display-message", "-c", client, &message]);
            } else {
                tmux::display_message(&message);
            }
        }
    }
    result
}

fn run_doctor() -> Result<()> {
    println!("tnx {}", env!("CARGO_PKG_VERSION"));
    println!("tmux: {}", describe_program("tmux", &["-V"], true));
    println!("fzf: {}", describe_program("fzf", &["--version"], true));
    println!(
        "fzf-tmux: {}",
        describe_program("fzf-tmux", &["--help"], false)
    );
    println!("rg: {}", describe_program("rg", &["--version"], false));
    println!("clipboard: {}", clipboard_status());
    let config = Config::load(&ConfigOverrides::default())?;
    println!(
        "config: key={} scope={} action={} preview={} prompt_query={} history={} history_lines={} join_wraps={}",
        config.launch_key,
        config.scope,
        config.default_action,
        config.preview,
        config.prompt_query,
        config.include_history,
        config
            .history_lines
            .map(|lines| lines.to_string())
            .unwrap_or_else(|| "all".to_string()),
        config.join_wraps
    );
    println!(
        "motion: key={} key2={} hints={} case={} smartsign={} copy_mode_no_prefix={}",
        config.motion_key,
        if config.motion2_key.is_empty() {
            "(disabled)"
        } else {
            config.motion2_key.as_str()
        },
        config.motion_hints,
        config.motion_case_mode,
        config.motion_smartsign,
        config.motion_copy_mode_no_prefix
    );
    println!("manager: {}", manage::doctor_summary()?);
    Ok(())
}

fn history_override(history: bool, no_history: bool) -> Option<bool> {
    if history {
        Some(true)
    } else if no_history {
        Some(false)
    } else {
        None
    }
}

fn history_lines_override(history_lines: Option<usize>) -> Option<Option<usize>> {
    history_lines.map(|lines| (lines > 0).then_some(lines))
}

fn read_query_option(option: &str) -> Option<String> {
    let value = tmux::show_option(option);
    tmux::run_ignore(["set-option", "-gu", option]);
    value
}

fn ensure_tmux() -> Result<()> {
    if tmux::have("tmux") {
        Ok(())
    } else {
        anyhow::bail!("tmux is not installed or not in PATH")
    }
}

fn ensure_fzf() -> Result<()> {
    if tmux::have("fzf") {
        Ok(())
    } else {
        anyhow::bail!("fzf is required for interactive search")
    }
}

fn describe_program(program: &str, version_args: &[&str], required: bool) -> String {
    if !tmux::have(program) {
        return if required {
            "missing (required)"
        } else {
            "missing"
        }
        .to_string();
    }
    tmux::command_version(program, version_args)
        .and_then(|value| value.lines().next().map(ToOwned::to_owned))
        .unwrap_or_else(|| "installed".to_string())
}

fn clipboard_status() -> String {
    for program in ["pbcopy", "wl-copy", "xclip", "xsel", "clip.exe"] {
        if tmux::have(program) {
            return program.to_string();
        }
    }
    "tmux buffer only".to_string()
}
