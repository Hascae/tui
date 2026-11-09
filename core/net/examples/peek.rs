//! Connects to PTT, prints the first screen as text, and leaves.
//!
//! cargo run -p tui-net --example peek [ssh]
//!
//! Set TUI_RECORD to a file path to keep the raw stream as a replay fixture.

use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use tui_net::{Config, Events, Route, Session, State};

struct Printer(Sender<State>);

impl Events for Printer {
    fn state(&self, state: State) {
        let _ = self.0.send(state);
    }
    fn screen(&self) {}
    fn bell(&self) {}
    fn host_key(&self, fingerprint: String) {
        eprintln!("host key {fingerprint}");
    }
}

fn main() -> std::io::Result<()> {
    let route = if std::env::args().nth(1).as_deref() == Some("ssh") { Route::Ssh } else { Route::WebSocket };
    let (sender, states) = channel();
    let session = Session::new(Box::new(Printer(sender)))?;
    let record = std::env::var_os("TUI_RECORD").map(Into::into);
    session.connect(Config { route, rows: 24, login: None, host_key: None, record });

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut connected = false;
    while Instant::now() < deadline {
        match states.recv_timeout(Duration::from_secs(4)) {
            Ok(State::Connected) => connected = true,
            Ok(state @ (State::Failed(_) | State::Dropped | State::Closed)) => {
                eprintln!("{state:?}");
                break;
            }
            Ok(_) => {}
            // Quiet for a while after connecting: the welcome screen is complete.
            Err(_) if connected => break,
            Err(_) => {}
        }
    }
    for row in 0..24 {
        println!("{}", session.text_between((row, 0), (row, 79)));
    }
    eprintln!("{:?}", session.screen());
    session.disconnect();
    Ok(())
}
