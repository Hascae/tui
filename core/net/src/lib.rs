//! A PTT session: one connection, the screen it paints, and the bytes typed into it.

mod transport;

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tui_term::screen::{self, LoginAssist, Screen};
use tui_term::{MAX_ROWS, MIN_ROWS, Terminal, big5};

use transport::{Endpoint, Transport};

// A frame the server never closes still has to reach the screen.
const SYNC_FRAME_LIMIT: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Route {
    WebSocket,
    Ssh,
}

pub struct Config {
    pub route: Route,
    pub rows: u8,
    /// Account and password to type at the login prompts.
    pub login: Option<(String, String)>,
    /// Fingerprint the SSH host key must match; `None` trusts the first one seen.
    pub host_key: Option<String>,
    /// Appends every byte the server paints with to this file, for replay tests.
    pub record: Option<PathBuf>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Failure {
    /// The network or the handshake failed.
    Network(String),
    /// The server answered, and said no.
    Refused(String),
    /// The SSH host presented this fingerprint instead of the remembered one.
    HostKeyChanged(String),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum State {
    Idle,
    Connecting,
    Connected,
    /// Closed on request.
    Closed,
    /// The server ended the session.
    Dropped,
    Failed(Failure),
}

/// Session callbacks. They run on the session's own threads.
pub trait Events: Send + Sync + 'static {
    fn state(&self, state: State);
    /// The screen changed and is in a state worth showing.
    fn screen(&self);
    fn bell(&self);
    /// First contact with an SSH host; the fingerprint should be remembered.
    fn host_key(&self, fingerprint: String);
}

enum Command {
    Data(Vec<u8>),
    Resize(u8),
    Close,
}

struct Shared {
    term: Mutex<Terminal>,
    events: Box<dyn Events>,
    // Bumped on every connect so a superseded connection cannot report into the new one.
    epoch: AtomicU64,
    commands: Mutex<Option<mpsc::UnboundedSender<Command>>>,
}

impl Shared {
    fn term(&self) -> MutexGuard<'_, Terminal> {
        self.term.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn current(&self, epoch: u64) -> bool {
        self.epoch.load(Ordering::Acquire) == epoch
    }

    fn report(&self, epoch: u64, state: State) {
        if self.current(epoch) {
            self.events.state(state);
        }
    }
}

pub struct Session {
    shared: Arc<Shared>,
    runtime: Runtime,
}

impl Session {
    pub fn new(events: Box<dyn Events>) -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("tui-net")
            .enable_all()
            .build()?;
        let shared = Arc::new(Shared {
            term: Mutex::new(Terminal::new(MIN_ROWS)),
            events,
            epoch: AtomicU64::new(0),
            commands: Mutex::new(None),
        });
        Ok(Session { shared, runtime })
    }

    /// Starts a connection, replacing any current one.
    pub fn connect(&self, config: Config) {
        self.connect_to(Endpoint::Ptt(config.route), config);
    }

    #[doc(hidden)]
    pub fn connect_websocket_at(&self, url: &str, config: Config) {
        self.connect_to(Endpoint::WebSocket(url.to_owned()), config);
    }

    fn connect_to(&self, endpoint: Endpoint, config: Config) {
        let (sender, receiver) = mpsc::unbounded_channel();
        let epoch = {
            let mut commands = self.commands();
            if let Some(previous) = commands.replace(sender) {
                let _ = previous.send(Command::Close);
            }
            self.shared.epoch.fetch_add(1, Ordering::AcqRel) + 1
        };
        let rows = usize::from(config.rows).clamp(MIN_ROWS, MAX_ROWS);
        {
            let mut term = self.shared.term();
            term.reset();
            term.resize(rows);
        }
        self.shared.events.state(State::Connecting);
        self.shared.events.screen();
        self.runtime.spawn(run(Arc::clone(&self.shared), epoch, endpoint, config, receiver));
    }

    pub fn disconnect(&self) {
        if let Some(sender) = self.commands().take() {
            let _ = sender.send(Command::Close);
        }
    }

    fn commands(&self) -> MutexGuard<'_, Option<mpsc::UnboundedSender<Command>>> {
        self.shared.commands.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn command(&self, command: Command) {
        if let Some(sender) = self.commands().as_ref() {
            let _ = sender.send(command);
        }
    }

