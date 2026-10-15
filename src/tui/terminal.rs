use std::io::{self, IsTerminal};
use std::time::Duration;

use crossterm::event::{self, Event};
use ratatui::DefaultTerminal;
use tokio::task;

const EVENT_POLL_TIMEOUT: Duration = Duration::from_millis(250);

pub(crate) struct TerminalSession {
    terminal: DefaultTerminal,
}

impl TerminalSession {
    pub(crate) fn new() -> io::Result<Self> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err(io::Error::other(
                "This command requires an interactive terminal",
            ));
        }

        let terminal: DefaultTerminal = match ratatui::try_init() {
            Ok(terminal) => terminal,
            Err(error) => {
                ratatui::restore();
                return Err(error);
            }
        };

        Ok(Self { terminal })
    }

    #[inline]
    pub(crate) fn terminal(&mut self) -> &mut DefaultTerminal {
        &mut self.terminal
    }
}

impl Drop for TerminalSession {
    #[inline]
    fn drop(&mut self) {
        ratatui::restore();
    }
}

fn poll_event() -> io::Result<Option<Event>> {
    if event::poll(EVENT_POLL_TIMEOUT)? {
        return event::read().map(Some);
    }

    Ok(None)
}

#[inline]
pub(super) async fn read_event() -> io::Result<Option<Event>> {
    // Crossterm's event::read is blocking; keep it away from the Tokio workers.
    // Await the bounded read before handing the terminal to another screen.
    task::spawn_blocking(poll_event)
        .await
        .map_err(io::Error::other)?
}
