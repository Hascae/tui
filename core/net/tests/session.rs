use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tui_net::{Config, Events, Failure, Route, Session, State};
use tui_term::{big5, screen::Screen};

const WAIT: Duration = Duration::from_secs(10);

#[derive(Debug, PartialEq)]
enum Event {
    State(State),
    Screen,
    Bell,
}

struct Recorder(Sender<Event>);

impl Events for Recorder {
    fn state(&self, state: State) {
        let _ = self.0.send(Event::State(state));
    }
    fn screen(&self) {
        let _ = self.0.send(Event::Screen);
    }
    fn bell(&self) {
        let _ = self.0.send(Event::Bell);
    }
    fn host_key(&self, _: String) {}
}

fn session() -> (Session, Receiver<Event>) {
    let (sender, receiver) = channel();
    (Session::new(Box::new(Recorder(sender))).unwrap(), receiver)
}

fn config(login: Option<(&str, &str)>) -> Config {
    Config {
        route: Route::WebSocket,
        rows: 24,
        login: login.map(|(account, password)| (account.to_owned(), password.to_owned())),
        host_key: None,
        record: None,
    }
}

fn next_state(events: &Receiver<Event>) -> State {
    loop {
        match events.recv_timeout(WAIT).expect("no state change in time") {
            Event::State(state) => return state,
            _ => continue,
        }
    }
}

fn wait_for(events: &Receiver<Event>, wanted: Event) {
    loop {
        if events.recv_timeout(WAIT).expect("event did not arrive in time") == wanted {
            return;
        }
    }
}

fn big5(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    big5::encode_str(text, &mut out);
    out
}

type ServerSocket = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

async fn read_until(socket: &mut ServerSocket, seen: &mut Vec<u8>, needle: &[u8]) {
    while !seen.windows(needle.len()).any(|window| window == needle) {
        match socket.next().await.expect("client hung up").expect("read failed") {
            Message::Binary(payload) => seen.extend_from_slice(&payload),
            other => panic!("unexpected message {other:?}"),
        }
    }
}

/// Runs `script` against the first client to connect and returns the server's address.
fn serve<F, Fut>(script: F) -> String
where
    F: FnOnce(ServerSocket) -> Fut + Send + 'static,
    Fut: Future<Output = ()>,
{
    let (ready, address) = channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async move {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            ready.send(listener.local_addr().unwrap()).unwrap();
            let (stream, _) = listener.accept().await.unwrap();
            let socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            script(socket).await;
        });
    });
    format!("ws://{}/bbs", address.recv_timeout(WAIT).unwrap())
}

#[test]
fn negotiates_logs_in_and_reports_the_screen() {
    let url = serve(|mut socket| async move {
        let mut seen = Vec::new();
        // IAC DO NAWS, then the welcome screen in one synchronised frame.
        let mut hello = vec![255, 253, 31];
        hello.extend_from_slice(&big5("\x1b[?2026h\x1b[H\x1b[2J批踢踢實業坊\x1b[21;1H請輸入代號，或以 guest 參觀，或以 new 註冊: \x1b[?2026l"));
        socket.send(Message::Binary(hello.into())).await.unwrap();

        read_until(&mut socket, &mut seen, &[255, 250, 31, 0, 80, 0, 24, 255, 240]).await;
        read_until(&mut socket, &mut seen, b"someone\r").await;
        socket.send(Message::Binary(big5("\x1b[22;1H請輸入您的密碼: ").into())).await.unwrap();
        read_until(&mut socket, &mut seen, b"secret\r").await;

        socket.send(Message::Binary(big5("\x1b[H\x1b[2J【主功能表】\x07").into())).await.unwrap();
        read_until(&mut socket, &mut seen, b"typed \xb1\xc0").await;
        assert_eq!(seen.windows(8).filter(|window| window == b"someone\r").count(), 1);
        socket.close(None).await.unwrap();
    });

    let (session, events) = session();
    session.connect_websocket_at(&url, config(Some(("someone", "secret"))));
    assert_eq!(next_state(&events), State::Connecting);
    assert_eq!(next_state(&events), State::Connected);
    wait_for(&events, Event::Bell);
    wait_for(&events, Event::Screen);
    assert_eq!(session.screen(), Screen::MainMenu);

    session.send_text("typed 推");
    assert_eq!(next_state(&events), State::Dropped);
}

