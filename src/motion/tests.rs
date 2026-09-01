use super::{
    capture::{init_panes_with_client, MotionSnapshot},
    hints::{assign_hints_by_distance, generate_hints, hint_positions},
    matching::{find_matches, smartsign_patterns},
    navigation::move_cursor_with_client,
    popup_command,
    rendering::{expand_tabs, string_width, true_position, visual_slice},
    terminal::read_key,
    tmux_tab_mode_from_version, HintTarget, Match, MotionArgs, MotionKind, Pane, TabMode,
};
use crate::{tmux, types::CaseMode};
use std::{
    path::Path,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::Duration,
};

static NEXT_SOCKET_ID: AtomicUsize = AtomicUsize::new(0);

fn pane(lines: &[&str]) -> Pane {
    Pane {
        window_id: "@1".into(),
        pane_id: "%1".into(),
        active: true,
        start_y: 0,
        height: 10,
        start_x: 0,
        width: 80,
        copy_mode: false,
        scroll_position: 0,
        cursor_y: 0,
        cursor_x: 0,
        lines: lines.iter().map(|line| (*line).to_string()).collect(),
    }
}

struct TestTmux {
    socket: String,
    pane_id: String,
    width: usize,
    height: usize,
}

impl TestTmux {
    fn start(command: &str, width: usize, height: usize) -> Self {
        assert!(tmux::have("tmux"), "tmux is required for integration tests");

        let socket = format!(
            "tnx-motion-test-{}-{}",
            std::process::id(),
            NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed)
        );
        let output = Command::new("tmux")
            .arg("-L")
            .arg(&socket)
            .arg("new-session")
            .arg("-d")
            .arg("-x")
            .arg(width.to_string())
            .arg("-y")
            .arg(height.to_string())
            .arg("-P")
            .arg("-F")
            .arg("#{pane_id}")
            .arg(command)
            .output()
            .expect("failed to start test tmux server");
        assert!(
            output.status.success(),
            "failed to start test tmux server: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let pane_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        assert!(!pane_id.is_empty(), "test tmux returned no pane id");

        Self {
            socket,
            pane_id,
            width,
            height,
        }
    }

    fn direct_stdout(socket: &str, args: &[&str]) -> Option<String> {
        let output = Command::new("tmux")
            .arg("-L")
            .arg(socket)
            .args(args)
            .output()
            .ok()?;
        output.status.success().then(|| {
            String::from_utf8_lossy(&output.stdout)
                .trim_end()
                .to_string()
        })
    }

    fn stdout(&self, args: &[&str]) -> String {
        Self::direct_stdout(&self.socket, args)
            .unwrap_or_else(|| panic!("tmux command failed: {args:?}"))
    }

    fn client(&self) -> tmux::TmuxClient {
        tmux::TmuxClient::with_args(["-L", self.socket.as_str()])
    }

    fn split_window(&self, command: &str) -> String {
        self.stdout(&[
            "split-window",
            "-t",
            self.pane_id.as_str(),
            "-P",
            "-F",
            "#{pane_id}",
            command,
        ])
    }

    fn capture_lines(&self, pane_id: &str) -> Vec<String> {
        let output = self.stdout(&["capture-pane", "-p", "-t", pane_id]);
        output
            .strip_suffix('\n')
            .unwrap_or(&output)
            .split('\n')
            .take(self.height)
            .map(ToOwned::to_owned)
            .collect()
    }

