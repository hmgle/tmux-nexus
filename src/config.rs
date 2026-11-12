use std::{
    collections::{HashMap, HashSet},
    env,
    fmt::Display,
    str::FromStr,
    sync::OnceLock,
};

use anyhow::Result;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    tmux,
    types::{ActionKind, CaseMode, Scope, SearchMode},
};

const TMUX_OPTION_PREFIX: &str = "@tmux_nexus_";

#[derive(Clone, Debug)]
pub struct Config {
    pub launch_key: String,
    pub motion_key: String,
    pub motion2_key: String,
    pub motion_copy_mode_no_prefix: bool,
    pub scope: Scope,
    pub include_history: bool,
    pub history_lines: Option<usize>,
    pub case_mode: CaseMode,
    pub join_wraps: bool,
    pub skip_blank: bool,
    pub preview: bool,
    pub prompt_query: bool,
    pub default_action: ActionKind,
    pub fzf_options: String,
    pub search_mode: SearchMode,
    pub motion_hints: String,
    pub motion_case_mode: CaseMode,
    pub motion_smartsign: bool,
    pub motion_vertical_border: String,
    pub motion_horizontal_border: String,
    pub motion_hint1_fg: String,
    pub motion_hint2_fg: String,
    pub motion_dim: String,
}

#[derive(Clone, Debug, Default)]
pub struct ConfigOverrides {
    pub scope: Option<Scope>,
    pub include_history: Option<bool>,
    pub history_lines: Option<Option<usize>>,
    pub case_mode: Option<CaseMode>,
    pub join_wraps: Option<bool>,
    pub skip_blank: Option<bool>,
    pub preview: Option<bool>,
    pub default_action: Option<ActionKind>,
    pub search_mode: Option<SearchMode>,
}

impl Config {
    pub fn load(overrides: &ConfigOverrides) -> Result<Self> {
        let mut config = Self {
            launch_key: setting("launch_key", "TNX_LAUNCH_KEY")?.unwrap_or_else(|| "g".into()),
            motion_key: setting("motion_key", "TNX_MOTION_KEY")?.unwrap_or_else(|| "s".into()),
            motion2_key: setting("motion2_key", "TNX_MOTION2_KEY")?.unwrap_or_default(),
            motion_copy_mode_no_prefix: bool_setting(
                "motion_copy_mode_no_prefix",
                "TNX_MOTION_COPY_MODE_NO_PREFIX",
            )?
            .unwrap_or(false),
            scope: parse_setting("scope", "TNX_SCOPE")?.unwrap_or_default(),
            include_history: bool_setting("include_history", "TNX_INCLUDE_HISTORY")?
                .unwrap_or(true),
            history_lines: usize_setting("history_lines", "TNX_HISTORY_LINES")?.and_then(nonzero),
            case_mode: parse_setting("case", "TNX_CASE")?.unwrap_or_default(),
            join_wraps: bool_setting("join_wraps", "TNX_JOIN_WRAPS")?.unwrap_or(true),
            skip_blank: bool_setting("skip_blank", "TNX_SKIP_BLANK")?.unwrap_or(true),
            preview: bool_setting("preview", "TNX_PREVIEW")?.unwrap_or(true),
            prompt_query: bool_setting("prompt_query", "TNX_PROMPT_QUERY")?.unwrap_or(false),
            default_action: parse_setting("default_action", "TNX_DEFAULT_ACTION")?
                .unwrap_or_default(),
            fzf_options: setting("fzf_options", "TNX_FZF_OPTIONS")?.unwrap_or_default(),
            search_mode: SearchMode::Literal,
            motion_hints: setting("motion_hints", "TNX_MOTION_HINTS")?
                .unwrap_or_else(|| "asdghklqwertyuiopzxcvbnmfj;".into()),
            motion_case_mode: parse_setting("motion_case", "TNX_MOTION_CASE")?
                .unwrap_or(CaseMode::Insensitive),
            motion_smartsign: bool_setting("motion_smartsign", "TNX_MOTION_SMARTSIGN")?
                .unwrap_or(false),
            motion_vertical_border: setting(
                "motion_vertical_border",
                "TNX_MOTION_VERTICAL_BORDER",
            )?
            .unwrap_or_else(|| "|".into()),
            motion_horizontal_border: setting(
                "motion_horizontal_border",
                "TNX_MOTION_HORIZONTAL_BORDER",
            )?
            .unwrap_or_else(|| "-".into()),
            motion_hint1_fg: setting("motion_hint1_fg", "TNX_MOTION_HINT1_FG")?
                .unwrap_or_else(|| "1;31".into()),
            motion_hint2_fg: setting("motion_hint2_fg", "TNX_MOTION_HINT2_FG")?
                .unwrap_or_else(|| "1;32".into()),
            motion_dim: setting("motion_dim", "TNX_MOTION_DIM")?.unwrap_or_else(|| "2".into()),
        };

        if let Some(scope) = overrides.scope {
            config.scope = scope;
        }
        if let Some(include_history) = overrides.include_history {
            config.include_history = include_history;
        }
        if let Some(history_lines) = overrides.history_lines {
            config.history_lines = history_lines;
        }
        if let Some(case_mode) = overrides.case_mode {
            config.case_mode = case_mode;
        }
        if let Some(join_wraps) = overrides.join_wraps {
            config.join_wraps = join_wraps;
        }
        if let Some(skip_blank) = overrides.skip_blank {
            config.skip_blank = skip_blank;
        }
        if let Some(preview) = overrides.preview {
            config.preview = preview;
        }
        if let Some(default_action) = overrides.default_action {
            config.default_action = default_action;
        }
        if let Some(search_mode) = overrides.search_mode {
            config.search_mode = search_mode;
        }

        validate_motion_config(&config)?;
        Ok(config)
    }
}

