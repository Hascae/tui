//! The slice of the telnet protocol PTT negotiates over its WebSocket endpoint.

const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;

const BINARY: u8 = 0;
const ECHO: u8 = 1;
const SGA: u8 = 3;
const TTYPE: u8 = 24;
const NAWS: u8 = 31;

const TTYPE_IS: u8 = 0;
const TTYPE_SEND: u8 = 1;
const TERMINAL_TYPE: &[u8] = b"VT100";

const MAX_SUBNEGOTIATION: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Data,
    Command,
    Option(u8),
    Sub,
    SubCommand,
}

pub struct Telnet {
    state: State,
    sub: Vec<u8>,
    cols: u8,
    rows: u8,
    naws: bool,
}

impl Telnet {
    pub fn new(cols: u8, rows: u8) -> Self {
        Telnet { state: State::Data, sub: Vec::new(), cols, rows, naws: false }
    }

    /// Splits server bytes into screen data and negotiation, answering the latter.
    pub fn receive(&mut self, input: &[u8], data: &mut Vec<u8>, reply: &mut Vec<u8>) {
        for &byte in input {
            match self.state {
                State::Data => {
                    if byte == IAC {
                        self.state = State::Command;
                    } else {
                        data.push(byte);
                    }
                }
                State::Command => match byte {
                    IAC => {
                        data.push(IAC);
                        self.state = State::Data;
                    }
                    DO | DONT | WILL | WONT => self.state = State::Option(byte),
                    SB => {
                        self.sub.clear();
                        self.state = State::Sub;
                    }
                    _ => self.state = State::Data,
                },
                State::Option(command) => {
                    self.negotiate(command, byte, reply);
                    self.state = State::Data;
                }
                State::Sub => {
                    if byte == IAC {
                        self.state = State::SubCommand;
                    } else if self.sub.len() < MAX_SUBNEGOTIATION {
                        self.sub.push(byte);
                    }
                }
                State::SubCommand => match byte {
                    SE => {
                        self.subnegotiate(reply);
                        self.state = State::Data;
                    }
                    IAC => {
                        if self.sub.len() < MAX_SUBNEGOTIATION {
                            self.sub.push(IAC);
                        }
                        self.state = State::Sub;
                    }
                    _ => self.state = State::Data,
                },
            }
        }
    }

    fn negotiate(&mut self, command: u8, option: u8, reply: &mut Vec<u8>) {
        match (command, option) {
            (DO, NAWS) => {
                self.naws = true;
                reply.extend_from_slice(&[IAC, WILL, NAWS]);
                self.push_window_size(reply);
            }
            (DO, TTYPE | BINARY | SGA) => reply.extend_from_slice(&[IAC, WILL, option]),
            (DO, _) => reply.extend_from_slice(&[IAC, WONT, option]),
            (WILL, ECHO | SGA | BINARY) => reply.extend_from_slice(&[IAC, DO, option]),
            (WILL, _) => reply.extend_from_slice(&[IAC, DONT, option]),
            _ => {}
        }
    }

    fn subnegotiate(&mut self, reply: &mut Vec<u8>) {
        if self.sub == [TTYPE, TTYPE_SEND] {
            reply.extend_from_slice(&[IAC, SB, TTYPE, TTYPE_IS]);
            reply.extend_from_slice(TERMINAL_TYPE);
            reply.extend_from_slice(&[IAC, SE]);
        }
    }

    fn push_window_size(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&[IAC, SB, NAWS, 0, self.cols, 0, self.rows, IAC, SE]);
    }

    /// Records a new height and, once the server has asked for sizes, reports it.
    pub fn resize(&mut self, rows: u8, out: &mut Vec<u8>) {
        self.rows = rows;
        if self.naws {
            self.push_window_size(out);
        }
    }

    /// Escapes user bytes for the wire.
    pub fn send(input: &[u8], out: &mut Vec<u8>) {
        for &byte in input {
            if byte == IAC {
                out.push(IAC);
            }
            out.push(byte);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(telnet: &mut Telnet, input: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let (mut data, mut reply) = (Vec::new(), Vec::new());
        telnet.receive(input, &mut data, &mut reply);
        (data, reply)
    }

    #[test]
    fn answers_the_options_ptt_opens_with() {
        let mut telnet = Telnet::new(80, 24);
        let hello = [
            IAC, DO, TTYPE, IAC, SB, TTYPE, TTYPE_SEND, IAC, SE, IAC, DO, NAWS, IAC, WILL, ECHO, IAC, WILL,
            SGA, IAC, WILL, BINARY, IAC, DO, BINARY,
        ];
        let (data, reply) = run(&mut telnet, &hello);
        assert!(data.is_empty());
        let mut expected = vec![IAC, WILL, TTYPE, IAC, SB, TTYPE, TTYPE_IS];
        expected.extend_from_slice(b"VT100");
        expected.extend_from_slice(&[IAC, SE, IAC, WILL, NAWS, IAC, SB, NAWS, 0, 80, 0, 24, IAC, SE]);
        expected.extend_from_slice(&[IAC, DO, ECHO, IAC, DO, SGA, IAC, DO, BINARY, IAC, WILL, BINARY]);
        assert_eq!(reply, expected);
    }

    #[test]
    fn refuses_unknown_options() {
        let mut telnet = Telnet::new(80, 24);
        let (_, reply) = run(&mut telnet, &[IAC, DO, 39, IAC, WILL, 5]);
        assert_eq!(reply, [IAC, WONT, 39, IAC, DONT, 5]);
    }

    #[test]
    fn passes_data_through_across_chunk_boundaries() {
        let mut telnet = Telnet::new(80, 24);
        let (first, _) = run(&mut telnet, &[b'a', IAC]);
        let (second, _) = run(&mut telnet, &[IAC, b'b', IAC]);
        let (third, reply) = run(&mut telnet, &[DO, NAWS, b'c']);
        assert_eq!([first, second, third].concat(), [b'a', IAC, b'b', b'c']);
        assert_eq!(reply[..3], [IAC, WILL, NAWS]);
    }

    #[test]
    fn reports_size_changes_only_after_negotiation() {
        let mut telnet = Telnet::new(80, 24);
        let mut out = Vec::new();
        telnet.resize(30, &mut out);
        assert!(out.is_empty());
        run(&mut telnet, &[IAC, DO, NAWS]);
        telnet.resize(32, &mut out);
        assert_eq!(out, [IAC, SB, NAWS, 0, 80, 0, 32, IAC, SE]);
    }

    #[test]
    fn doubles_iac_in_outgoing_data() {
        let mut out = Vec::new();
        Telnet::send(&[1, IAC, 2], &mut out);
        assert_eq!(out, [1, IAC, IAC, 2]);
    }
}