    fn wait_for_lines(&self, pane_id: &str, needle: &str) -> Vec<String> {
        for _ in 0..50 {
            let lines = self.capture_lines(pane_id);
            if lines.iter().any(|line| line.contains(needle)) {
                return lines;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("tmux pane did not contain {needle:?}");
    }

    fn pane(&self, pane_id: &str, lines: Vec<String>) -> Pane {
        let window_id = self.stdout(&["display-message", "-p", "-t", pane_id, "#{window_id}"]);
        Pane {
            window_id,
            pane_id: pane_id.to_string(),
            active: pane_id == self.active_pane(),
            start_y: 0,
            height: self.height,
            start_x: 0,
            width: self.width,
            copy_mode: false,
            scroll_position: 0,
            cursor_y: 0,
            cursor_x: 0,
            lines,
        }
    }

    fn active_pane(&self) -> String {
        self.stdout(&["list-panes", "-F", "#{pane_active}\t#{pane_id}"])
            .lines()
            .find_map(|line| {
                let (active, pane_id) = line.split_once('\t')?;
                (active == "1").then(|| pane_id.to_string())
            })
            .unwrap_or_default()
    }

    fn copy_cursor(&self, pane_id: &str) -> (usize, usize) {
        let output = self.stdout(&[
            "display-message",
            "-p",
            "-t",
            pane_id,
            "#{copy_cursor_x},#{copy_cursor_y}",
        ]);
        let (x, y) = output.split_once(',').expect("copy cursor position");
        (x.parse().unwrap(), y.parse().unwrap())
    }
}

impl Drop for TestTmux {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .arg("-L")
            .arg(&self.socket)
            .arg("kill-server")
            .output();
    }
}

#[test]
fn detects_tmux_tab_modes() {
    assert_eq!(tmux_tab_mode_from_version("tmux 3.5a"), TabMode::Fixed);
    assert_eq!(
        tmux_tab_mode_from_version("tmux 3.6"),
        TabMode::PositionAware
    );
    assert_eq!(
        tmux_tab_mode_from_version("tmux next-3.7"),
        TabMode::PositionAware
    );
    assert_eq!(tmux_tab_mode_from_version("tmux master"), TabMode::Fixed);
    assert_eq!(
        tmux_tab_mode_from_version("tmux openbsd-6.6"),
        TabMode::Fixed
    );
}

#[test]
fn calculates_cjk_and_tab_widths() {
    assert_eq!(string_width("a", TabMode::PositionAware), 1);
    assert_eq!(string_width("你", TabMode::PositionAware), 2);
    assert_eq!(string_width("a\tb", TabMode::PositionAware), 9);
    assert_eq!(string_width("a\tb", TabMode::Fixed), 10);
    assert_eq!(expand_tabs("a\tb", TabMode::PositionAware), "a       b");
    assert_eq!(expand_tabs("a\tb", TabMode::Fixed), "a        b");
}

#[test]
fn converts_visual_column_to_true_position() {
    assert_eq!(true_position("a你b", 0, TabMode::PositionAware), 0);
    assert_eq!(true_position("a你b", 1, TabMode::PositionAware), 1);
    assert_eq!(true_position("a你b", 3, TabMode::PositionAware), 2);
    assert_eq!(true_position("a\tb", 8, TabMode::PositionAware), 2);
    assert_eq!(true_position("a\tb", 8, TabMode::Fixed), 2);
    assert_eq!(true_position("a\tb", 9, TabMode::Fixed), 2);
}

#[test]
fn visual_slice_pads_and_avoids_splitting_wide_chars() {
    assert_eq!(visual_slice("ab", 4, TabMode::PositionAware), "ab  ");
    assert_eq!(visual_slice("a你b", 2, TabMode::PositionAware), "a ");
    assert_eq!(visual_slice("a你b", 3, TabMode::PositionAware), "a你");
}

#[test]
fn generates_non_ambiguous_hints() {
    assert_eq!(generate_hints("asdf", 4), vec!["a", "s", "d", "f"]);
    let hints = generate_hints("asdf", 7);
    assert_eq!(hints.len(), 7);
    assert!(!hints
        .iter()
        .any(|hint| hint.len() == 2 && hint.starts_with('a')));
}

#[test]
fn generates_hints_beyond_two_keys() {
    let hints = generate_hints("ab", 7);

    assert_eq!(hints.len(), 7);
    assert!(hints.iter().any(|hint| hint.chars().count() == 3));
    for (idx, hint) in hints.iter().enumerate() {
        assert!(!hints
            .iter()
            .enumerate()
            .any(|(other_idx, other)| { idx != other_idx && other.starts_with(hint) }));
    }
}

#[test]
fn generate_hints_ignores_duplicate_keys() {
    assert_eq!(generate_hints("aabb", 2), vec!["a", "b"]);
}

#[test]
fn generate_hints_rejects_an_insufficient_alphabet() {
    assert!(generate_hints("a", 2).is_empty());
    assert!(generate_hints("aaaa", 3).is_empty());
}

#[test]
fn read_key_reads_multibyte_utf8() {
    let mut input = "你".as_bytes();

    assert_eq!(read_key(&mut input).expect("utf-8 key"), '你');
}

#[test]
fn read_key_returns_ctrl_c_for_graceful_cancellation() {
    let mut input = [0x03].as_slice();
    assert_eq!(read_key(&mut input).unwrap(), '\u{3}');
}

#[test]
fn motion_snapshot_round_trips() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let snapshot = MotionSnapshot {
        panes: vec![pane(&["alpha"])],
        matches: vec![Match {
            pane_index: 0,
            line_no: 0,
            visual_col: 0,
        }],
    };
    snapshot.save(file.path()).unwrap();

