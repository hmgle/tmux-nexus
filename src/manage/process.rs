use std::{collections::HashMap, process::Command};

use anyhow::{Context, Result};

use super::{choose, ManagerContext, Row};
use crate::tmux;

#[derive(Clone, Debug)]
struct ProcessEntry {
    row: Row,
    uid: u32,
    pid: u32,
    started: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ProcessIdentity {
    uid: u32,
    started: String,
}

pub(super) fn run(action: Option<&str>, context: &ManagerContext) -> Result<()> {
    let mut actions = vec![
        "display",
        "terminate",
        "kill",
        "interrupt",
        "continue",
        "stop",
        "quit",
        "hangup",
    ];
    if tmux::have("pstree") {
        actions.insert(1, "tree");
    }
    let action = super::resolve_action(action, &actions, context, "process action> ")?;
    if action.is_empty() {
        return Ok(());
    }
    let processes = process_rows()?;
    let rows: Vec<Row> = processes
        .iter()
        .map(|process| process.row.clone())
        .collect();
    let multi = !matches!(action.as_str(), "display" | "tree");
    let indexes = choose(
        &rows,
        context,
        "process> ",
        "Select a process; TAB selects multiple signal targets",
        multi,
        None,
    )?;
    if indexes.is_empty() {
        return Ok(());
    }
    if action == "display" {
        return display_process(context, processes[indexes[0]].pid);
    }
    if action == "tree" {
        return popup_or_split(
            context,
            &format!("pstree -p {}", processes[indexes[0]].pid),
            "70%",
            "70%",
        );
    }
    let (signal_name, signal_number) = match action.as_str() {
        "terminate" => ("TERM", libc::SIGTERM),
        "kill" => ("KILL", libc::SIGKILL),
        "interrupt" => ("INT", libc::SIGINT),
        "continue" => ("CONT", libc::SIGCONT),
        "stop" => ("STOP", libc::SIGSTOP),
        "quit" => ("QUIT", libc::SIGQUIT),
        "hangup" => ("HUP", libc::SIGHUP),
        _ => unreachable!(),
    };
    let selected: Vec<&ProcessEntry> = indexes.iter().map(|index| &processes[*index]).collect();
    signal_processes(context, signal_name, signal_number, &selected)
}

fn process_rows() -> Result<Vec<ProcessEntry>> {
    let output = command_stdout(
        "ps",
        [
            "-eo",
            "uid=,user=,pid=,lstart=,ppid=,stat=,%cpu=,%mem=,command=",
        ],
    )?;
    let mut processes: Vec<ProcessEntry> = output.lines().filter_map(parse_process_entry).collect();
    for process in &mut processes {
        if let Some(started) = process_start_identity(process.pid) {
            process.started = started;
        }
    }
    Ok(processes)
}

fn parse_process_entry(line: &str) -> Option<ProcessEntry> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    let uid: u32 = fields.first()?.parse().ok()?;
    let pid: u32 = fields.get(2)?.parse().ok()?;
    let started = fields.get(3..8)?.join(" ");
    Some(ProcessEntry {
        row: Row::new(pid.to_string(), line.trim()),
        uid,
        pid,
        started,
    })
}

fn signal_processes(
    context: &ManagerContext,
    signal_name: &str,
    signal_number: i32,
    processes: &[&ProcessEntry],
) -> Result<()> {
    if !confirm_action(
        context,
        &format!("Send {signal_name} to selected process(es)?"),
    )? {
        return Ok(());
    }
    let current_uid = unsafe { libc::geteuid() };
    let privilege = ["sudo", "doas"]
        .into_iter()
        .find(|program| tmux::have(program));
    let current = current_process_identities(processes)?;
    let mut succeeded = Vec::new();
    let mut failures = Vec::new();
    for process in processes {
        let Some(identity) = current.get(&process.pid) else {
            failures.push(format!("{} exited", process.pid));
            continue;
        };
        if identity.uid != process.uid || identity.started != process.started {
            failures.push(format!("{} changed identity", process.pid));
            continue;
        }
        if let Err(error) = revalidate_process(process) {
            failures.push(format!("{}: {error:#}", process.pid));
            continue;
        }
        let result: Result<()> = if process.uid == current_uid {
            signal_same_uid(process, signal_number)
        } else if let Some(program) = privilege {
            match Command::new(program)
                .args(["kill", "-s", signal_name, &process.pid.to_string()])
                .status()
            {
                Ok(status) if status.success() => Ok(()),
                Ok(status) => Err(anyhow::anyhow!("{program} exited with status {status}")),
                Err(error) => Err(error).with_context(|| format!("failed to start {program}")),
            }
        } else {
            Err(anyhow::anyhow!(
                "install sudo or doas to signal UID {}",
                process.uid
            ))
        };
        match result {
            Ok(()) => succeeded.push(process.pid),
            Err(error) => failures.push(format!("{}: {error:#}", process.pid)),
        }
    }
    if failures.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "sent {signal_name} to {} process(es); failed for {}",
        succeeded.len(),
        failures.join(", ")
    )
}

