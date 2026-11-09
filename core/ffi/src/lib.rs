//! The interface the Android app sees. UniFFI generates the Kotlin side from this file.

mod zhuyin;

use std::sync::Arc;

use tui_net::{Config, Events, Failure, State};

pub use zhuyin::{Composition, Zhuyin};

uniffi::setup_scaffolding!();

#[derive(uniffi::Enum, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Route {
    WebSocket,
    Ssh,
}

#[derive(uniffi::Record)]
pub struct ConnectOptions {
    pub route: Route,
    pub rows: u8,
    /// Typed at the login prompts when both are present.
    pub account: Option<String>,
    pub password: Option<String>,
    /// Fingerprint the SSH host key must match; none trusts the first key seen.
    pub host_key: Option<String>,
}

#[derive(uniffi::Enum, Clone, PartialEq, Eq, Debug)]
pub enum ConnectionState {
    Connecting,
    Connected,
    /// Closed because the app asked.
    Closed,
    /// The server ended the session.
    Dropped,
    NetworkFailed {
        detail: String,
    },
    Refused {
        detail: String,
    },
    /// The SSH host no longer presents the key that was remembered.
    HostKeyChanged {
        fingerprint: String,
    },
}

#[derive(uniffi::Enum, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    Unknown,
    Login,
    Password,
    AnyKey,
    MainMenu,
    BoardList,
    ArticleList,
    Article,
}

#[derive(uniffi::Error, Debug)]
pub enum CoreError {
    Startup { detail: String },
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoreError::Startup { detail } => write!(f, "could not start the session runtime: {detail}"),
        }
    }
}

impl std::error::Error for CoreError {}

/// Called from the session's own threads, never from the caller's.
#[uniffi::export(callback_interface)]
pub trait SessionListener: Send + Sync {
    fn on_state(&self, state: ConnectionState);
    /// The screen changed; fetch a snapshot when convenient.
    fn on_screen(&self);
    fn on_bell(&self);
    /// First contact with the SSH host. Store the fingerprint and pass it to later connects.
    fn on_host_key(&self, fingerprint: String);
}

struct Bridge(Box<dyn SessionListener>);

impl Events for Bridge {
    fn state(&self, state: State) {
        let state = match state {
            State::Idle => return,
            State::Connecting => ConnectionState::Connecting,
            State::Connected => ConnectionState::Connected,
            State::Closed => ConnectionState::Closed,
            State::Dropped => ConnectionState::Dropped,
            State::Failed(Failure::Network(detail)) => ConnectionState::NetworkFailed { detail },
            State::Failed(Failure::Refused(detail)) => ConnectionState::Refused { detail },
            State::Failed(Failure::HostKeyChanged(fingerprint)) => {
                ConnectionState::HostKeyChanged { fingerprint }
            }
        };
        self.0.on_state(state);
    }

    fn screen(&self) {
        self.0.on_screen();
    }

    fn bell(&self) {
        self.0.on_bell();
    }

    fn host_key(&self, fingerprint: String) {
        self.0.on_host_key(fingerprint);
    }
}

#[derive(uniffi::Object)]
pub struct Session {
    inner: tui_net::Session,
}

#[uniffi::export]
impl Session {
    #[uniffi::constructor]
    pub fn new(listener: Box<dyn SessionListener>) -> Result<Arc<Self>, CoreError> {
        let inner = tui_net::Session::new(Box::new(Bridge(listener)))
            .map_err(|error| CoreError::Startup { detail: error.to_string() })?;
        Ok(Arc::new(Session { inner }))
    }

    /// Starts a connection, replacing any current one. Progress arrives on the listener.
    pub fn connect(&self, options: ConnectOptions) {
        let login = match (options.account, options.password) {
            (Some(account), Some(password)) if !account.is_empty() => Some((account, password)),
            _ => None,
        };
        self.inner.connect(Config {
            route: match options.route {
                Route::WebSocket => tui_net::Route::WebSocket,
                Route::Ssh => tui_net::Route::Ssh,
            },
            rows: options.rows,
            login,
            host_key: options.host_key,
            record: None,
        });
    }

    pub fn disconnect(&self) {
        self.inner.disconnect();
    }

    /// Sends raw key bytes.
    pub fn send(&self, bytes: Vec<u8>) {
        self.inner.send(bytes);
    }

    /// Sends text in the encoding PTT expects.
    pub fn send_text(&self, text: String) {
        self.inner.send_text(&text);
    }

    pub fn resize(&self, rows: u8) {
        self.inner.resize(rows);
    }

    /// The screen in the binary layout documented on `tui_term::Terminal::snapshot`.
    pub fn snapshot(&self) -> Vec<u8> {
        self.inner.snapshot()
    }

    pub fn screen(&self) -> Screen {
        use tui_term::screen::Screen as Seen;
        match self.inner.screen() {
            Seen::Unknown => Screen::Unknown,
            Seen::Login => Screen::Login,
            Seen::Password => Screen::Password,
            Seen::AnyKey => Screen::AnyKey,
            Seen::MainMenu => Screen::MainMenu,
            Seen::BoardList => Screen::BoardList,
            Seen::ArticleList => Screen::ArticleList,
            Seen::Article => Screen::Article,
        }
    }

    pub fn link_at(&self, row: u32, col: u32) -> Option<String> {
        self.inner.link_at(row as usize, col as usize)
    }

    /// Text between two cells in reading order, both inclusive.
    pub fn text_between(&self, start_row: u32, start_col: u32, end_row: u32, end_col: u32) -> String {
        self.inner
            .text_between((start_row as usize, start_col as usize), (end_row as usize, end_col as usize))
    }
}
