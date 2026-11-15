//! scry's viewer in a browser, drawn by Ratzilla, on a report saved with `--format json`.
//! The configurations arrive one by one, as in a live run.
//!
//! Build from this directory, with the target and tools from nixpkgs:
//!
//! ```sh
//! (cd .. && SCRY_FIXTURE=1 cargo run -- colmena --format json) > report.json
//! nix shell nixpkgs#rustc nixpkgs#cargo nixpkgs#trunk nixpkgs#lld -c trunk build --release
//! ```
//!
//! WebGL2 rather than Ratzilla's DOM backend: in 0.3.1 `DomBackend::size` assumes 10×20 px
//! cells instead of the measured ones, so the frame covers only part of the grid.

use std::cell::RefCell;
use std::io;
use std::rc::Rc;

use ratzilla::event::{KeyCode as WebCode, KeyEvent as WebEvent};
use ratzilla::ratatui::Terminal;
use ratzilla::{WebGl2Backend, WebRenderer};
use scry::report::Report;
use scry::tui::app::{App, KeyCode, KeyEvent};
use scry::tui::{Options, draw};

/// Written by the build (see above), not checked in.
const REPORT: &str = include_str!("../report.json");

/// Animation frames between two arrivals: about 0.4 s at 60 frames a second.
const ARRIVAL_FRAMES: u32 = 24;

fn main() -> io::Result<()> {
    let report: Report = serde_json::from_str(REPORT).map_err(io::Error::other)?;
    let names = report
        .configurations
        .iter()
        .map(|c| c.name.clone())
        .collect();
    let options = Options {
        expand_churn: false,
        color: true,
    };
    let app = Rc::new(RefCell::new(App::new(report.mode, names, options)));
    let mut pending = report.configurations.into_iter();
    let mut frame = 0;

    let mut terminal = Terminal::new(WebGl2Backend::new()?)?;
    terminal.on_key_event({
        let app = Rc::clone(&app);
        move |event| {
            let mut app = app.borrow_mut();
            app.key(key(event));
            // shortcut: `y`/`Y` copy nothing here (OSC 52 needs a terminal); wire up
            // navigator.clipboard if the demo should copy.
            app.clipboard = None;
            // `q` has nothing to quit to in a page.
            app.quit = false;
        }
    })?;
    terminal.draw_web(move |f| {
        frame += 1;
        let mut app = app.borrow_mut();
        if frame % ARRIVAL_FRAMES == 0
            && let Some(configuration) = pending.next()
        {
            app.arrive(configuration);
        }
        let area = f.area();
        app.resize(area.width, area.height);
        draw::draw(&app, f);
    });
    Ok(())
}

fn key(event: WebEvent) -> KeyEvent {
    let code = match event.code {
        WebCode::Tab if event.shift => KeyCode::BackTab,
        WebCode::Char(c) => KeyCode::Char(c),
        WebCode::Enter => KeyCode::Enter,
        WebCode::Esc => KeyCode::Esc,
        WebCode::Backspace => KeyCode::Backspace,
        WebCode::Tab => KeyCode::Tab,
        WebCode::Up => KeyCode::Up,
        WebCode::Down => KeyCode::Down,
        WebCode::PageUp => KeyCode::PageUp,
        WebCode::PageDown => KeyCode::PageDown,
        _ => KeyCode::Other,
    };
    KeyEvent {
        code,
        ctrl: event.ctrl,
        alt: event.alt,
    }
}
