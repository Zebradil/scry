//! The viewer on a terminal: crossterm's raw mode, alternate screen and key events, and a panic
//! guard that restores the terminal. Native only; the browser build drives [`App`] itself.

use std::io::{self, Write};
use std::panic::PanicHookInfo;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self as term, Event, KeyEventKind, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{EnterAlternateScreen, enable_raw_mode};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::error::TryRecvError;

use super::app::{App, KeyCode, KeyEvent};
use super::{Finished, Options, draw};
use crate::report::{Configuration, Report};

/// Runs the viewer on the calling thread until the user quits or `quit` is set (a signal, or a
/// failure on the run's side), then restores the terminal, also on an error or a panic.
///
/// `header` gives the mode (its configurations and shared changes are ignored); `names` are
/// the selected configurations in name order, each sent once on `events` when final.
pub fn run(
    header: Report,
    names: Vec<String>,
    mut events: UnboundedReceiver<Configuration>,
    options: Options,
    quit: &AtomicBool,
) -> io::Result<Finished> {
    let mut app = App::new(header.mode, names, options);
    let _guard = Guard::install(ratatui::restore);
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut connected = true;
    let mut dirty = true;
    while !app.quit && !quit.load(Ordering::Relaxed) {
        while connected {
            match events.try_recv() {
                Ok(configuration) => {
                    app.arrive(configuration);
                    dirty = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => connected = false,
            }
        }
        if dirty {
            let size = terminal.size()?;
            app.resize(size.width, size.height);
            terminal.draw(|frame| draw::draw(&app, frame))?;
            dirty = false;
        }
        if !term::poll(Duration::from_millis(100))? {
            continue;
        }
        match term::read()? {
            Event::Key(event) if event.kind != KeyEventKind::Release => app.key(key(event)),
            Event::Resize(..) => {}
            _ => continue,
        }
        dirty = true;
        if let Some(payload) = app.clipboard.take() {
            // Past ratatui's buffer: the escape draws nothing, so the screen stays in sync.
            let out = terminal.backend_mut();
            out.write_all(payload.as_bytes())?;
            out.flush()?;
        }
    }
    Ok(finish(app, &mut events))
}

/// What arrived since the last poll is finished too.
pub(super) fn finish(mut app: App, events: &mut UnboundedReceiver<Configuration>) -> Finished {
    while let Ok(configuration) = events.try_recv() {
        app.arrive(configuration);
    }
    app.finish()
}

type Hook = Box<dyn Fn(&PanicHookInfo<'_>) + Send + Sync>;

/// Leaves raw mode and the alternate screen on every way out of [`run`], with a panic hook for
/// its lifetime. A panic on the viewer's thread restores the terminal before its message
/// prints. Another thread's message waits until the terminal is restored: printed now it would
/// paint over the screen, and restoring for it would leave the viewer running on a cooked
/// terminal.
pub(super) struct Guard {
    /// `ratatui::restore`; a stand-in in tests.
    restore: fn(),
    /// The hook before ours, put back on drop.
    previous: Option<Arc<Hook>>,
    /// Other threads' panic messages; `None` once the terminal is restored, when ours passes
    /// every panic on to `previous`.
    pub(super) held: Arc<Mutex<Option<Vec<String>>>>,
}

impl Guard {
    pub(super) fn install(restore: fn()) -> Self {
        let previous: Arc<Hook> = Arc::new(std::panic::take_hook());
        let held = Arc::new(Mutex::new(Some(Vec::new())));
        let viewer = std::thread::current().id();
        let (hook_previous, hook_held) = (Arc::clone(&previous), Arc::clone(&held));
        std::panic::set_hook(Box::new(move |info| {
            let thread = std::thread::current();
            let mut held = lock(&hook_held);
            if let Some(messages) = held.as_mut() {
                if thread.id() != viewer {
                    let name = thread.name().unwrap_or("<unnamed>");
                    messages.push(format!("thread '{name}' {info}"));
                    return;
                }
                restore();
                print(held.take());
            }
            drop(held);
            hook_previous(info);
        }));
        Self {
            restore,
            previous: Some(previous),
            held,
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        (self.restore)();
        print(lock(&self.held).take());
        // `set_hook` panics on a panicking thread; ours passes everything on by now anyway.
        if std::thread::panicking() {
            return;
        }
        drop(std::panic::take_hook());
        if let Some(previous) = self.previous.take() {
            let hook = Arc::try_unwrap(previous)
                .unwrap_or_else(|shared| Box::new(move |info: &PanicHookInfo<'_>| shared(info)));
            std::panic::set_hook(hook);
        }
    }
}

pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn print(messages: Option<Vec<String>>) {
    for message in messages.into_iter().flatten() {
        // Nothing to do when stderr is gone; panicking in a panic hook would abort.
        let _ = writeln!(io::stderr(), "{message}");
    }
}

fn key(event: term::KeyEvent) -> KeyEvent {
    use term::KeyCode as C;
    KeyEvent {
        code: match event.code {
            C::Char(c) => KeyCode::Char(c),
            C::Enter => KeyCode::Enter,
            C::Esc => KeyCode::Esc,
            C::Backspace => KeyCode::Backspace,
            C::Tab => KeyCode::Tab,
            C::BackTab => KeyCode::BackTab,
            C::Up => KeyCode::Up,
            C::Down => KeyCode::Down,
            C::PageUp => KeyCode::PageUp,
            C::PageDown => KeyCode::PageDown,
            _ => KeyCode::Other,
        },
        ctrl: event.modifiers.contains(KeyModifiers::CONTROL),
        alt: event.modifiers.contains(KeyModifiers::ALT),
    }
}