    let loaded = MotionSnapshot::load(file.path()).unwrap();
    assert_eq!(loaded.panes[0].lines, vec!["alpha"]);
    assert_eq!(loaded.matches[0].visual_col, 0);
}

#[test]
fn expands_smartsign_patterns() {
    assert_eq!(smartsign_patterns("3", true), vec!["3", "#"]);
    assert_eq!(smartsign_patterns("3x", true), vec!["3x", "#x"]);
    assert_eq!(smartsign_patterns("ab", false), vec!["ab"]);
}

#[test]
fn finds_matches_with_visual_columns() {
    let panes = vec![pane(&["a\tb", "你好 hello"])];
    let matches = find_matches(
        &panes,
        "b",
        CaseMode::Insensitive,
        false,
        TabMode::PositionAware,
    );
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].visual_col, 8);

    let matches = find_matches(&panes, "b", CaseMode::Insensitive, false, TabMode::Fixed);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].visual_col, 9);

    let matches = find_matches(
        &panes,
        "he",
        CaseMode::Insensitive,
        false,
        TabMode::PositionAware,
    );
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].visual_col, 5);
}

#[test]
fn finds_matches_only_on_grapheme_boundaries() {
    let panes = vec![pane(&["👍🏽 thumbs", "👍 plain"])];

    let matches = find_matches(
        &panes,
        "🏽",
        CaseMode::Insensitive,
        false,
        TabMode::PositionAware,
    );
    assert!(matches.is_empty());

    let matches = find_matches(
        &panes,
        "👍",
        CaseMode::Insensitive,
        false,
        TabMode::PositionAware,
    );
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].line_no, 1);
    assert_eq!(matches[0].visual_col, 0);

    let matches = find_matches(
        &panes,
        "👍🏽",
        CaseMode::Insensitive,
        false,
        TabMode::PositionAware,
    );
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].line_no, 0);
    assert_eq!(matches[0].visual_col, 0);
}

#[test]
fn hint_positions_restore_whole_graphemes() {
    let panes = vec![pane(&["👍🏽x"])];
    let positions = hint_positions(
        &panes,
        &[HintTarget {
            hint: "a".into(),
            target: Match {
                pane_index: 0,
                line_no: 0,
                visual_col: 0,
            },
        }],
        TabMode::PositionAware,
    );

    assert_eq!(positions.len(), 1);
    assert_eq!(positions[0].original, "👍🏽");
}

#[test]
fn hint_positions_restore_expanded_tabs() {
    let panes = vec![pane(&["a\tb"])];
    let positions = hint_positions(
        &panes,
        &[HintTarget {
            hint: "a".into(),
            target: Match {
                pane_index: 0,
                line_no: 0,
                visual_col: 1,
            },
        }],
        TabMode::PositionAware,
    );

    assert_eq!(positions.len(), 1);
    assert_eq!(positions[0].original, "       ");
}

#[test]
fn hint_positions_do_not_restore_wide_cells_across_pane_edges() {
    let mut pane = pane(&["你"]);
    pane.width = 1;
    let positions = hint_positions(
        &[pane],
        &[HintTarget {
            hint: "a".into(),
            target: Match {
                pane_index: 0,
                line_no: 0,
                visual_col: 0,
            },
        }],
        TabMode::PositionAware,
    );

    assert_eq!(positions[0].original, " ");
}

#[test]
fn smartsign_matching_finds_shifted_symbol() {
    let panes = vec![pane(&["test 3# code"])];
    let matches = find_matches(
        &panes,
        "3",
        CaseMode::Sensitive,
        true,
        TabMode::PositionAware,
    );
    assert_eq!(matches.len(), 2);
}

#[test]
fn assigns_short_hints_to_closer_matches() {
    let pane = pane(&["aaaaaaaaaa"]);
    let matches = vec![
        Match {
            pane_index: 0,
            line_no: 0,
            visual_col: 8,
        },
        Match {
            pane_index: 0,
            line_no: 0,
            visual_col: 1,
        },
    ];
    let panes = vec![pane];
    let targets = assign_hints_by_distance(&panes, &matches, 0, 0, "ab");
    assert_eq!(targets[0].target.visual_col, 1);
    assert_eq!(targets[0].hint, "a");
}

