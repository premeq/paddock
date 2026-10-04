#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use serde::Deserialize;
use serde_json::{json, Value};
use unicode_width::UnicodeWidthChar;

use crate::herdr::Runner;

const WHEEL_LINES: u16 = 3;

/// A live herdr pane streamed through `herdr terminal session control`.
pub struct PaneView {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<Msg>,
    grid: Grid,
    cols: u16,
    rows: u16,
    closed: Option<String>,
}

enum Msg {
    Frame { ansi: String, full: bool, width: u16, height: u16 },
    Closed(String),
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum Record {
    #[serde(rename = "terminal.frame")]
    Frame {
        bytes: String,
        #[serde(default)]
        full: bool,
        width: u16,
        height: u16,
    },
    #[serde(rename = "terminal.closed")]
    Closed {
        #[serde(default)]
        reason: String,
    },
    #[serde(other)]
    Other,
}

impl PaneView {
    pub fn open(runner: &Runner, pane_id: &str, cols: u16, rows: u16) -> Result<PaneView> {
        let argv = runner.session_control_argv(pane_id, cols, rows);
        let mut child = Command::new(&argv[0])
            .args(&argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn {}", argv.join(" ")))?;
        let stdout = child.stdout.take().context("no stdout")?;
        let stderr = child.stderr.take().context("no stderr")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || read_stream(stdout, stderr, tx));
        Ok(PaneView { stdin: child.stdin.take(), child, rx, grid: Grid::default(), cols, rows, closed: None })
    }

    pub fn pump(&mut self) -> bool {
        let mut changed = false;
        while let Ok(msg) = self.rx.try_recv() {
            changed = true;
            match msg {
                Msg::Frame { ansi, full, width, height } => {
                    self.grid.resize(width.into(), height.into());
                    if full {
                        self.grid.clear();
                    }
                    self.grid.apply(&ansi);
                }
                Msg::Closed(reason) => {
                    self.closed.get_or_insert(reason);
                }
            }
        }
        changed
    }

    pub fn closed(&self) -> Option<&str> {
        self.closed.as_deref()
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        if (cols, rows) == (self.cols, self.rows) || cols == 0 || rows == 0 {
            return;
        }
        (self.cols, self.rows) = (cols, rows);
        self.send(json!({"type": "terminal.resize", "cols": cols, "rows": rows}));
    }

    pub fn key(&mut self, key: KeyEvent) {
        if key.modifiers.is_empty() && matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
            let direction = if key.code == KeyCode::PageUp { "up" } else { "down" };
            let lines = self.rows.saturating_sub(1).max(1);
            self.send(json!({"type": "terminal.scroll", "direction": direction, "lines": lines, "source": "page_key"}));
            return;
        }
        if let Some(bytes) = encode_key(key, self.grid.app_cursor) {
            self.input(&bytes);
        }
    }

    pub fn paste(&mut self, text: &str) {
        let text = text.replace("\x1b[201~", "");
        self.input(format!("\x1b[200~{text}\x1b[201~").as_bytes());
    }

