mod app;
mod demo;
mod events;
mod herdr;
mod model;
mod pane_view;
mod state;
mod theme;
mod ui;

use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
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
