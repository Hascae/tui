//! Terminal core for PTT: Big5-UAO, telnet negotiation, the screen grid and what can
//! be read off it. No I/O happens here.

pub mod big5;
pub mod screen;
mod telnet;
mod term;

pub use telnet::Telnet;
pub use term::{
    CELL_BLINK, CELL_UNDERLINE, COLS, KIND_LEAD, KIND_SINGLE, KIND_TRAIL, MAX_ROWS, MIN_ROWS, SNAPSHOT_CELL,
    SNAPSHOT_HEADER, SNAPSHOT_VERSION, Terminal,
};