    pub fn mouse(&mut self, ev: MouseEvent, origin: (u16, u16)) {
        let (Some(column), Some(row)) = (ev.column.checked_sub(origin.0), ev.row.checked_sub(origin.1))
        else {
            return;
        };
        if usize::from(column) >= self.grid.width || usize::from(row) >= self.grid.height {
            return;
        }
        let modifiers = ev.modifiers.bits() & 0b111;
        let (action, button) = match ev.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let direction = if ev.kind == MouseEventKind::ScrollUp { "up" } else { "down" };
                self.send(json!({"type": "terminal.scroll", "direction": direction, "lines": WHEEL_LINES,
                    "source": "wheel", "column": column, "row": row, "modifiers": modifiers}));
                return;
            }
            MouseEventKind::Down(b) => ("down", b),
            MouseEventKind::Up(b) => ("up", b),
            MouseEventKind::Drag(b) => ("drag", b),
            MouseEventKind::Moved => ("move", MouseButton::Left),
            _ => return,
        };
        let button = match button {
            MouseButton::Left => "left",
            MouseButton::Right => "right",
            MouseButton::Middle => "middle",
        };
        self.send(json!({"type": "terminal.mouse", "action": action, "button": button,
            "column": column, "row": row, "modifiers": modifiers}));
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        self.grid.render(area, buf);
    }

    pub fn cursor(&self, area: Rect) -> Option<(u16, u16)> {
        let g = &self.grid;
        if self.closed.is_some() || !g.cursor_visible || g.col >= g.width || g.row >= g.height {
            return None;
        }
        let (x, y) = (g.col as u16, g.row as u16);
        (x < area.width && y < area.height).then_some((area.x + x, area.y + y))
    }

    pub fn release(mut self) {
        self.send(json!({"type": "terminal.release"}));
        self.stdin = None;
        for _ in 0..20 {
            if !matches!(self.child.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn input(&mut self, bytes: &[u8]) {
        self.send(json!({"type": "terminal.input", "bytes": B64.encode(bytes)}));
    }

    fn send(&mut self, cmd: Value) {
        if self.closed.is_some() {
            return;
        }
        let Some(stdin) = self.stdin.as_mut() else {
            return;
        };
        let line = format!("{cmd}\n");
        if stdin.write_all(line.as_bytes()).and_then(|_| stdin.flush()).is_err() {
            self.closed = Some("write to herdr failed".into());
        }
    }
}

impl Drop for PaneView {
    fn drop(&mut self) {
        self.stdin = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_stream(stdout: impl Read, mut stderr: impl Read, tx: Sender<Msg>) {
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        let msg = match serde_json::from_str(&line) {
            Ok(Record::Frame { bytes, full, width, height }) => {
                let ansi = String::from_utf8_lossy(&B64.decode(bytes).unwrap_or_default()).into_owned();
                Msg::Frame { ansi, full, width, height }
            }
            Ok(Record::Closed { reason }) => Msg::Closed(reason),
            _ => continue,
        };
        if tx.send(msg).is_err() {
            return;
        }
    }
    let mut err = String::new();
    let _ = stderr.read_to_string(&mut err);
    let reason = err.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("stream ended");
    let _ = tx.send(Msg::Closed(reason.to_owned()));
}

/// xterm-style bytes for a key, or None for keys a terminal cannot receive.
fn encode_key(key: KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    let mods = key.modifiers;
    let (shift, alt, ctrl) = (
        mods.contains(KeyModifiers::SHIFT),
        mods.contains(KeyModifiers::ALT),
        mods.contains(KeyModifiers::CONTROL),
    );
    let m = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    let csi_letter = |c: char, ss3: bool| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[1;{m}{c}").into_bytes()
        } else if ss3 {
            format!("\x1bO{c}").into_bytes()
        } else {
            format!("\x1b[{c}").into_bytes()
        }
    };
    let csi_tilde = |n: u8| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[{n};{m}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    let plain: Vec<u8> = match key.code {
        KeyCode::Char(c) if ctrl => vec![ctrl_byte(c).unwrap_or(c as u8)],
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Tab if shift => return Some(b"\x1b[Z".to_vec()),
        KeyCode::BackTab => return Some(b"\x1b[Z".to_vec()),
        KeyCode::Tab => b"\t".to_vec(),
        KeyCode::Backspace if ctrl => vec![0x08],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => return Some(csi_letter('A', app_cursor)),
        KeyCode::Down => return Some(csi_letter('B', app_cursor)),
        KeyCode::Right => return Some(csi_letter('C', app_cursor)),
        KeyCode::Left => return Some(csi_letter('D', app_cursor)),
        KeyCode::Home => return Some(csi_letter('H', app_cursor)),
        KeyCode::End => return Some(csi_letter('F', app_cursor)),
        KeyCode::Insert => return Some(csi_tilde(2)),
        KeyCode::Delete => return Some(csi_tilde(3)),
        KeyCode::PageUp => return Some(csi_tilde(5)),
        KeyCode::PageDown => return Some(csi_tilde(6)),
        KeyCode::F(n @ 1..=4) => return Some(csi_letter(b"PQRS"[n as usize - 1] as char, true)),
        KeyCode::F(n @ 5..=12) => return Some(csi_tilde([15, 17, 18, 19, 20, 21, 23, 24][n as usize - 5])),
        _ => return None,
    };
    Some(if alt { [&[0x1b][..], &plain].concat() } else { plain })
}

fn ctrl_byte(c: char) -> Option<u8> {
    Some(match c.to_ascii_lowercase() {
        c @ 'a'..='z' => c as u8 - b'a' + 1,
        '@' | ' ' | '2' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '-' | '7' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

// Grid decoding ported from herdr-mirror src/grid.rs by Niko Kristanto (MIT).

#[derive(Clone, PartialEq, Debug)]
struct Cell {
    /// Empty for the spacer that follows a wide char.
    sym: String,
    style: Style,
}

impl Default for Cell {
    fn default() -> Self {
        Cell { sym: " ".into(), style: Style::default() }
    }
}

/// Cell grid for herdr's frame ANSI: absolute CUP + SGR + text, no scrolling or relative moves.
#[derive(Default)]
struct Grid {
    cells: Vec<Cell>,
    width: usize,
    height: usize,
    row: usize,
    col: usize,
    cursor_visible: bool,
    app_cursor: bool,
    style: Style,
}

impl Grid {
    fn resize(&mut self, width: usize, height: usize) {
        if (width, height) != (self.width, self.height) {
            (self.width, self.height) = (width, height);
            self.clear();
        }
    }

    fn clear(&mut self) {
        self.cells = vec![Cell::default(); self.width * self.height];
    }

    fn blank(&mut self, from: usize, to: usize) {
        let to = to.min(self.cells.len());
        if from < to {
            self.cells[from..to].fill(Cell::default());
        }
    }

    fn apply(&mut self, ansi: &str) {
        self.style = Style::default();
        let mut last: Option<usize> = None;
        let mut i = 0;
        while i < ansi.len() {
            let rest = &ansi[i..];
            let Some(ch) = rest.chars().next() else { break };
            if ch == '\x1b' {
                i += self.escape(rest);
                last = None;
                continue;
            }
            i += ch.len_utf8();
            let ch = if ch == '\t' { ' ' } else { ch };
            if ch.is_control() {
                continue;
            }
            match ch.width() {
                Some(0) => {
                    if let Some(idx) = last {
                        self.cells[idx].sym.push(ch);
                    }
                }
                Some(w) => {
                    last = None;
                    if self.row < self.height && self.col < self.width {
                        let idx = self.row * self.width + self.col;
                        self.cells[idx] = Cell { sym: ch.to_string(), style: self.style };
                        if w == 2 && self.col + 1 < self.width {
                            self.cells[idx + 1] = Cell { sym: String::new(), style: self.style };
                        }
                        last = Some(idx);
                    }
                    self.col += w;
                }
                None => {}
            }
        }
    }

    /// Handles the escape sequence at the start of `s`; returns its byte length.
    fn escape(&mut self, s: &str) -> usize {
        let b = s.as_bytes();
        match b.get(1) {
            Some(b'[') => {
                if let Some((params, fin, len)) = parse_csi(b) {
                    self.csi(params, fin);
                    return len;
                }
            }
            Some(b']') => {
                if let Some(len) = osc_len(b) {
                    return len;
                }
            }
            _ => {}
        }
        1 + s[1..].chars().next().map_or(0, char::len_utf8)
    }

    fn csi(&mut self, params: &str, fin: u8) {
        let num = |i: usize, default: usize| {
            params.split(';').nth(i).and_then(|n| n.parse().ok()).unwrap_or(default)
        };
        let pos = self.row * self.width + self.col.min(self.width);
        match fin {
            b'H' | b'f' => {
                self.row = num(0, 1).clamp(1, 0xffff) - 1;
                self.col = num(1, 1).clamp(1, 0xffff) - 1;
            }
            b'm' => self.style = sgr(self.style, params),
            b'J' => match num(0, 0) {
                0 => self.blank(pos, self.cells.len()),
                1 => self.blank(0, pos + 1),
                _ => self.clear(),
            },
            b'K' if self.row < self.height => {
                let start = self.row * self.width;
                match num(0, 0) {
                    0 => self.blank(pos, start + self.width),
                    1 => self.blank(start, pos + 1),
                    _ => self.blank(start, start + self.width),
                }
            }
            b'h' | b'l' if params.starts_with('?') => {
                for mode in params[1..].split(';') {
                    match mode {
                        "25" => self.cursor_visible = fin == b'h',
                        "1" => self.app_cursor = fin == b'h',
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let w = self.width.min(area.width.into());
        for y in 0..self.height.min(area.height.into()) {
            for x in 0..w {
                let cell = &self.cells[y * self.width + x];
                let wide = cell.sym.chars().next().is_some_and(|c| c.width() == Some(2));
                let sym = if wide && x + 1 >= w { " " } else { cell.sym.as_str() };
                if let Some(c) = buf.cell_mut((area.x + x as u16, area.y + y as u16)) {
                    c.reset();
                    c.set_symbol(sym);
                    c.set_style(cell.style);
                }
            }
        }
    }

    fn text_lines(&self) -> Vec<String> {
        self.cells
            .chunks(self.width.max(1))
            .map(|r| r.iter().map(|c| c.sym.as_str()).collect::<String>().trim_end().to_owned())
            .collect()
    }
}

/// CSI: ESC [ params(0x30-0x3F)* intermediates(0x20-0x2F)* final(0x40-0x7E).
fn parse_csi(b: &[u8]) -> Option<(&str, u8, usize)> {
    let mut end_params = 2;
    while end_params < b.len() && (0x30..=0x3f).contains(&b[end_params]) {
        end_params += 1;
    }
    let mut i = end_params;
    while i < b.len() && (0x20..=0x2f).contains(&b[i]) {
        i += 1;
    }
    let fin = *b.get(i)?;
    if !(0x40..=0x7e).contains(&fin) {
        return None;
    }
    let params = std::str::from_utf8(&b[2..end_params]).ok()?;
    Some((params, fin, i + 1))
}

/// OSC: ESC ] ... (BEL | ESC \). Hyperlinks and titles are dropped.
fn osc_len(b: &[u8]) -> Option<usize> {
    let mut i = 2;
    while i < b.len() {
        match b[i] {
            0x07 => return Some(i + 1),
            0x1b if b.get(i + 1) == Some(&b'\\') => return Some(i + 2),
            0x1b => return None,
            _ => i += 1,
        }
    }
    None
}

fn sgr(mut style: Style, params: &str) -> Style {
    let tokens: Vec<&str> = params.split(';').collect();
    let mut i = 0;
    while i < tokens.len() {
        let mut sub = tokens[i].split(':');
        let n: u16 = sub.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        let sub: Vec<&str> = sub.collect();
        match n {
            0 => style = Style::default(),
            1 => style = style.add_modifier(Modifier::BOLD),
            2 => style = style.add_modifier(Modifier::DIM),
            3 => style = style.add_modifier(Modifier::ITALIC),
            4 if sub.first() == Some(&"0") => style = style.remove_modifier(Modifier::UNDERLINED),
            4 => style = style.add_modifier(Modifier::UNDERLINED),
            5 => style = style.add_modifier(Modifier::SLOW_BLINK),
            6 => style = style.add_modifier(Modifier::RAPID_BLINK),
            7 => style = style.add_modifier(Modifier::REVERSED),
            8 => style = style.add_modifier(Modifier::HIDDEN),
            9 => style = style.add_modifier(Modifier::CROSSED_OUT),
            22 => style = style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => style = style.remove_modifier(Modifier::ITALIC),
            24 => style = style.remove_modifier(Modifier::UNDERLINED),
            25 => style = style.remove_modifier(Modifier::SLOW_BLINK | Modifier::RAPID_BLINK),
            27 => style = style.remove_modifier(Modifier::REVERSED),
            28 => style = style.remove_modifier(Modifier::HIDDEN),
            29 => style = style.remove_modifier(Modifier::CROSSED_OUT),
            30..=37 => style.fg = Some(ANSI[usize::from(n - 30)]),
            90..=97 => style.fg = Some(ANSI[usize::from(n - 82)]),
            40..=47 => style.bg = Some(ANSI[usize::from(n - 40)]),
            100..=107 => style.bg = Some(ANSI[usize::from(n - 92)]),
            39 => style.fg = None,
            49 => style.bg = None,
            38 | 48 | 58 => {
                let (color, used) = if sub.is_empty() {
                    extended_color(&tokens[i + 1..])
                } else {
                    (extended_color(&sub).0, 0)
                };
                i += used;
                match (n, color) {
                    (38, Some(c)) => style.fg = Some(c),
                    (48, Some(c)) => style.bg = Some(c),
                    _ => {}
                }
            }
            _ => {}
        }
        i += 1;
    }
    style
}

/// `5;n` or `2;r;g;b` (colon form may carry an empty colorspace id); returns color and tokens used.
fn extended_color(t: &[&str]) -> (Option<Color>, usize) {
    let n = |i: usize| t.get(i).and_then(|v| v.parse::<u8>().ok());
    match t.first().copied() {
        Some("5") => (n(1).map(Color::Indexed), 2),
        Some("2") if t.len() >= 5 && t[1].is_empty() => {
            (n(2).zip(n(3)).zip(n(4)).map(|((r, g), b)| Color::Rgb(r, g, b)), 5)
        }
        Some("2") => (n(1).zip(n(2)).zip(n(3)).map(|((r, g), b)| Color::Rgb(r, g, b)), 4),
        _ => (None, 0),
    }
}

const ANSI: [Color; 16] = [
    Color::Black,
    Color::Red,
    Color::Green,
    Color::Yellow,
    Color::Blue,
    Color::Magenta,
    Color::Cyan,
    Color::Gray,
    Color::DarkGray,
    Color::LightRed,
    Color::LightGreen,
    Color::LightYellow,
    Color::LightBlue,
    Color::LightMagenta,
    Color::LightCyan,
    Color::White,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> Vec<u8> {
        encode_key(KeyEvent::new(code, mods), false).unwrap()
    }

    #[test]
    fn encodes_keys() {
        let none = KeyModifiers::NONE;
        assert_eq!(key(KeyCode::Char('é'), none), "é".as_bytes());
        assert_eq!(key(KeyCode::Char('c'), KeyModifiers::CONTROL), [0x03]);
        assert_eq!(key(KeyCode::Char(' '), KeyModifiers::CONTROL), [0x00]);
        assert_eq!(key(KeyCode::Char(']'), KeyModifiers::CONTROL), [0x1d]);
        assert_eq!(key(KeyCode::Char('x'), KeyModifiers::ALT), b"\x1bx");
        assert_eq!(key(KeyCode::Enter, none), b"\r");
        assert_eq!(key(KeyCode::BackTab, KeyModifiers::SHIFT), b"\x1b[Z");
        assert_eq!(key(KeyCode::Backspace, none), [0x7f]);
        assert_eq!(key(KeyCode::Delete, none), b"\x1b[3~");
        assert_eq!(key(KeyCode::F(1), none), b"\x1bOP");
        assert_eq!(key(KeyCode::F(12), none), b"\x1b[24~");
    }

    #[test]
    fn encodes_modified_and_app_cursor_keys() {
        assert_eq!(key(KeyCode::Up, KeyModifiers::NONE), b"\x1b[A");
        assert_eq!(key(KeyCode::Left, KeyModifiers::CONTROL), b"\x1b[1;5D");
        assert_eq!(key(KeyCode::Right, KeyModifiers::SHIFT | KeyModifiers::ALT), b"\x1b[1;4C");
        assert_eq!(key(KeyCode::PageDown, KeyModifiers::CONTROL), b"\x1b[6;5~");
        assert_eq!(key(KeyCode::F(3), KeyModifiers::SHIFT), b"\x1b[1;2R");
        let app = |code| encode_key(KeyEvent::new(code, KeyModifiers::NONE), true).unwrap();
        assert_eq!(app(KeyCode::Down), b"\x1bOB");
        assert_eq!(app(KeyCode::Home), b"\x1bOH");
    }

    #[test]
    fn applies_frame() {
        let mut g = Grid::default();
        g.resize(10, 3);
        g.apply(
            "\x1b[?2026h\x1b[?25l\x1b]8;;\x1b\\\x1b[1;1H\x1b[0;1;38;2;255;0;0;49mhi\x1b[0;39;49m \x1b]8;;https://x.y\x1b\\\
             link\x1b]8;;\x1b\\\x1b[2;1H\x1b[0;38;5;42;49m한\x1b[2;3H!\x1b[0m\x1b[3;4H\x1b[2 q\x1b[?25h\x1b[?2026l",
        );
        assert_eq!(g.text_lines(), ["hi link", "한!", ""]);
        assert_eq!((g.row, g.col, g.cursor_visible), (2, 3, true));
        assert_eq!(g.cells[0].style, Style::default().fg(Color::Rgb(255, 0, 0)).add_modifier(Modifier::BOLD));
        assert_eq!(g.cells[10].style.fg, Some(Color::Indexed(42)));
        assert_eq!(g.cells[11].sym, "");
        g.apply("\x1b[1;3H\x1b[K\x1b[?1h\x1b[?25l");
        assert_eq!(g.text_lines(), ["hi", "한!", ""]);
        assert!(g.app_cursor && !g.cursor_visible);
    }

    #[test]
    fn garbage_does_not_panic_and_render_clips() {
        let mut g = Grid::default();
        g.apply("\x1b[5;5Hx\x1b[J\x1b[1K");
        g.resize(3, 2);
        for s in [
            "\x1b",
            "\x1b[",
            "\x1b]8;;",
            "\x1bé",
            "\x1b[99;99Hzz\x1b[2K\x1b[1J",
            "\r\n\x1b[2;1Ha\u{301}\x1b[38;2m\x1b[38:5m",
        ] {
            g.apply(s);
        }
        assert_eq!(g.text_lines(), ["", "a\u{301}"]);
        g.apply("\x1b[1;1Hx한");
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        g.render(Rect::new(0, 0, 2, 5), &mut buf);
        assert_eq!(buf, Buffer::with_lines(["x "]));
    }

    #[test]
    #[ignore]
    fn live_pane() {
        let pane = std::env::var("PADDOCK_TEST_PANE").expect("set PADDOCK_TEST_PANE");
        let mut view = PaneView::open(&Runner::local(), &pane, 80, 24).unwrap();
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(2) {
            view.pump();
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        println!("closed: {:?}  cursor: {:?}", view.closed(), view.cursor(Rect::new(0, 0, 80, 24)));
        for line in view.grid.text_lines() {
            println!("|{line}");
        }
        assert!(view.grid.width > 0);
        view.release();
    }
}
