//! Supervise hint windows without relying on a shell to signal completion.

use std::{
    ffi::OsString,
    path::Path,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};

use super::{overlay_args, MotionArgs};
use crate::tmux::TmuxClient;

const CLIENT_FORMAT: &str = "#{client_name}\t#{client_pid}\t#{window_id}";
const PANE_FORMAT: &str = "#{pane_dead}\t#{pane_dead_status}\t#{pane_dead_signal}\t#{pane_title}";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);

struct Owner {
    name: String,
    pid: String,
    window: String,
}

impl Owner {
    fn find(output: &str, name: &str) -> Option<Self> {
        output.lines().find_map(|line| {
            let mut fields = line.split('\t');
            let client = fields.next()?;
            let pid = fields.next()?;
            let window = fields.next()?;
            (client == name).then(|| Self {
                name: client.into(),
                pid: pid.into(),
                window: window.into(),
            })
        })
    }

    fn current(&self, output: &str) -> Option<Self> {
        Self::find(output, &self.name).filter(|client| client.pid == self.pid)
    }
}

struct HintWindow {
    client: TmuxClient,
    owner: Owner,
    source: String,
    window: String,
    pane: String,
    linked_target: String,
    cleaned: bool,
}

impl HintWindow {
    fn supervise(&self, ready_title: &str) -> Result<()> {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        let mut shown = false;
        loop {
            // One tmux request reads both the process state and its owner.
            // A killed pane/window (or server) is cancellation, not a wait
            // for a completion signal which can no longer arrive.
            let Some(output) = self.client.try_stdout([
                "display-message",
                "-p",
                "-t",
                &self.pane,
                PANE_FORMAT,
                ";",
                "list-clients",
                "-F",
                CLIENT_FORMAT,
            ]) else {
                return Ok(());
            };
            let (pane, clients) = output.split_once('\n').unwrap_or((&output, ""));
            let Some(owner) = self.owner.current(clients) else {
                return Ok(());
            };
            let expected = if shown {
                &self.window
            } else {
                &self.owner.window
            };
            if &owner.window != expected {
                // Navigation also returns to the source. Never pull a client
                // back after it has deliberately selected another window.
                return Ok(());
            }
            let mut fields = pane.splitn(4, '\t');
            let dead = fields.next() == Some("1");
            let status = fields.next().unwrap_or_default();
            let signal = fields.next().unwrap_or_default();
            let title = fields.next().unwrap_or_default();
            if dead {
                if status == "0" {
                    return Ok(());
                }
                if let Some(error) = self
                    .client
                    .try_stdout(["show-option", "-pqv", "-t", &self.pane, "@tnx_motion_error"])
                    .filter(|error| !error.is_empty())
                {
                    anyhow::bail!("{error}");
                }
                anyhow::bail!("motion overlay exited (status {status}, signal {signal})");
            }
            if !shown {
                if title == ready_title {
                    self.client.run([
                        "switch-client",
                        "-c",
                        &self.owner.name,
                        "-t",
                        &self.window,
                    ])?;
                    shown = true;
                } else {
                    anyhow::ensure!(
                        Instant::now() < deadline,
                        "motion overlay startup timed out"
                    );
                }
            }
            thread::sleep(if shown {
                Duration::from_millis(50)
            } else {
                Duration::from_millis(5)
            });
        }
    }

    fn cleanup(&mut self) -> Result<()> {
        let clients = self
            .client
            .try_stdout(["list-clients", "-F", CLIENT_FORMAT]);
        let restore = if clients
            .as_deref()
            .and_then(|s| self.owner.current(s))
            .is_some_and(|owner| owner.window == self.window)
        {
            self.client
                .run(["switch-client", "-c", &self.owner.name, "-t", &self.source])
        } else {
            Ok(())
        };
        let remove = if self
            .client
            .try_stdout([
                "display-message",
                "-p",
                "-t",
                &self.linked_target,
                "#{window_id}",
            ])
            .as_deref()
            == Some(&self.window)
        {
            // Unlike kill-window, unlink does not renumber the user's windows.
            self.client
                .run(["unlink-window", "-k", "-t", &self.linked_target])
        } else {
            Ok(())
        };
        self.cleaned = remove.is_ok();
        restore.and(remove)
    }
}

impl Drop for HintWindow {
    fn drop(&mut self) {
        if !self.cleaned {
            let _ = self.cleanup();
        }
    }
}

pub(super) fn run(
    exe: &str,
    args: &MotionArgs,
    pattern: &str,
    target_window: &str,
    target_pane: &str,
    target_client: Option<&str>,
    snapshot_path: &Path,
) -> Result<()> {
    let client = TmuxClient::from_env()?;
    let name = target_client.context("motion requires an attached tmux client")?;
    let initial = client.stdout([
        "display-message",
        "-p",
        "-t",
        target_pane,
        "#{session_id}\t#{window_width}\t#{window_height}",
        ";",
        "list-clients",
        "-F",
        CLIENT_FORMAT,
    ])?;
    let (source, clients) = initial
        .split_once('\n')
        .context("missing motion source metadata")?;
    let mut source = source.split('\t');
    let session = source.next().context("missing motion source session")?;
    let width = source.next().context("missing motion source width")?;
    let height = source.next().context("missing motion source height")?;
    let owner = Owner::find(clients, name).context("motion client is no longer attached")?;
    // A short-lived helper keeps the pane alive while it is configured.
    // Older floating-pane tmux versions do not support empty new-window.
    // Both helper and overlay use argv directly, never default-shell.
    let created = client.stdout([
        "new-window",
        "-d",
        "-P",
        "-F",
        "#{window_id}\t#{pane_id}",
        "-t",
        &format!("{session}:"),
        "-n",
        "motion",
        "/bin/sleep",
        "10",
    ])?;
    let (window, pane) = created
        .lines()
        .next()
        .unwrap_or_default()
        .split_once('\t')
        .context("malformed motion window identifiers")?;
    let mut window = HintWindow {
        linked_target: format!("{session}:{window}"),
        client,
        owner,
        source: target_window.into(),
        window: window.into(),
        pane: pane.into(),
        cleaned: false,
    };
    let title = format!("tnx-motion-ready-{}", std::process::id());
    let mut spawn = strings(&["respawn-pane", "-k", "-t", &window.pane]);
    if let Ok(value) = std::env::var("TNX_TMUX_ARGS") {
        spawn.extend(["-e".into(), format!("TNX_TMUX_ARGS={value}").into()]);
    }
    spawn.extend(
        overlay_args(
            exe,
            args,
            pattern,
            target_window,
            target_client,
            snapshot_path,
        )
        .into_iter()
        .map(OsString::from),
    );
    spawn.extend(["--ready-title".into(), title.clone().into()]);
    let result = window
        .client
        .run_commands(&[
            strings(&[
                "set-option",
                "-p",
                "-t",
                &window.pane,
                "remain-on-exit",
                "on",
            ]),
            strings(&[
                "set-option",
                "-p",
                "-t",
                &window.pane,
                "allow-set-title",
                "on",
            ]),
            strings(&[
                "set-option",
                "-w",
                "-t",
                &window.window,
                "window-size",
                "manual",
            ]),
            strings(&[
                "set-option",
                "-w",
                "-t",
                &window.window,
                "pane-border-status",
                "off",
            ]),
            strings(&[
                "resize-window",
                "-t",
                &window.window,
                "-x",
                width,
                "-y",
                height,
            ]),
            spawn,
        ])
        .and_then(|()| window.supervise(&title));
    let cleanup = window.cleanup();
    result.and(cleanup)
}

fn strings(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}