#[test]
fn popup_command_targets_originating_client() {
    let args = MotionArgs {
        kind: MotionKind::S,
        pattern: None,
        query: None,
        query_option: None,
        target_window: None,
        target_client: None,
        overlay: false,
        snapshot: None,
        case_mode: Some(CaseMode::Sensitive),
        smartsign: true,
        no_smartsign: false,
    };

    let command = popup_command(
        "/tmp/tnx binary",
        &args,
        "a'b",
        "@1",
        "%1",
        Some("/dev/pts/1"),
        Path::new("/tmp/motion snapshot.json"),
    );

    assert_eq!(command[0], "display-popup");
    assert!(command.contains(&"-E".to_string()));
    assert!(command.contains(&"-B".to_string()));
    assert!(command.contains(&"-c".to_string()));
    assert!(command.contains(&"/dev/pts/1".to_string()));
    assert!(command.contains(&"%1".to_string()));
    assert!(!command[..command.len() - 1].contains(&"@1".to_string()));
    assert!(!command.iter().any(|part| part == "new-window"));
    let shell_command = command.last().expect("popup shell command");
    assert!(shell_command.contains("'/tmp/tnx binary'"));
    assert!(shell_command.contains("--query 'a'\"'\"'b'"));
    assert!(shell_command.contains("--target-window @1"));
    assert!(shell_command.contains("--target-client /dev/pts/1"));
    assert!(shell_command.contains("--overlay"));
    assert!(shell_command.contains("--snapshot '/tmp/motion snapshot.json'"));
    assert!(shell_command.contains("--case sensitive"));
    assert!(shell_command.contains("--smartsign"));
}

#[test]
fn captures_every_visible_pane_with_the_correct_output() {
    let server = TestTmux::start("printf 'left_marker\\n'; sleep 60", 50, 12);
    let pane2 = server.split_window("printf 'right_marker\\n'; sleep 60");
    server.wait_for_lines(&server.pane_id, "left_marker");
    server.wait_for_lines(&pane2, "right_marker");
    let window_id = server.stdout(&[
        "display-message",
        "-p",
        "-t",
        server.pane_id.as_str(),
        "#{window_id}",
    ]);

    let panes =
        init_panes_with_client(&server.client(), Some(&window_id)).expect("capture visible panes");

    assert_eq!(panes.len(), 2);
    let left = panes
        .iter()
        .find(|pane| pane.pane_id == server.pane_id)
        .expect("left pane capture");
    let right = panes
        .iter()
        .find(|pane| pane.pane_id == pane2)
        .expect("right pane capture");
    assert!(left.lines.iter().any(|line| line.contains("left_marker")));
    assert!(right.lines.iter().any(|line| line.contains("right_marker")));
}

#[test]
fn move_cursor_positions_same_pane() {
    let server = TestTmux::start("printf 'line0\\nline1\\nline2_target\\n'; sleep 60", 30, 10);
    let lines = server.wait_for_lines(&server.pane_id, "line2_target");
    let pane = server.pane(&server.pane_id, lines);

    move_cursor_with_client(&server.client(), &pane, 2, 7, None, TabMode::PositionAware)
        .expect("same-pane cursor move");

    assert_eq!(server.copy_cursor(&server.pane_id), (7, 2));
}

#[test]
fn move_cursor_handles_wrapped_continuation_at_top() {
    let server = TestTmux::start(
        "printf '%*s' 60 '' | tr ' ' x; printf '\\n'; printf 'line1\\nline2\\nline3\\nline4\\nline5\\nline6\\nline7\\ntarget_line\\n'; sleep 60",
        20,
        10,
    );
    let lines = server.wait_for_lines(&server.pane_id, "target_line");
    let target_line = lines
        .iter()
        .position(|line| line.contains("target_line"))
        .expect("target line");
    assert!(target_line >= 2, "expected wrapped rows above target");
    let pane = server.pane(&server.pane_id, lines);

    move_cursor_with_client(
        &server.client(),
        &pane,
        target_line,
        0,
        None,
        TabMode::PositionAware,
    )
    .expect("wrapped continuation cursor move");

    assert_eq!(server.copy_cursor(&server.pane_id), (0, target_line));
    assert_eq!(
        server.stdout(&[
            "display-message",
            "-p",
            "-t",
            server.pane_id.as_str(),
            "#{copy_cursor_line}",
        ]),
        "target_line"
    );
}

