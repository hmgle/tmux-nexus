use std::{
    fmt::Write as _,
    io::{self, Read, Write},
    os::fd::AsRawFd,
};

use anyhow::{Context, Result};

use super::MotionConfig;

#[derive(Clone, Copy)]
pub(super) enum Attr {
    Normal,
    Dim,
    Hint1,
    Hint2,
}

pub(super) struct AnsiScreen {
    dim: String,
    hint1: String,
    hint2: String,
    buffer: String,
    active: bool,
}

impl AnsiScreen {
    pub(super) fn new(config: &MotionConfig) -> Self {
        Self {
            dim: format!("\x1b[{}m", config.dim),
            hint1: format!("\x1b[{}m", config.hint1_fg),
            hint2: format!("\x1b[{}m", config.hint2_fg),
            buffer: String::with_capacity(4096),
            active: false,
        }
    }

    pub(super) fn init(&mut self) -> Result<()> {
        self.buffer.push_str("\x1b[?25l\x1b[2J");
        self.active = true;
        Ok(())
    }

    // The title follows the complete frame in the same PTY byte stream. The
    // supervisor waits for tmux to parse it before exposing the window.
    pub(super) fn set_title(&mut self, title: &str) -> Result<()> {
        anyhow::ensure!(!title.chars().any(char::is_control), "invalid motion title");
        write!(self.buffer, "\x1b]2;{title}\x07")?;
        Ok(())
    }

    pub(super) fn cleanup(&mut self) -> Result<()> {
        let mut stdout = io::stdout().lock();
        if !self.buffer.is_empty() {
            stdout.write_all(self.buffer.as_bytes())?;
            self.buffer.clear();
        }
        stdout.write_all(b"\x1b[0m\x1b[?25h")?;
        stdout.flush()?;
        self.active = false;
        Ok(())
    }

    pub(super) fn addstr(&mut self, y: usize, x: usize, text: &str, attr: Attr) -> Result<()> {
        let attr = match attr {
            Attr::Normal => "",
            Attr::Dim => &self.dim,
            Attr::Hint1 => &self.hint1,
            Attr::Hint2 => &self.hint2,
        };
        if attr.is_empty() {
            write!(self.buffer, "\x1b[{};{}H{}", y + 1, x + 1, text)?;
        } else {
            write!(
                self.buffer,
                "\x1b[{};{}H{}{}\x1b[0m",
                y + 1,
                x + 1,
                attr,
                text
            )?;
        }
        Ok(())
    }

    pub(super) fn refresh(&mut self) -> Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let mut stdout = io::stdout().lock();
        stdout.write_all(self.buffer.as_bytes())?;
        stdout.flush()?;
        self.buffer.clear();
        Ok(())
    }
}

impl Drop for AnsiScreen {
    fn drop(&mut self) {
        if self.active {
            let _ = self.cleanup();
        }
    }
}

pub(super) struct RawMode {
    fd: i32,
    original: libc::termios,
}

impl RawMode {
    pub(super) fn new() -> Result<Self> {
        let fd = io::stdin().as_raw_fd();
        let original = termios_for_fd(fd)?;
        let mut raw = original;
        unsafe { libc::cfmakeraw(&mut raw) };
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        set_termios(fd, &raw)?;
        Ok(Self { fd, original })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = set_termios(self.fd, &self.original);
    }
}

pub(super) fn read_key(reader: &mut impl Read) -> Result<char> {
    let mut first = [0_u8; 1];
    reader.read_exact(&mut first)?;
    let width = utf8_char_width(first[0]).context("invalid utf-8 input")?;
    let mut bytes = vec![first[0]];
    if width > 1 {
        let mut rest = vec![0_u8; width - 1];
        reader.read_exact(&mut rest)?;
        bytes.extend(rest);
    }
    let value = std::str::from_utf8(&bytes)?;
    value.chars().next().context("empty key input")
}

pub(super) fn drain_pending_input(fd: i32, timeout_ms: i32) {
    let mut poll_fd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let mut remaining = timeout_ms;
    let mut buffer = [0_u8; 64];
    while remaining >= 0 {
        let ready = unsafe { libc::poll(&mut poll_fd, 1, remaining) };
        if ready <= 0 {
            break;
        }
        let read = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        if read <= 0 {
            break;
        }
        remaining = 0;
    }
}

fn utf8_char_width(byte: u8) -> Option<usize> {
    match byte {
        0x00..=0x7f => Some(1),
        0xc2..=0xdf => Some(2),
        0xe0..=0xef => Some(3),
        0xf0..=0xf4 => Some(4),
        _ => None,
    }
}

fn termios_for_fd(fd: i32) -> Result<libc::termios> {
    let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
    let status = unsafe { libc::tcgetattr(fd, termios.as_mut_ptr()) };
    if status != 0 {
        return Err(io::Error::last_os_error()).context("failed to read terminal mode");
    }
    Ok(unsafe { termios.assume_init() })
}

fn set_termios(fd: i32, termios: &libc::termios) -> Result<()> {
    let status = unsafe { libc::tcsetattr(fd, libc::TCSADRAIN, termios) };
    if status != 0 {
        return Err(io::Error::last_os_error()).context("failed to set terminal mode");
    }
    Ok(())
}
