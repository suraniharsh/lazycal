//! Dev tool: render the TUI into an in-memory `TestBackend` and dump each
//! frame as plain text, so layout can be inspected without a real terminal.
//!
//! Reads whatever is already in the local cache; run `sync_once` first for
//! real data. Run with: cargo run --example preview

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lazycal::app::App;
use lazycal::{config, ui};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

const WIDTH: u16 = 100;
const HEIGHT: u16 = 30;

fn main() -> Result<()> {
    let paths = config::resolve()?;
    let mut app = App::open(&paths.database)?;

    show("month", &app);

    for (label, code) in [
        ("week", 'w'),
        ("day", 'd'),
        ("agenda", 'a'),
        ("back to month", 'm'),
    ] {
        press(&mut app, KeyCode::Char(code));
        show(label, &app);
    }

    press(&mut app, KeyCode::PageDown);
    show("next month", &app);
    press(&mut app, KeyCode::PageUp);

    press(&mut app, KeyCode::Char('c'));
    show("sidebar focused", &app);

    if let Some(name) = app.calendars().first().map(|cal| cal.summary.clone()) {
        press(&mut app, KeyCode::Char(' '));
        show(&format!("after hiding {name:?}"), &app);

        press(&mut app, KeyCode::Char(' '));
        show(&format!("after showing {name:?} again"), &app);
    } else {
        println!("\n(no calendars cached — run `cargo run --example sync_once` first)");
    }

    Ok(())
}

fn press(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn show(label: &str, app: &App) {
    println!("\n=== {label} ===");
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("test terminal");
    terminal.draw(|frame| ui::draw(frame, app)).expect("draw");
    dump(terminal.backend().buffer());
}

fn dump(buffer: &Buffer) {
    for y in 0..buffer.area.height {
        let row: String = (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect();
        println!("{}", row.trim_end());
    }
}
