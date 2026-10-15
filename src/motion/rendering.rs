use anyhow::Result;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::{
    terminal::{AnsiScreen, Attr},
    HintPosition, MotionConfig, Pane, TabMode,
};

pub(super) fn draw_all_panes(
    screen: &mut AnsiScreen,
    panes: &[Pane],
    max_x: usize,
    terminal_height: usize,
    config: &MotionConfig,
) -> Result<()> {
    let mut order: Vec<usize> = (0..panes.len()).collect();
    order.sort_by_key(|index| panes[*index].start_y + panes[*index].height);

    for index in order {
        let pane = &panes[index];
        let visible_height = pane
            .height
            .min(terminal_height.saturating_sub(pane.start_y));
        for (y, line) in pane.lines.iter().take(visible_height).enumerate() {
            let expanded = expand_tabs(line, config.tab_mode);
            let sliced = visual_slice(&expanded, pane.width, config.tab_mode);
            screen.addstr(pane.start_y + y, pane.start_x, &sliced, Attr::Normal)?;
        }

        if pane.start_x + pane.width < max_x {
            for y in pane.start_y..pane.start_y + visible_height {
                screen.addstr(
                    y,
                    pane.start_x + pane.width,
                    &config.vertical_border,
                    Attr::Dim,
                )?;
            }
        }

        let end_y = pane.start_y + visible_height;
        if end_y < terminal_height {
            screen.addstr(
                end_y,
                pane.start_x,
                &config.horizontal_border.repeat(pane.width),
                Attr::Dim,
            )?;
        }
    }
    Ok(())
}

pub(super) fn draw_all_hints(
    screen: &mut AnsiScreen,
    positions: &[HintPosition],
    terminal_height: usize,
) -> Result<()> {
    for position in positions {
        if position.screen_y >= terminal_height {
            continue;
        }
        let mut hint_chars = position.hint.chars();
        if let Some(first) = hint_chars.next() {
            screen.addstr(
                position.screen_y,
                position.screen_x,
                &first.to_string(),
                Attr::Hint1,
            )?;
        }
    }
    Ok(())
}

pub(super) fn update_hints_display(
    screen: &mut AnsiScreen,
    positions: &[HintPosition],
    current_key: &str,
) -> Result<()> {
    for position in positions {
        if !position.hint.starts_with(current_key) {
            screen.addstr(
                position.screen_y,
                position.screen_x,
                &position.original,
                Attr::Normal,
            )?;
            continue;
        }

        let current_len = current_key.chars().count();
        if position.hint.chars().count() > current_len {
            if let Some(next_hint) = position.hint.chars().nth(current_len) {
                screen.addstr(
                    position.screen_y,
                    position.screen_x,
                    &next_hint.to_string(),
                    Attr::Hint2,
                )?;
            }
        } else {
            screen.addstr(
                position.screen_y,
                position.screen_x,
                &position.original,
                Attr::Normal,
            )?;
        }
    }
    screen.refresh()
}

fn calculate_tab_width(position: usize) -> usize {
    8 - (position % 8)
}

pub(super) fn grapheme_at_char_index(
    line: &str,
    target_char_index: usize,
) -> Option<(&str, Option<&str>)> {
    let mut char_index = 0;
    let mut iter = line.graphemes(true).peekable();
    while let Some(grapheme) = iter.next() {
        let next_index = char_index + grapheme.chars().count();
        if char_index == target_char_index {
            return Some((grapheme, iter.peek().copied()));
        }
        if next_index > target_char_index {
            return None;
        }
        char_index = next_index;
    }
    None
}

pub(super) fn grapheme_width_at(grapheme: &str, position: usize, tab_mode: TabMode) -> usize {
    if grapheme == "\t" {
        match tab_mode {
            TabMode::Fixed => 8,
            TabMode::PositionAware => calculate_tab_width(position),
        }
    } else {
        UnicodeWidthStr::width(grapheme).max(1)
    }
}

pub(super) fn display_grapheme(grapheme: &str, position: usize, tab_mode: TabMode) -> String {
    if grapheme == "\t" {
        " ".repeat(grapheme_width_at(grapheme, position, tab_mode))
    } else {
        grapheme.to_string()
    }
}

#[cfg(test)]
pub(super) fn string_width(value: &str, tab_mode: TabMode) -> usize {
    let mut width = 0;
    for grapheme in value.graphemes(true) {
        width += grapheme_width_at(grapheme, width, tab_mode);
    }
    width
}

pub(super) fn true_position(line: &str, target_col: usize, tab_mode: TabMode) -> usize {
    let mut visual_pos = 0;
    let mut true_pos = 0;
    for grapheme in line.graphemes(true) {
        if visual_pos >= target_col {
            break;
        }
        visual_pos += grapheme_width_at(grapheme, visual_pos, tab_mode);
        true_pos += grapheme.chars().count();
    }
    true_pos
}

pub(super) fn visual_slice(value: &str, max_width: usize, tab_mode: TabMode) -> String {
    let mut visual_pos = 0;
    let mut out = String::new();
    for grapheme in value.graphemes(true) {
        let width = grapheme_width_at(grapheme, visual_pos, tab_mode);
        if visual_pos + width > max_width {
            break;
        }
        out.push_str(grapheme);
        visual_pos += width;
    }
    if visual_pos < max_width {
        out.push_str(&" ".repeat(max_width - visual_pos));
    }
    out
}

pub(super) fn expand_tabs(line: &str, tab_mode: TabMode) -> String {
    if !line.contains('\t') {
        return line.to_string();
    }
    let mut out = String::new();
    let mut pos = 0;
    for grapheme in line.graphemes(true) {
        if grapheme == "\t" {
            let width = grapheme_width_at(grapheme, pos, tab_mode);
            out.push_str(&" ".repeat(width));
            pos += width;
        } else {
            out.push_str(grapheme);
            pos += UnicodeWidthStr::width(grapheme).max(1);
        }
    }
    out
}