fn current_process_identities(
    processes: &[&ProcessEntry],
) -> Result<HashMap<u32, ProcessIdentity>> {
    const PS_PID_BATCH: usize = 512;
    let mut identities = HashMap::new();
    for batch in processes.chunks(PS_PID_BATCH) {
        let pids = batch
            .iter()
            .map(|process| process.pid.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let output = Command::new("ps")
            .args(["-o", "uid=,pid=,lstart=", "-p", pids.as_str()])
            .output()
            .context("failed to revalidate selected processes")?;
        if !output.status.success() && output.status.code() != Some(1) {
            anyhow::bail!(
                "ps failed while revalidating processes: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        identities.extend(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| {
                    let (pid, mut identity) = parse_process_identity(line)?;
                    if let Some(started) = process_start_identity(pid) {
                        identity.started = started;
                    }
                    Some((pid, identity))
                }),
        );
    }
    Ok(identities)
}

fn parse_process_identity(line: &str) -> Option<(u32, ProcessIdentity)> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    let uid: u32 = fields.first()?.parse().ok()?;
    let pid: u32 = fields.get(1)?.parse().ok()?;
    let started = fields.get(2..7)?.join(" ");
    Some((pid, ProcessIdentity { uid, started }))
}

#[cfg(target_os = "linux")]
fn process_start_identity(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let command_end = stat.rfind(')')?;
    let start_ticks = stat.get(command_end + 2..)?.split_whitespace().nth(19)?;
    Some(format!("linux:{start_ticks}"))
}

#[cfg(not(target_os = "linux"))]
fn process_start_identity(_pid: u32) -> Option<String> {
    None
}

fn revalidate_process(process: &ProcessEntry) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let current = process_start_identity(process.pid)
            .context("process exited during final identity check")?;
        if current != process.started {
            anyhow::bail!("process changed identity during final identity check");
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = process;
    Ok(())
}

#[cfg(target_os = "linux")]
fn signal_same_uid(process: &ProcessEntry, signal_number: i32) -> Result<()> {
    let pid = libc::pid_t::try_from(process.pid)
        .map_err(|_| anyhow::anyhow!("PID is outside the platform range"))?;
    let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if pidfd < 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ENOSYS) {
            revalidate_process(process)?;
            let status = unsafe { libc::kill(pid, signal_number) };
            return if status == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error()).context("process signal failed")
            };
        }
        return Err(error).context("failed to open process pidfd");
    }
    let result = (|| {
        let current = process_start_identity(process.pid)
            .context("process exited while opening its pidfd")?;
        if current != process.started {
            anyhow::bail!("process changed identity while opening its pidfd");
        }
        let status = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                pidfd,
                signal_number,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if status == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error()).context("pidfd signal failed")
        }
    })();
    unsafe { libc::close(pidfd as libc::c_int) };
    result
}

#[cfg(not(target_os = "linux"))]
fn signal_same_uid(process: &ProcessEntry, signal_number: i32) -> Result<()> {
    let pid = libc::pid_t::try_from(process.pid)
        .map_err(|_| anyhow::anyhow!("PID is outside the platform range"))?;
    let status = unsafe { libc::kill(pid, signal_number) };
    if status == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error()).context("process signal failed")
    }
}

fn confirm_action(context: &ManagerContext, prompt: &str) -> Result<bool> {
    if !context.config.confirm {
        return Ok(true);
    }
    let rows = [Row::new("yes", "yes"), Row::new("no", "no")];
    Ok(choose(&rows, context, "confirm> ", prompt, false, None)?
        .first()
        .is_some_and(|index| rows[*index].id == "yes"))
}

fn display_process(context: &ManagerContext, pid: u32) -> Result<()> {
    let command = if cfg!(target_os = "macos") {
        format!("top -pid {pid}")
    } else {
        format!("top -p {pid}")
    };
    popup_or_split(context, &command, "70%", "70%")
}

fn popup_or_split(
    context: &ManagerContext,
    command: &str,
    width: &str,
    height: &str,
) -> Result<()> {
    if context.picker.tmux_popup {
        context.tmux.run([
            "display-popup",
            "-E",
            "-w",
            width,
            "-h",
            height,
            "-d",
            "#{pane_current_path}",
            command,
        ])
    } else {
        context.tmux.run([
            "split-window",
            "-v",
            "-l",
            "50%",
            "-c",
            "#{pane_current_path}",
            command,
        ])
    }
}

fn command_stdout<I, S>(program: &str, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = Command::new(program).args(args).output()?;
    if !output.status.success() {
        anyhow::bail!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::{parse_process_entry, parse_process_identity};

    #[test]
    fn parses_numeric_process_identity_and_start_time() {
        let process =
            parse_process_entry("1000 alice 42 Sun Jul 12 19:52:57 2026 1 S 0.0 0.1 sleep 60")
                .unwrap();
        assert_eq!(process.uid, 1000);
        assert_eq!(process.pid, 42);
        assert_eq!(process.started, "Sun Jul 12 19:52:57 2026");

        let (pid, identity) = parse_process_identity("1000 42 Sun Jul 12 19:52:57 2026").unwrap();
        assert_eq!(pid, 42);
        assert_eq!(identity.uid, 1000);
        assert_eq!(identity.started, process.started);
    }
}
