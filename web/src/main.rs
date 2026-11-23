//! scry's viewer in a browser, drawn by Ratzilla, on a report saved with `--format json`.
//! The configurations arrive one by one, as in a live run.
//!
//! `nix run .#web-demo -- OUT_DIR` writes the fixture's report to `report.json`, puts the font
//! in `fonts/` and builds the page into `OUT_DIR`; the docs site's prebuild runs it.
//!
//! Ratzilla's DOM backend rather than WebGL2: the browser draws the text, so it is crisp at any
//! pixel ratio and can be selected, and the wasm is about half the size (#34).

use std::cell::RefCell;
use std::io;
use std::rc::Rc;

use ratzilla::event::{KeyCode as WebCode, KeyEvent as WebEvent};
use ratzilla::ratatui::buffer::Buffer;
use ratzilla::ratatui::style::Color;
use ratzilla::ratatui::{Frame, Terminal};
use ratzilla::{DomBackend, WebRenderer};
use scry::report::Report;
use scry::tui::app::{App, KeyCode, KeyEvent};
use scry::tui::{Options, draw};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::HtmlElement;

/// Written by the build (see above), not checked in.
const REPORT: &str = include_str!("../report.json");

/// Animation frames between two arrivals: about 0.4 s at 60 frames a second.
const ARRIVAL_FRAMES: u32 = 24;

/// Declared with `@font-face` in `index.html`.
const FONT: &str = "JetBrains Mono";
const FONT_SIZE: u32 = 14;

/// What the viewer's named colours become. Ratzilla would draw them in xterm's dark defaults
/// (`#800000` for red) on black.
struct Palette {
    background: (u8, u8, u8),
    foreground: (u8, u8, u8),
    red: (u8, u8, u8),
    green: (u8, u8, u8),
    yellow: (u8, u8, u8),
    magenta: (u8, u8, u8),
    cyan: (u8, u8, u8),
}

/// Tokyo Night.
const PALETTE: Palette = Palette {
    background: (0x1a, 0x1b, 0x26),
    foreground: (0xc0, 0xca, 0xf5),
    red: (0xf7, 0x76, 0x8e),
    green: (0x9e, 0xce, 0x6a),
    yellow: (0xe0, 0xaf, 0x68),
    magenta: (0xbb, 0x9a, 0xf7),
    cyan: (0x7d, 0xcf, 0xff),
};

fn main() {
    // The backend measures a cell once, at start, so the font must be loaded before it.
    wasm_bindgen_futures::spawn_local(async {
        for weight in ["", "bold "] {
            if let Some(document) = ratzilla::web_sys::window().and_then(|w| w.document()) {
                let promise = document
                    .fonts()
                    .load(&format!("{weight}{FONT_SIZE}px '{FONT}'"));
                // A font that fails to load leaves the browser's monospace fallback.
                let _ = JsFuture::from(promise).await;
            }
        }
        if let Err(error) = start() {
            ratzilla::web_sys::console::error_1(&error.to_string().into());
        }
    });
}

fn start() -> io::Result<()> {
    let report: Report = serde_json::from_str(REPORT).map_err(io::Error::other)?;
    let names = report
        .configurations
        .iter()
        .map(|c| c.name.clone())
        .collect();
    let options = Options {
        expand_churn: false,
        color: true,
        ..Options::default()
    };
    let app = Rc::new(RefCell::new(App::new(report.mode, names, options)));
    let mut pending = report.configurations.into_iter();
    let mut frame = 0;

    let mut terminal = Terminal::new(DomBackend::new()?)?;
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
    // Keys reach the grid only while it has focus; a click gives it back.
    if let Some(grid) = ratzilla::web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.query_selector("[tabindex]").ok().flatten())
        .and_then(|e| e.dyn_into::<HtmlElement>().ok())
    {
        let _ = grid.focus();
    }
    terminal.draw_web(move |f: &mut Frame| {
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
        paint(f.buffer_mut());
    });
    Ok(())
}

/// Puts [`PALETTE`] on the drawn cells. `DIM` is left to the backend, which draws it at half
/// opacity over the background.
fn paint(buffer: &mut Buffer) {
    let p = &PALETTE;
    for cell in &mut buffer.content {
        let fg = match cell.fg {
            Color::Red => p.red,
            Color::Green => p.green,
            Color::Yellow => p.yellow,
            Color::Magenta => p.magenta,
            Color::Cyan => p.cyan,
            _ => p.foreground,
        };
        cell.fg = rgb(fg);
        cell.bg = rgb(p.background);
    }
}

fn rgb((r, g, b): (u8, u8, u8)) -> Color {
    Color::Rgb(r, g, b)
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
