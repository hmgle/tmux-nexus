use std::{
    fs::{self, File},
    io::{BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const INDEX_VERSION: u32 = 3;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaneSnapshot {
    pub location: String,
    pub pane_id: String,
    pub command: String,
    pub window_name: String,
    #[serde(default)]
    pub history_start_line: usize,
    pub lines: Vec<String>,
}

impl PaneSnapshot {
    pub fn location(&self) -> &str {
        &self.location
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub pane_index: usize,
    pub line_index: usize,
    pub grid_line_index: Option<usize>,
}

impl Record {
    pub fn raw_line_no(&self) -> usize {
        self.grid_line_index.unwrap_or(self.line_index) + 1
    }

    pub fn grid_line_no(&self) -> Option<usize> {
        self.grid_line_index.map(|index| index + 1)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SearchIndex {
    pub version: u32,
    pub panes: Vec<PaneSnapshot>,
    pub records: Vec<Record>,
}

impl SearchIndex {
    pub fn load(path: &Path) -> Result<Self> {
        let file =
            File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
        let index: Self = serde_json::from_reader(BufReader::new(file))
            .with_context(|| format!("failed to parse {}", path.display()))?;
        if index.version != INDEX_VERSION {
            anyhow::bail!(
                "unsupported index version {} (expected {INDEX_VERSION})",
                index.version
            );
        }
        Ok(index)
    }

    pub fn record(&self, id: usize) -> Option<&Record> {
        self.records.get(id)
    }

    pub fn pane_for(&self, record: &Record) -> Option<&PaneSnapshot> {
        self.panes.get(record.pane_index)
    }

    pub fn text_for(&self, record: &Record) -> Option<&str> {
        self.pane_for(record)?
            .lines
            .get(record.line_index)
            .map(String::as_str)
    }

    pub fn save_preview_panes(&self, directory: &Path) -> Result<()> {
        fs::create_dir_all(directory)
            .with_context(|| format!("failed to create {}", directory.display()))?;
        for (pane_index, pane) in self.panes.iter().enumerate() {
            let path = preview_pane_path(directory, pane_index);
            let file = File::create(&path)
                .with_context(|| format!("failed to create {}", path.display()))?;
            let mut writer = BufWriter::new(file);
            serde_json::to_writer(&mut writer, pane)?;
            writer
                .flush()
                .with_context(|| format!("failed to write {}", path.display()))?;
        }
        Ok(())
    }

    pub fn load_preview_pane(directory: &Path, pane_index: usize) -> Result<PaneSnapshot> {
        let path = preview_pane_path(directory, pane_index);
        let file =
            File::open(&path).with_context(|| format!("failed to open {}", path.display()))?;
        serde_json::from_reader(BufReader::new(file))
            .with_context(|| format!("failed to parse {}", path.display()))
    }

    #[allow(dead_code)]
    pub fn legacy_tsv(&self, record: &Record) -> Option<String> {
        let pane = self.pane_for(record)?;
        let line_no = pane.history_start_line + record.raw_line_no();
        let text = self.text_for(record)?;
        Some(format!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            pane.pane_id, pane.location, pane.command, pane.window_name, line_no, text
        ))
    }
}

fn preview_pane_path(directory: &Path, pane_index: usize) -> PathBuf {
    directory.join(format!("pane-{pane_index}.json"))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyRecord {
    pub pane_id: String,
    pub location: String,
    pub command: String,
    pub window_name: String,
    pub line_no: usize,
    pub text: String,
}

impl LegacyRecord {
    pub fn parse(value: &str) -> Option<Self> {
        let mut parts = value.splitn(6, '\t');
        Some(Self {
            pane_id: parts.next()?.to_string(),
            location: parts.next()?.to_string(),
            command: parts.next()?.to_string(),
            window_name: parts.next()?.to_string(),
            line_no: parts.next()?.parse().ok()?,
            text: parts
                .next()
                .unwrap_or_default()
                .trim_end_matches('\r')
                .to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{LegacyRecord, PaneSnapshot, Record, SearchIndex};
    use tempfile::tempdir;

    #[test]
    fn parses_legacy_record_with_tabs_in_text() {
        let record = LegacyRecord::parse("%1\tmain:1.0\tzsh\tlogs\t7\talpha\tbeta").unwrap();
        assert_eq!(record.pane_id, "%1");
        assert_eq!(record.line_no, 7);
        assert_eq!(record.text, "alpha\tbeta");
    }

    #[test]
    fn record_lookup_uses_the_record_id_as_index() {
        let index = SearchIndex {
            panes: vec![PaneSnapshot {
                location: "s:1.0".into(),
                pane_id: "%1".into(),
                command: "zsh".into(),
                window_name: "main".into(),
                history_start_line: 0,
                lines: vec!["alpha".into()],
            }],
            records: vec![Record {
                pane_index: 0,
                line_index: 0,
                grid_line_index: Some(0),
            }],
            ..SearchIndex::default()
        };

        assert_eq!(
            index.record(0).and_then(|record| index.text_for(record)),
            Some("alpha")
        );
        assert!(index.record(1).is_none());
    }

    #[test]
    fn legacy_tsv_uses_absolute_history_line_number() {
        let index = SearchIndex {
            panes: vec![PaneSnapshot {
                location: "s:1.0".into(),
                pane_id: "%1".into(),
                command: "zsh".into(),
                window_name: "main".into(),
                history_start_line: 1200,
                lines: (1..=25)
                    .map(|line| {
                        if line == 25 {
                            "alpha".to_string()
                        } else {
                            String::new()
                        }
                    })
                    .collect(),
            }],
            records: vec![Record {
                pane_index: 0,
                line_index: 24,
                grid_line_index: Some(24),
            }],
            ..SearchIndex::default()
        };

        assert_eq!(
            index.legacy_tsv(&index.records[0]).as_deref(),
            Some("%1\ts:1.0\tzsh\tmain\t1225\talpha")
        );
    }

    #[test]
    fn preview_snapshot_loads_one_pane() {
        let directory = tempdir().unwrap();
        let index = SearchIndex {
            panes: vec![PaneSnapshot {
                location: "s:1.0".into(),
                pane_id: "%1".into(),
                command: "zsh".into(),
                window_name: "main".into(),
                history_start_line: 0,
                lines: vec!["alpha".into(), "beta".into()],
            }],
            ..SearchIndex::default()
        };

        index.save_preview_panes(directory.path()).unwrap();
        let pane = SearchIndex::load_preview_pane(directory.path(), 0).unwrap();
        assert_eq!(pane.location(), "s:1.0");
        assert_eq!(pane.lines, vec!["alpha", "beta"]);
    }
}
