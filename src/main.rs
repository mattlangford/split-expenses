mod app;
mod cli;
mod money;
mod pi;
mod split;
mod state;
mod types;
mod ui;

use std::time::Duration;

use clap::Parser;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

fn main() -> anyhow::Result<()> {
    let args = cli::Args::parse();

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        hook(info);
    }));

    let mut terminal = ratatui::init();
    let mut app = app::App::new(args);
    let result = run(&mut terminal, &mut app);
    ratatui::restore();
    app.print_terminal_messages();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut app::App) -> anyhow::Result<()> {
    while !app.quit {
        app.tick();
        terminal.draw(|frame| ui::render(frame, app))?;
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.handle_key(key);
        }
    }
    Ok(())
}
