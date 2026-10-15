//! Supervise hint windows without relying on a shell to signal completion.

use std::{
    ffi::OsString,
    path::Path,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use tempfile::{Builder, NamedTempFile};

use super::{overlay_args, MotionArgs};
use crate::tmux::TmuxClient;

const CLIENT_FORMAT: &str = "#{client_name}\t#{client_pid}\t#{window_id}";
const PANE_FORMAT: &str = "#{pane_dead}\t#{pane_dead_status}\t#{pane_dead_signal}\t#{pane_title}";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
// The overlay sets this length only after a successful return, including
// terminal cleanup. A failed or killed overlay still needs liveness checks.
const FINISHED: u64 = 2;

pub(super) fn finish(path: &Path) -> Result<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|file| file.set_len(FINISHED))
        .context("failed to report motion completion")?;
    // Keep the displayed pane alive until the supervisor unlinks it. Exiting
    // here would briefly render tmux's dead-pane banner over the hint frame.
    while path.exists() {
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

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
    fn supervise(&self, ready_title: &str, ready: &NamedTempFile) -> Result<()> {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        let mut next_check = Instant::now();
        let mut shown = false;
        loop {
            let state = ready.as_file().metadata()?.len();
            if state == FINISHED {
                return Ok(());
            }
            // Poll local readiness/completion between periodic tmux liveness
            // checks. Normal completion must not wait for the next 50ms poll.
            if (shown || state == 0) && Instant::now() < next_check {
                thread::sleep(Duration::from_millis(2));
                continue;
            }
            next_check = Instant::now() + Duration::from_millis(50);
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
                // Never pull a client back after it has deliberately
                // selected another window.
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
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn cleanup(&mut self) -> Result<()> {
        let clients = self
            .client
            .try_stdout(["list-clients", "-F", CLIENT_FORMAT]);
        let mut commands = Vec::new();
        if clients
            .as_deref()
            .and_then(|s| self.owner.current(s))
            .is_some_and(|owner| owner.window == self.window)
        {
            commands.push(strings(&[
                "switch-client",
                "-c",
                &self.owner.name,
                "-t",
                &self.source,
            ]));
        }
        let remove = self
            .client
            .try_stdout([
                "display-message",
                "-p",
                "-t",
                &self.linked_target,
                "#{window_id}",
            ])
            .as_deref()
            == Some(&self.window);
        if remove {
            // Unlike kill-window, unlink does not renumber the user's windows.
            commands.push(strings(&["unlink-window", "-k", "-t", &self.linked_target]));
        }
        // Expose the final cursor and remove the hint window in one server
        // command sequence, avoiding a second full redraw after returning.
        let result = self.client.run_commands(&commands);
        self.cleaned = result.is_ok();
        if result.is_err() && remove {
            // Still remove the overlay if restoring a vanished source fails.
            self.cleaned = self
                .client
                .run(["unlink-window", "-k", "-t", &self.linked_target])
                .is_ok();
        }
        result
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
    let ready = Builder::new().prefix("tnx_motion_ready.").tempfile()?;
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
    spawn.extend(["--ready-file".into(), ready.path().as_os_str().to_owned()]);
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
        .and_then(|()| window.supervise(&title, &ready));
    let cleanup = window.cleanup();
    result.and(cleanup)
}

fn strings(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}