fn setting(option_name: &str, env_name: &str) -> Result<Option<String>> {
    if let Some(value) = env::var(env_name).ok().filter(|value| !value.is_empty()) {
        return Ok(Some(value));
    }
    Ok(tmux_options()?.get(option_name).cloned())
}

fn tmux_options() -> Result<&'static HashMap<String, String>> {
    static OPTIONS: OnceLock<Result<HashMap<String, String>, String>> = OnceLock::new();
    match OPTIONS.get_or_init(|| {
        tmux::show_options(TMUX_OPTION_PREFIX)
            .map(collect_tmux_options)
            .map_err(|err| format!("{err:#}"))
    }) {
        Ok(options) => Ok(options),
        Err(err) => anyhow::bail!(err.clone()),
    }
}

fn collect_tmux_options(options: Vec<(String, String)>) -> HashMap<String, String> {
    options
        .into_iter()
        .filter(|(_, value)| !value.is_empty())
        .filter_map(|(name, value)| {
            name.strip_prefix(TMUX_OPTION_PREFIX)
                .map(|key| (key.to_string(), value))
        })
        .collect()
}

fn parse_setting<T>(option_name: &str, env_name: &str) -> Result<Option<T>>
where
    T: FromStr,
    T::Err: Display,
{
    setting(option_name, env_name)?
        .map(|value| parse_value(&value, env_name, option_name))
        .transpose()
}

fn parse_value<T>(value: &str, env_name: &str, option_name: &str) -> Result<T>
where
    T: FromStr,
    T::Err: Display,
{
    value.parse().map_err(|err| {
        anyhow::anyhow!(
            "invalid {env_name}/{} value '{value}': {err}",
            format_args!("{TMUX_OPTION_PREFIX}{option_name}"),
        )
    })
}

fn bool_setting(option_name: &str, env_name: &str) -> Result<Option<bool>> {
    setting(option_name, env_name)?
        .map(|value| match value.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => anyhow::bail!(
                "invalid {env_name}/{TMUX_OPTION_PREFIX}{option_name} value '{value}'"
            ),
        })
        .transpose()
}

fn usize_setting(option_name: &str, env_name: &str) -> Result<Option<usize>> {
    parse_setting(option_name, env_name)
}

fn nonzero(value: usize) -> Option<usize> {
    (value > 0).then_some(value)
}

fn validate_motion_config(config: &Config) -> Result<()> {
    let distinct: HashSet<char> = config.motion_hints.chars().collect();
    if distinct.len() < 2 {
        anyhow::bail!("motion_hints must contain at least two distinct characters");
    }
    if distinct
        .iter()
        .any(|ch| ch.is_control() || UnicodeWidthChar::width(*ch) != Some(1))
    {
        anyhow::bail!("motion_hints must contain printable single-column characters");
    }

    for (name, value) in [
        ("motion_vertical_border", &config.motion_vertical_border),
        ("motion_horizontal_border", &config.motion_horizontal_border),
    ] {
        if value.graphemes(true).count() != 1 || UnicodeWidthStr::width(value.as_str()) != 1 {
            anyhow::bail!("{name} must be exactly one display column");
        }
    }

    for (name, value) in [
        ("motion_hint1_fg", &config.motion_hint1_fg),
        ("motion_hint2_fg", &config.motion_hint2_fg),
        ("motion_dim", &config.motion_dim),
    ] {
        if value.is_empty() || !value.chars().all(|ch| ch.is_ascii_digit() || ch == ';') {
            anyhow::bail!("{name} must be a numeric SGR sequence");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{collect_tmux_options, parse_value, validate_motion_config};
    use crate::{
        config::Config,
        types::{ActionKind, CaseMode, Scope, SearchMode},
    };

    #[test]
    fn collect_tmux_options_strips_plugin_prefix() {
        let options = collect_tmux_options(vec![
            ("@tmux_nexus_scope".into(), "session".into()),
            ("@other_plugin_scope".into(), "all".into()),
        ]);

        assert_eq!(options.get("scope").map(String::as_str), Some("session"));
        assert!(!options.contains_key("other_plugin_scope"));
    }

    fn config() -> Config {
        Config {
            launch_key: "g".into(),
            motion_key: "s".into(),
            motion2_key: String::new(),
            motion_copy_mode_no_prefix: false,
            scope: Scope::All,
            include_history: true,
            history_lines: None,
            case_mode: CaseMode::Smart,
            join_wraps: true,
            skip_blank: true,
            preview: true,
            prompt_query: false,
            default_action: ActionKind::Jump,
            fzf_options: String::new(),
            search_mode: SearchMode::Literal,
            motion_hints: "asdf".into(),
            motion_case_mode: CaseMode::Insensitive,
            motion_smartsign: false,
            motion_vertical_border: "|".into(),
            motion_horizontal_border: "-".into(),
            motion_hint1_fg: "1;31".into(),
            motion_hint2_fg: "1;32".into(),
            motion_dim: "2".into(),
        }
    }

    #[test]
    fn rejects_unsafe_motion_configuration() {
        let mut value = config();
        value.motion_hints = "a".into();
        assert!(validate_motion_config(&value).is_err());

        let mut value = config();
        value.motion_hint1_fg = "1m\x1b[2J".into();
        assert!(validate_motion_config(&value).is_err());

        let mut value = config();
        value.motion_vertical_border = "你".into();
        assert!(validate_motion_config(&value).is_err());
    }

    #[test]
    fn reports_invalid_typed_settings() {
        let error = parse_value::<Scope>("typo", "TNX_SCOPE", "scope")
            .expect_err("invalid scope should fail")
            .to_string();
        assert!(error.contains("TNX_SCOPE"));
        assert!(error.contains("typo"));
    }
}