#[test]
fn move_cursor_selects_cross_pane_target() {
    let server = TestTmux::start("printf 'left\\n'; sleep 60", 40, 12);
    let pane2 = server.split_window("printf 'line0\\nline1_target\\n'; sleep 60");
    server.stdout(&["select-pane", "-t", server.pane_id.as_str()]);
    let lines = server.wait_for_lines(&pane2, "line1_target");
    let pane = server.pane(&pane2, lines);

    move_cursor_with_client(&server.client(), &pane, 1, 6, None, TabMode::PositionAware)
        .expect("cross-pane cursor move");

    assert_eq!(server.active_pane(), pane2);
    assert_eq!(server.copy_cursor(&pane.pane_id), (6, 1));
}

#[test]
fn move_cursor_handles_leading_empty_rows() {
    let server = TestTmux::start("printf '\\n\\nleading_target\\n'; sleep 60", 40, 10);
    let lines = server.wait_for_lines(&server.pane_id, "leading_target");
    let target_line = lines
        .iter()
        .position(|line| line.contains("leading_target"))
        .expect("target line");
    let pane = server.pane(&server.pane_id, lines);

    move_cursor_with_client(
        &server.client(),
        &pane,
        target_line,
        0,
        None,
        TabMode::PositionAware,
    )
    .expect("leading-empty cursor move");

    assert_eq!(server.copy_cursor(&server.pane_id), (0, target_line));
}

#[test]
fn move_cursor_normalizes_tabs_without_crossing_rows() {
    let server = TestTmux::start(
        "printf '%b' '\\n\\tZ\\n'; printf '\\033[2;11H'; sleep 60",
        20,
        8,
    );
    let lines = server.wait_for_lines(&server.pane_id, "Z");
    let target_line = lines
        .iter()
        .position(|line| line.contains('Z'))
        .expect("tab target line");
    let pane = server.pane(&server.pane_id, lines);

    move_cursor_with_client(
        &server.client(),
        &pane,
        target_line,
        1,
        None,
        TabMode::PositionAware,
    )
    .expect("tab cursor move");

    assert_eq!(server.copy_cursor(&server.pane_id), (8, target_line));
    assert_eq!(
        server.stdout(&[
            "display-message",
            "-p",
            "-t",
            server.pane_id.as_str(),
            "#{copy_cursor_line}",
        ]),
        "\tZ"
    );
}

#[test]
fn move_cursor_normalizes_wide_cells_without_crossing_rows() {
    let server = TestTmux::start(
        "printf '%b' '\\n你你你a\\n'; printf '\\033[2;11H'; sleep 60",
        20,
        8,
    );
    let lines = server.wait_for_lines(&server.pane_id, "你你你a");
    let target_line = lines
        .iter()
        .position(|line| line.contains("你你你a"))
        .expect("wide target line");
    let pane = server.pane(&server.pane_id, lines);

    move_cursor_with_client(
        &server.client(),
        &pane,
        target_line,
        0,
        None,
        TabMode::PositionAware,
    )
    .expect("wide cursor move");

    assert_eq!(server.copy_cursor(&server.pane_id), (0, target_line));
    assert_eq!(
        server.stdout(&[
            "display-message",
            "-p",
            "-t",
            server.pane_id.as_str(),
            "#{copy_cursor_line}",
        ]),
        "你你你a"
    );
}

#[test]
fn move_cursor_handles_wrapped_target_line() {
    let server = TestTmux::start(
        "printf '%b' '012345678901234567890123456789\\ntarget-h\\nend\\n'; sleep 60",
        5,
        8,
    );
    server.wait_for_lines(&server.pane_id, "t-h");
    let window_id = server.stdout(&[
        "display-message",
        "-p",
        "-t",
        server.pane_id.as_str(),
        "#{window_id}",
    ]);
    let panes = init_panes_with_client(&server.client(), Some(&window_id))
        .expect("capture wrapped target pane");
    let pane = panes
        .iter()
        .find(|pane| pane.pane_id == server.pane_id)
        .expect("wrapped target pane");
    let matches = find_matches(
        std::slice::from_ref(pane),
        "h",
        CaseMode::Insensitive,
        false,
        TabMode::PositionAware,
    );
    assert_eq!(matches.len(), 1);
    let target = &matches[0];
    let line = &pane.lines[target.line_no];
    let true_col = true_position(line, target.visual_col, TabMode::PositionAware);

    move_cursor_with_client(
        &server.client(),
        pane,
        target.line_no,
        true_col,
        None,
        TabMode::PositionAware,
    )
    .expect("wrapped target cursor move");

    assert_eq!(server.copy_cursor(&server.pane_id), (2, 5));
    assert_eq!(
        server.stdout(&[
            "display-message",
            "-p",
            "-t",
            server.pane_id.as_str(),
            "#{copy_cursor_line}",
        ]),
        "t-h"
    );
}