#[test]
fn disconnect_closes_cleanly() {
    let url = serve(|mut socket| async move {
        socket.send(Message::Binary(b"hi".to_vec().into())).await.unwrap();
        while let Some(Ok(message)) = socket.next().await {
            if message.is_close() {
                break;
            }
        }
    });

    let (session, events) = session();
    session.connect_websocket_at(&url, config(None));
    assert_eq!(next_state(&events), State::Connecting);
    assert_eq!(next_state(&events), State::Connected);
    wait_for(&events, Event::Screen);
    session.disconnect();
    assert_eq!(next_state(&events), State::Closed);
    // Input after closing goes nowhere and must not panic.
    session.send_text("ignored");
    session.resize(30);
}

#[test]
fn resize_reaches_the_server() {
    let url = serve(|mut socket| async move {
        let mut seen = Vec::new();
        socket.send(Message::Binary(vec![255, 253, 31, b'x'].into())).await.unwrap();
        read_until(&mut socket, &mut seen, &[255, 250, 31, 0, 80, 0, 24, 255, 240]).await;
        read_until(&mut socket, &mut seen, &[255, 250, 31, 0, 80, 0, 40, 255, 240]).await;
        socket.send(Message::Binary(b"\x1b[40;1Hbottom".to_vec().into())).await.unwrap();
        read_until(&mut socket, &mut seen, b"done").await;
    });

    let (session, events) = session();
    session.connect_websocket_at(&url, config(None));
    assert_eq!(next_state(&events), State::Connecting);
    assert_eq!(next_state(&events), State::Connected);
    wait_for(&events, Event::Screen);
    session.resize(40);
    loop {
        wait_for(&events, Event::Screen);
        if session.text_between((39, 0), (39, 79)) == "bottom" {
            break;
        }
    }
    session.send(b"done".to_vec());
}

#[test]
fn reports_an_unreachable_server() {
    let (session, events) = session();
    // Port 1 is never listening.
    session.connect_websocket_at("ws://127.0.0.1:1/bbs", config(None));
    assert_eq!(next_state(&events), State::Connecting);
    assert!(matches!(next_state(&events), State::Failed(Failure::Network(_))));
}

#[test]
fn reports_a_server_that_refuses_the_upgrade() {
    let (ready, address) = channel();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        ready.send(listener.local_addr().unwrap()).unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 2048];
        let _ = stream.read(&mut request);
        let _ = stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
    });
    let url = format!("ws://{}/bbs", address.recv_timeout(WAIT).unwrap());

    let (session, events) = session();
    session.connect_websocket_at(&url, config(None));
    assert_eq!(next_state(&events), State::Connecting);
    assert!(matches!(next_state(&events), State::Failed(Failure::Refused(_))));
}

#[test]
fn a_new_connection_silences_the_old_one() {
    let slow = serve(|mut socket| async move { while socket.next().await.is_some() {} });
    let fast = serve(|mut socket| async move {
        socket.send(Message::Binary(b"second".to_vec().into())).await.unwrap();
        while socket.next().await.is_some() {}
    });

    let (session, events) = session();
    session.connect_websocket_at(&slow, config(None));
    assert_eq!(next_state(&events), State::Connecting);
    assert_eq!(next_state(&events), State::Connected);
    session.connect_websocket_at(&fast, config(None));
    // Only the new connection's states may follow; the old one's close is not reported.
    assert_eq!(next_state(&events), State::Connecting);
    assert_eq!(next_state(&events), State::Connected);
    loop {
        wait_for(&events, Event::Screen);
        if session.text_between((0, 0), (0, 79)) == "second" {
            break;
        }
    }
}