    /// Sends raw bytes as typed.
    pub fn send(&self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            self.command(Command::Data(bytes));
        }
    }

    /// Sends text, converted to the encoding PTT expects.
    pub fn send_text(&self, text: &str) {
        let mut bytes = Vec::with_capacity(text.len());
        big5::encode_str(text, &mut bytes);
        self.send(bytes);
    }

    /// Changes the terminal height. Applies to the live connection and to later ones.
    pub fn resize(&self, rows: u8) {
        let rows = usize::from(rows).clamp(MIN_ROWS, MAX_ROWS);
        let changed = {
            let mut term = self.shared.term();
            let changed = term.rows() != rows;
            term.resize(rows);
            changed
        };
        if changed {
            self.command(Command::Resize(rows as u8));
            self.shared.events.screen();
        }
    }

    pub fn snapshot(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.shared.term().snapshot(&mut out);
        out
    }

    pub fn screen(&self) -> Screen {
        screen::classify(&self.shared.term())
    }

    pub fn link_at(&self, row: usize, col: usize) -> Option<String> {
        self.shared.term().link_at(row, col)
    }

    pub fn text_between(&self, start: (usize, usize), end: (usize, usize)) -> String {
        self.shared.term().text_between(start, end)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.disconnect();
    }
}

async fn run(
    shared: Arc<Shared>,
    epoch: u64,
    endpoint: Endpoint,
    config: Config,
    mut commands: mpsc::UnboundedReceiver<Command>,
) {
    let rows = shared.term().rows() as u8;
    let first_contact = config.host_key.is_none();
    let connecting = transport::connect(endpoint, rows, config.host_key);
    tokio::pin!(connecting);

    // Stay responsive to a cancel while the handshake is in flight.
    let connected = loop {
        tokio::select! {
            result = &mut connecting => break result,
            command = commands.recv() => {
                if matches!(command, Some(Command::Close) | None) {
                    shared.report(epoch, State::Closed);
                    return;
                }
            }
        }
    };
    let mut transport = match connected {
        Ok(connected) => {
            if let Some(fingerprint) = connected.host_key.filter(|_| first_contact && shared.current(epoch)) {
                shared.events.host_key(fingerprint);
            }
            connected.transport
        }
        Err(failure) => {
            shared.report(epoch, State::Failed(failure));
            return;
        }
    };
    shared.report(epoch, State::Connected);

    let mut login = config.login.map(|(account, password)| LoginAssist::new(account, password));
    let mut record =
        config.record.and_then(|path| std::fs::File::options().create(true).append(true).open(path).ok());
    let end = serve(&shared, epoch, &mut transport, &mut commands, &mut login, &mut record).await;
    transport.close().await;
    shared.report(epoch, end);
}

async fn serve(
    shared: &Shared,
    epoch: u64,
    transport: &mut Transport,
    commands: &mut mpsc::UnboundedReceiver<Command>,
    login: &mut Option<LoginAssist>,
    record: &mut Option<std::fs::File>,
) -> State {
    // The size may have changed while the handshake was running.
    let rows = shared.term().rows() as u8;
    if let Err(failure) = transport.resize(rows).await {
        return State::Failed(failure);
    }

    let mut sync_deadline: Option<Instant> = None;
    loop {
        let sync_timeout = async {
            match sync_deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            command = commands.recv() => {
                let sent = match command {
                    Some(Command::Data(bytes)) => transport.send(&bytes).await,
                    Some(Command::Resize(rows)) => transport.resize(rows).await,
                    Some(Command::Close) | None => return State::Closed,
                };
                if let Err(failure) = sent {
                    return State::Failed(failure);
                }
            }
            received = transport.receive() => {
                let data = match received {
                    Ok(Some(data)) => data,
                    Ok(None) => return State::Dropped,
                    Err(failure) => return State::Failed(failure),
                };
                if let Some(file) = record.as_mut()
                    && file.write_all(&data).is_err()
                {
                    *record = None;
                }
                let (mut replies, bells, in_frame) = {
                    let mut term = shared.term();
                    term.feed(&data);
                    let mut replies = term.take_replies();
                    if let Some(assist) = login.as_mut()
                        && let Some(keys) = assist.step(&term)
                    {
                        replies.extend_from_slice(&keys);
                    }
                    (replies, term.take_bells(), term.in_sync_frame())
                };
                if login.as_ref().is_some_and(LoginAssist::finished) {
                    *login = None;
                }
                if !replies.is_empty() {
                    let sent = transport.send(&replies).await;
                    replies.fill(0);
                    if let Err(failure) = sent {
                        return State::Failed(failure);
                    }
                }
                if !shared.current(epoch) {
                    return State::Closed;
                }
                if bells > 0 {
                    shared.events.bell();
                }
                if in_frame {
                    sync_deadline.get_or_insert_with(|| Instant::now() + SYNC_FRAME_LIMIT);
                } else {
                    sync_deadline = None;
                    shared.events.screen();
                }
            }
            () = sync_timeout => {
                sync_deadline = None;
                shared.events.screen();
            }
        }
    }
}
