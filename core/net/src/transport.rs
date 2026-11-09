//! The two ways into PTT: telnet over WebSocket, and SSH.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use russh::keys::{HashAlg, PublicKeyOrCertificate};
use russh::{ChannelMsg, Disconnect, client};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};
use tui_term::{COLS, Telnet};

use crate::{Failure, Route};

const WEBSOCKET_URL: &str = "wss://ws.ptt.cc/bbs";
// PTT's proxy only upgrades requests whose Origin it lists; `app://` is the entry it
// keeps for native clients.
const ORIGIN: &str = "app://tui";
const USER_AGENT: &str = concat!("Tui/", env!("CARGO_PKG_VERSION"));

const SSH_HOST: (&str, u16) = ("ptt.cc", 22);
const SSH_USER: &str = "bbs";
const SSH_TERM: &str = "vt100";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub(crate) enum Transport {
    WebSocket { socket: Box<Socket>, telnet: Telnet },
    Ssh { handle: client::Handle<HostKey>, channel: russh::Channel<client::Msg> },
}

/// Where to connect. Tests point this at a local server.
#[derive(Clone)]
pub(crate) enum Endpoint {
    Ptt(Route),
    WebSocket(String),
}

pub(crate) struct Connected {
    pub transport: Transport,
    /// SHA-256 fingerprint of the SSH host key that was accepted.
    pub host_key: Option<String>,
}

pub(crate) async fn connect(
    endpoint: Endpoint,
    rows: u8,
    known_host_key: Option<String>,
) -> Result<Connected, Failure> {
    let attempt = async {
        match endpoint {
            Endpoint::Ptt(Route::WebSocket) => websocket(WEBSOCKET_URL, rows).await,
            Endpoint::WebSocket(url) => websocket(&url, rows).await,
            Endpoint::Ptt(Route::Ssh) => ssh(rows, known_host_key).await,
        }
    };
    match tokio::time::timeout(CONNECT_TIMEOUT, attempt).await {
        Ok(result) => result,
        Err(_) => Err(Failure::Network("connection timed out".into())),
    }
}

async fn websocket(url: &str, rows: u8) -> Result<Connected, Failure> {
    let mut request = url.into_client_request().map_err(network)?;
    let headers = request.headers_mut();
    headers.insert("Origin", HeaderValue::from_static(ORIGIN));
    headers.insert("User-Agent", HeaderValue::from_static(USER_AGENT));

    let connector =
        if url.starts_with("wss://") { Connector::Rustls(tls_config()?) } else { Connector::Plain };
    let (socket, _) = tokio_tungstenite::connect_async_tls_with_config(request, None, true, Some(connector))
        .await
        .map_err(|error| match error {
            tungstenite::Error::Http(response) => {
                Failure::Refused(format!("server answered {}", response.status()))
            }
            other => network(other),
        })?;
    let transport = Transport::WebSocket { socket: Box::new(socket), telnet: Telnet::new(COLS as u8, rows) };
    Ok(Connected { transport, host_key: None })
}

fn tls_config() -> Result<Arc<rustls::ClientConfig>, Failure> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(network)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// Trust on first use: remembers the first host key and refuses any other.
pub(crate) struct HostKey {
    known: Option<String>,
    seen: Arc<std::sync::Mutex<Option<String>>>,
}

impl client::Handler for HostKey {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let fingerprint = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.fingerprint(HashAlg::Sha256).to_string(),
            PublicKeyOrCertificate::Certificate(_) => return Ok(false),
        };
        let trusted = self.known.as_ref().is_none_or(|known| *known == fingerprint);
        *self.seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(fingerprint);
        Ok(trusted)
    }
}

async fn ssh(rows: u8, known: Option<String>) -> Result<Connected, Failure> {
    let seen = Arc::new(std::sync::Mutex::new(None));
    let handler = HostKey { known: known.clone(), seen: Arc::clone(&seen) };
    let config = Arc::new(client::Config { nodelay: true, ..Default::default() });

    let take_seen = || seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
    let mut handle = match client::connect(config, SSH_HOST, handler).await {
        Ok(handle) => handle,
        Err(error) => {
            return Err(match take_seen() {
                Some(fingerprint) if known.is_some_and(|known| known != fingerprint) => {
                    Failure::HostKeyChanged(fingerprint)
                }
                _ => network(error),
            });
        }
    };

    let auth = handle.authenticate_none(SSH_USER).await.map_err(network)?;
    if !auth.success() {
        return Err(Failure::Refused("server did not accept the bbs login".into()));
    }
    let channel = handle.channel_open_session().await.map_err(network)?;
    channel.request_pty(false, SSH_TERM, COLS as u32, rows.into(), 0, 0, &[]).await.map_err(network)?;
    channel.request_shell(false).await.map_err(network)?;
    Ok(Connected { transport: Transport::Ssh { handle, channel }, host_key: take_seen() })
}

fn network(error: impl std::fmt::Display) -> Failure {
    Failure::Network(error.to_string())
}

impl Transport {
    /// Waits for screen data. `Ok(None)` means the server closed the connection.
    ///
    /// Telnet negotiation is answered here and never reaches the caller.
    pub async fn receive(&mut self) -> Result<Option<Vec<u8>>, Failure> {
        match self {
            Transport::WebSocket { socket, telnet } => loop {
                let payload = match socket.next().await {
                    None | Some(Ok(Message::Close(_))) => return Ok(None),
                    Some(Ok(Message::Binary(payload))) => payload,
                    Some(Ok(Message::Text(text))) => text.into(),
                    Some(Ok(_)) => continue,
                    Some(Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed)) => {
                        return Ok(None);
                    }
                    Some(Err(error)) => return Err(network(error)),
                };
                let (mut data, mut reply) = (Vec::with_capacity(payload.len()), Vec::new());
                telnet.receive(&payload, &mut data, &mut reply);
                if !reply.is_empty() {
                    socket.send(Message::Binary(reply.into())).await.map_err(network)?;
                }
                if !data.is_empty() {
                    return Ok(Some(data));
                }
            },
            Transport::Ssh { channel, .. } => loop {
                match channel.wait().await {
                    Some(ChannelMsg::Data { data }) => return Ok(Some(data.to_vec())),
                    Some(ChannelMsg::Eof | ChannelMsg::Close) | None => return Ok(None),
                    Some(_) => {}
                }
            },
        }
    }

    pub async fn send(&mut self, bytes: &[u8]) -> Result<(), Failure> {
        match self {
            Transport::WebSocket { socket, .. } => {
                let mut wire = Vec::with_capacity(bytes.len());
                Telnet::send(bytes, &mut wire);
                socket.send(Message::Binary(wire.into())).await.map_err(network)
            }
            Transport::Ssh { channel, .. } => channel.data_bytes(bytes.to_vec()).await.map_err(network),
        }
    }

    pub async fn resize(&mut self, rows: u8) -> Result<(), Failure> {
        match self {
            Transport::WebSocket { socket, telnet } => {
                let mut wire = Vec::new();
                telnet.resize(rows, &mut wire);
                if wire.is_empty() {
                    return Ok(());
                }
                socket.send(Message::Binary(wire.into())).await.map_err(network)
            }
            Transport::Ssh { channel, .. } => {
                channel.window_change(COLS as u32, rows.into(), 0, 0).await.map_err(network)
            }
        }
    }

    pub async fn close(self) {
        match self {
            Transport::WebSocket { mut socket, .. } => {
                let _ = (*socket).close(None).await;
            }
            Transport::Ssh { handle, .. } => {
                let _ = handle.disconnect(Disconnect::ByApplication, "", "").await;
            }
        }
    }
}
