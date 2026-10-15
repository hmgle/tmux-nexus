use std::{
    collections::HashMap,
    ffi::OsString,
    fs::File,
    io::{BufReader, BufWriter, Write},
    path::Path,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::Pane;
use crate::tmux;

const PANE_FORMAT: &str = concat!(
    "#{window_id}\t#{pane_id}\t#{window_zoomed_flag}\t#{pane_active}\t",
    "#{pane_top}\t#{pane_height}\t#{pane_left}\t#{pane_width}\t",
    "#{pane_in_mode}\t#{scroll_position}\t#{cursor_y}\t#{cursor_x}\t",
    "#{copy_cursor_y}\t#{copy_cursor_x}",
);

#[derive(Serialize, Deserialize)]
pub(super) struct MotionSnapshot {
    pub(super) panes: Vec<Pane>,
}

impl MotionSnapshot {
    pub(super) fn save(&self, path: &Path) -> Result<()> {
        let file =
            File::create(path).with_context(|| format!("failed to create {}", path.display()))?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer(&mut writer, self)?;
        writer.flush()?;
        Ok(())
    }

    pub(super) fn load(path: &Path) -> Result<Self> {
        let file =
            File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
        serde_json::from_reader(BufReader::new(file))
            .with_context(|| format!("failed to parse {}", path.display()))
    }
}

pub(super) fn init_panes(target_window: Option<&str>) -> Result<Vec<Pane>> {
    let client = tmux::TmuxClient::from_env()?;
    init_panes_with_client(&client, target_window)
}

pub(super) fn init_panes_with_client(
    client: &tmux::TmuxClient,
    target_window: Option<&str>,
) -> Result<Vec<Pane>> {
    let mut panes: Vec<Pane> = list_visible_panes(client, target_window)?
        .into_iter()
        .filter(|pane| pane.height > 0 && pane.width > 0)
        .collect();
    capture_panes(client, &mut panes)?;
    Ok(panes)
}

pub(super) fn refresh_panes(panes: Vec<Pane>) -> Result<Vec<Pane>> {
    let client = tmux::TmuxClient::from_env()?;
    refresh_panes_with_client(&client, panes)
}

pub(super) fn refresh_panes_with_client(
    client: &tmux::TmuxClient,
    panes: Vec<Pane>,
) -> Result<Vec<Pane>> {
    let Some(source) = panes.first() else {
        return Ok(panes);
    };
    // Read metadata in one request, then match the original IDs and order.
    // A newly created popup may be active; it must not enter the snapshot.
    let output = client.stdout(["list-panes", "-t", &source.window_id, "-F", PANE_FORMAT])?;
    let mut metadata: HashMap<_, _> = output
        .lines()
        .filter_map(parse_pane_line)
        .map(|pane| (pane.pane_id.clone(), pane))
        .collect();
    let mut refreshed = Vec::with_capacity(panes.len());
    for pane in panes {
        let mut current = metadata
            .remove(&pane.pane_id)
            .with_context(|| format!("motion source pane {} disappeared or moved", pane.pane_id))?;
        current.active = pane.active;
        if current.height > 0 && current.width > 0 {
            refreshed.push(current);
        }
    }
    capture_panes(client, &mut refreshed)?;
    Ok(refreshed)
}

fn capture_panes(client: &tmux::TmuxClient, panes: &mut [Pane]) -> Result<()> {
    let commands = panes.iter().map(capture_visible_args).collect();
    let outputs = client.stdout_many(commands)?;
    for (pane, output) in panes.iter_mut().zip(outputs) {
        pane.lines = parse_visible_capture(&output, pane.height);
    }
    Ok(())
}

fn list_visible_panes(client: &tmux::TmuxClient, target_window: Option<&str>) -> Result<Vec<Pane>> {
    let mut args: Vec<OsString> = vec!["list-panes".into(), "-F".into(), PANE_FORMAT.into()];
    if let Some(target_window) = target_window {
        args.push("-t".into());
        args.push(target_window.into());
    }
    let output = client.stdout(args)?;
    let zoomed = window_zoomed(&output);
    let panes = output
        .lines()
        .filter_map(parse_pane_line)
        .filter(|pane| pane.active || !zoomed)
        .collect();
    Ok(panes)
}

fn parse_pane_line(line: &str) -> Option<Pane> {
    let mut parts = line.splitn(14, '\t');
    let window_id = parts.next()?.to_string();
    let pane_id = parts.next()?.to_string();
    let _zoomed = parts.next()?;
    let active = parts.next()? == "1";
    let start_y = parse_usize(parts.next()?);
    let height = parse_usize(parts.next()?);
    let start_x = parse_usize(parts.next()?);
    let width = parse_usize(parts.next()?);
    let copy_mode = parts.next()? == "1";
    let scroll_position = parts.next()?.parse().unwrap_or_default();
    let cursor_y = parse_usize(parts.next()?);
    let cursor_x = parse_usize(parts.next()?);
    let copy_cursor_y = parse_usize(parts.next()?);
    let copy_cursor_x = parse_usize(parts.next()?);
    let (cursor_y, cursor_x) = if copy_mode {
        (copy_cursor_y, copy_cursor_x)
    } else {
        (cursor_y, cursor_x)
    };

    Some(Pane {
        window_id,
        pane_id,
        active,
        start_y,
        height,
        start_x,
        width,
        copy_mode,
        scroll_position,
        cursor_y,
        cursor_x,
        lines: Vec::new(),
    })
}

fn parse_usize(value: &str) -> usize {
    value.parse().unwrap_or_default()
}

fn window_zoomed(output: &str) -> bool {
    output
        .lines()
        .filter_map(|line| line.split('\t').nth(2))
        .any(|zoomed| zoomed == "1")
}

fn capture_visible_args(pane: &Pane) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["capture-pane".into(), "-p".into()];

    if pane.scroll_position > 0 {
        let start = format!("-{}", pane.scroll_position);
        let end = (-(pane.scroll_position - pane.height as isize + 1)).to_string();
        args.push("-S".into());
        args.push(start.into());
        args.push("-E".into());
        args.push(end.into());
    }
    args.push("-t".into());
    args.push(pane.pane_id.clone().into());
    args
}

fn parse_visible_capture(output: &str, height: usize) -> Vec<String> {
    let output = output.strip_suffix('\n').unwrap_or(output);
    output
        .split('\n')
        .take(height)
        .map(ToOwned::to_owned)
        .collect()
}

pub(super) fn window_size(target_window: Option<&str>) -> Result<(usize, usize)> {
    let mut args: Vec<OsString> = vec!["display-message".into(), "-p".into()];
    if let Some(target_window) = target_window {
        args.push("-t".into());
        args.push(target_window.into());
    }
    args.push("#{window_width},#{window_height}".into());
    let output = tmux::stdout(args)?;
    let (width, height) = output
        .trim()
        .split_once(',')
        .context("malformed tmux window size")?;
    Ok((parse_usize(width), parse_usize(height)))
}
