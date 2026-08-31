use std::{
    ffi::{OsStr, OsString},
    io::Write,
    process::{Child, Command, Output, Stdio},
    sync::OnceLock,
};

use anyhow::{Context, Result};

use crate::util::version_at_least;

pub fn have(program: &str) -> bool {
    which::which(program).is_ok()
}

#[derive(Clone, Debug, Default)]
pub struct TmuxClient {
    args: Vec<OsString>,
}

impl TmuxClient {
    pub fn from_env() -> Result<Self> {
        Ok(Self::with_args(tmux_args_from_env()?))
    }

    pub fn with_args<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Self {
            args: args.into_iter().map(Into::into).collect(),
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new("tmux");
        command.args(&self.args);
        command
    }

    pub fn output<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<OsString> = args
            .into_iter()
            .map(|arg| arg.as_ref().to_os_string())
            .collect();
        self.command()
            .args(&args)
            .output()
            .with_context(|| format!("failed to execute tmux {}", display_args(&args)))
    }

    /// Start independent tmux commands together, then return stdout in input order.
    pub fn stdout_many(&self, commands: Vec<Vec<OsString>>) -> Result<Vec<String>> {
        let mut children = Vec::with_capacity(commands.len());
        for args in commands {
            match self
                .command()
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
            {
                Ok(child) => children.push((args, child)),
                Err(error) => {
                    terminate_children(children);
                    return Err(error).with_context(|| {
                        format!("failed to execute tmux {}", display_args(&args))
                    });
                }
            }
        }

        let mut outputs = Vec::with_capacity(children.len());
        let mut first_error = None;
        for (args, child) in children {
            match child.wait_with_output() {
                Ok(output) if output.status.success() => {
                    outputs.push(String::from_utf8_lossy(&output.stdout).to_string());
                }
                Ok(output) => {
                    if first_error.is_none() {
                        first_error = Some(anyhow::anyhow!(
                            "tmux {} failed: {}",
                            display_args(&args),
                            String::from_utf8_lossy(&output.stderr).trim()
                        ));
                    }
                    outputs.push(String::new());
                }
                Err(error) => {
                    if first_error.is_none() {
                        first_error =
                            Some(anyhow::Error::new(error).context(format!(
                                "failed to wait for tmux {}",
                                display_args(&args)
                            )));
                    }
                    outputs.push(String::new());
                }
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(outputs)
    }

    pub fn stdout<I, S>(&self, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.output(args)?;
        if !output.status.success() {
            anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    pub fn try_stdout<I, S>(&self, args: I) -> Option<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.output(args).ok()?;
        output.status.success().then(|| {
            String::from_utf8_lossy(&output.stdout)
                .trim_end()
                .to_string()
        })
    }

    pub fn run<I, S>(&self, args: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.output(args)?;
        if !output.status.success() {
            anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
        }
        Ok(())
    }

    pub fn run_with_input<I, S>(&self, args: I, input: &[u8]) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<OsString> = args
            .into_iter()
            .map(|arg| arg.as_ref().to_os_string())
            .collect();
        let mut child = self
            .command()
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to execute tmux {}", display_args(&args)))?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(input)?;
        }
        let output = child.wait_with_output()?;
        if !output.status.success() {
            anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
        }
        Ok(())
    }

    /// Run an ordered tmux command sequence through one client process.
    pub fn run_commands(&self, commands: &[Vec<OsString>]) -> Result<()> {
        let mut args = Vec::new();
        for command in commands.iter().filter(|command| !command.is_empty()) {
            if !args.is_empty() {
                args.push(";".into());
            }
            args.extend(command.iter().cloned());
        }
        if args.is_empty() {
            return Ok(());
        }
        self.run(args)
    }

    pub fn run_ignore<I, S>(&self, args: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let _ = self.output(args);
    }
}

fn terminate_children(children: Vec<(Vec<OsString>, Child)>) {
    for (_, mut child) in children {
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn display_args(args: &[OsString]) -> String {
    args.iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

fn tmux_args_from_env() -> Result<Vec<OsString>> {
    let Some(raw) = std::env::var_os("TNX_TMUX_ARGS") else {
        return Ok(Vec::new());
    };
    let raw = raw.to_string_lossy();
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    shell_words::split(&raw)
        .context("failed to parse TNX_TMUX_ARGS")
        .map(|parts| parts.into_iter().map(OsString::from).collect())
}

pub fn stdout<I, S>(args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    TmuxClient::from_env()?.stdout(args)
}

pub fn try_stdout<I, S>(args: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    TmuxClient::from_env().ok()?.try_stdout(args)
}

pub fn run<I, S>(args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    TmuxClient::from_env()?.run(args)
}

pub fn run_ignore<I, S>(args: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    if let Ok(client) = TmuxClient::from_env() {
        client.run_ignore(args);
    }
}

pub fn display_message(message: &str) {
    if have("tmux") {
        run_ignore(["display-message", message]);
    } else {
        eprintln!("{message}");
    }
}

pub fn show_option(name: &str) -> Option<String> {
    try_stdout(["show-option", "-gqv", name]).filter(|value| !value.is_empty())
}

pub fn show_option_allow_empty(name: &str) -> Option<String> {
    let output = TmuxClient::from_env()
        .ok()?
        .output(["show-option", "-g", name])
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&output.stdout);
    parse_show_option_line(line.trim()).map(|(_, value)| value)
}

pub fn show_options(prefix: &str) -> Result<Vec<(String, String)>> {
    let output = stdout(["show-options", "-gq"])?;
    Ok(output
        .lines()
        .filter_map(parse_show_option_line)
        .filter(|(name, _)| name.starts_with(prefix))
        .filter(|(_, value)| !value.is_empty())
        .collect())
}

fn parse_show_option_line(line: &str) -> Option<(String, String)> {
    let (name, value) = line.split_once(' ')?;
    Some((name.to_string(), unquote_option_value(value)))
}

fn unquote_option_value(value: &str) -> String {
    shell_words::split(value)
        .ok()
        .and_then(|mut parts| (parts.len() == 1).then(|| parts.remove(0)))
        .unwrap_or_else(|| value.to_string())
}

pub fn command_version(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn capture_flags_supported() -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        command_version("tmux", &["-V"]).is_some_and(|version| version_at_least(&version, 3, 7, 0))
    })
}

pub fn write_to_command(program: &str, args: &[&str], input: &str) -> Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to start {program}"))?;

    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(input.as_bytes())
            .with_context(|| format!("failed to write to {program}"))?;
    }

    let status = child.wait()?;
    if !status.success() {
        anyhow::bail!("{program} exited with status {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_show_option_line;

    #[test]
    fn parses_unquoted_tmux_option() {
        assert_eq!(
            parse_show_option_line("@tmux_nexus_scope all"),
            Some(("@tmux_nexus_scope".into(), "all".into()))
        );
    }

    #[test]
    fn parses_quoted_tmux_option() {
        assert_eq!(
            parse_show_option_line("@tmux_nexus_fzf_options \"--height 80%\""),
            Some(("@tmux_nexus_fzf_options".into(), "--height 80%".into()))
        );
    }
}
