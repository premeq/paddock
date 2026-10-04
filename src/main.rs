mod app;
mod herdr;
mod model;
mod theme;
mod ui;

use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::DefaultTerminal;

use app::{App, Effect};

fn main() -> Result<()> {
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!("paddock: home screen for your herdr fleet\n\nusage: paddock [--local-poll SECS] [--remote-poll SECS]");
        return Ok(());
    }
    let local = arg_secs("--local-poll").unwrap_or(1.0);
    let remote = arg_secs("--remote-poll").unwrap_or(3.0);
    let mut app = App::new(Duration::from_secs_f64(local), Duration::from_secs_f64(remote))?;
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app);
    ratatui::restore();
    result
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
        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(k) if k.kind != KeyEventKind::Release => effect = app.key(k),
                Event::Mouse(_) => {}
                _ => {}
            }
        }
        app.pump();
        match effect {
            Effect::None => {}
            Effect::Quit => return Ok(()),
            Effect::Exec(argv) => {
                ratatui::restore();
                let status = Command::new(&argv[0])
                    .args(&argv[1..])
                    .status()
                    .with_context(|| format!("run {}", argv.join(" ")));
                *terminal = ratatui::init();
                if let Err(e) = status {
                    app.last_error = Some(e.to_string());
                }
                app.pane_text.clear();
                app.rebuild();
            }
        }
    }
}
