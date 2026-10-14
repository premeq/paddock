mod app;
mod demo;
mod events;
mod herdr;
mod model;
mod pane_view;
mod state;
mod theme;
mod ui;

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event, KeyEventKind,
};
use ratatui::crossterm::execute;
use ratatui::DefaultTerminal;

use app::{App, Effect};

fn main() -> Result<()> {
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!("paddock: home screen for your herdr fleet\n\nusage: paddock [--local-poll SECS] [--remote-poll SECS] [--demo]");
        return Ok(());
    }
    let local = arg_secs("--local-poll").unwrap_or(1.0);
    let remote = arg_secs("--remote-poll").unwrap_or(3.0);
    let mut app = if std::env::args().any(|a| a == "--demo") {
        App::demo()?
    } else {
        App::new(Duration::from_secs_f64(local), Duration::from_secs_f64(remote))?
    };
    let mut terminal = init();
    let result = run(&mut terminal, &mut app);
    restore();
    result
}

fn init() -> DefaultTerminal {
    let terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste);
    terminal
}

fn restore() {
    let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
}

/// OSC 52 reaches the clipboard through most terminals and SSH; pbcopy covers
/// macOS terminals that ignore it.
fn copy(text: &str) {
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{}\x07", B64.encode(text)).and_then(|_| out.flush());
    if cfg!(target_os = "macos") {
        if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
            let _ = child.stdin.take().map(|mut i| i.write_all(text.as_bytes()));
            let _ = child.wait();
        }
    }
}

fn arg_secs(flag: &str) -> Option<f64> {
    let args: Vec<String> = std::env::args().collect();
    let i = args.iter().position(|a| a == flag)?;
    args.get(i + 1)?.parse().ok()
}

fn run(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;
        let mut effect = Effect::None;
        let tick = if app.attached.is_some() { 16 } else { 250 };
        if event::poll(Duration::from_millis(tick))? {
            match event::read()? {
                Event::Key(k) if k.kind != KeyEventKind::Release => effect = app.key(k),
                Event::Mouse(m) => effect = app.mouse(m),
                Event::Paste(text) => app.paste(&text),
                _ => {}
            }
        }
        app.pump();
        match effect {
            Effect::None => {}
            Effect::Quit => return Ok(()),
            Effect::Copy(text) => copy(&text),
            Effect::Exec(argv) => {
                restore();
                let status = Command::new(&argv[0])
                    .args(&argv[1..])
                    .status()
                    .with_context(|| format!("run {}", argv.join(" ")));
                *terminal = init();
                if let Err(e) = status {
                    app.notice = Some(e.to_string());
                }
                app.pane_text.clear();
                app.reload_machines();
            }
        }
    }
}
