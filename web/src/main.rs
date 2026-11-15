//! scry's viewer in a browser, drawn by Ratzilla, on a report saved with `--format json`.
//! The configurations arrive one by one, as in a live run.
//!
//! `nix run .#web-demo -- OUT_DIR` writes the fixture's report to `report.json`, puts the font
//! in `fonts/` and builds the page into `OUT_DIR`; the docs site's prebuild runs it.
//!
//! WebGL2 rather than Ratzilla's DOM backend: in 0.3.1 `DomBackend::size` assumes 10×20 px
//! cells instead of the measured ones, so the frame covers only part of the grid (#34).

use std::cell::RefCell;
use std::io;
use std::rc::Rc;

use ratzilla::backend::webgl2::{FontAtlasConfig, WebGl2BackendOptions};
use ratzilla::event::{KeyCode as WebCode, KeyEvent as WebEvent};
use ratzilla::ratatui::buffer::Buffer;
use ratzilla::ratatui::style::{Color, Modifier};
use ratzilla::ratatui::{Frame, Terminal};
use ratzilla::{WebGl2Backend, WebRenderer};
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
const FONT_SIZE: f32 = 14.0;

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
    // The atlas is drawn from the font once, so the font must be loaded before the backend.
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
    };
    let app = Rc::new(RefCell::new(App::new(report.mode, names, options)));
    let mut pending = report.configurations.into_iter();
    let mut frame = 0;

    let backend = WebGl2Backend::new_with_options(
        WebGl2BackendOptions::new()
            .font_atlas_config(FontAtlasConfig::dynamic(&[FONT], FONT_SIZE))
            .canvas_padding_color(rgb(PALETTE.background)),
    )?;
    let mut terminal = Terminal::new(backend)?;
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
    // Keys reach the canvas only while it has focus; a click gives it back.
    if let Some(canvas) = ratzilla::web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.query_selector("canvas").ok().flatten())
        .and_then(|e| e.dyn_into::<HtmlElement>().ok())
    {
        let _ = canvas.focus();
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

/// Puts [`PALETTE`] on the drawn cells, and fades `DIM` ones towards the background, since the
/// WebGL2 backend ignores that modifier.
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
        let fg = if cell.modifier.contains(Modifier::DIM) {
            mix(fg, p.background, 0.45)
        } else {
            fg
        };
        cell.fg = rgb(fg);
        cell.bg = rgb(p.background);
    }
}

/// `a` moved towards `b` by `t`.
fn mix(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let m = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
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
