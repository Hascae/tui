//! The screen PTT draws on: a byte grid driven by the escape sequences pfterm emits.
//!
//! Cells hold raw bytes, not characters. PTT may change colour between the two bytes of
//! a Big5 character, and may overwrite either half on its own, so pairing bytes into
//! characters only happens when the screen is read.

use crate::big5;

pub const COLS: usize = 80;
pub const MIN_ROWS: usize = 24;
pub const MAX_ROWS: usize = 100;

const BOLD: u8 = 1;
const BLINK: u8 = 1 << 1;
const UNDERLINE: u8 = 1 << 2;
const INVERSE: u8 = 1 << 3;

const DEFAULT_FG: u8 = 7;
const DEFAULT_BG: u8 = 0;

const MAX_PARAMS: usize = 16;
const MAX_OSC: usize = 2048;
const MAX_LINKS: usize = 1024;

pub const SNAPSHOT_VERSION: u8 = 1;
pub const SNAPSHOT_HEADER: usize = 16;
pub const SNAPSHOT_CELL: usize = 8;

pub const KIND_SINGLE: u8 = 0;
pub const KIND_LEAD: u8 = 1;
pub const KIND_TRAIL: u8 = 2;

pub const CELL_BLINK: u8 = 1;
pub const CELL_UNDERLINE: u8 = 1 << 1;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Pen {
    fg: u8,
    bg: u8,
    flags: u8,
}

impl Pen {
    const DEFAULT: Pen = Pen { fg: DEFAULT_FG, bg: DEFAULT_BG, flags: 0 };
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Cell {
    byte: u8,
    pen: Pen,
    link: u16,
}

impl Cell {
    const BLANK: Cell = Cell { byte: b' ', pen: Pen::DEFAULT, link: 0 };
}

type Row = [Cell; COLS];

const BLANK_ROW: Row = [Cell::BLANK; COLS];

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    Osc,
    OscEscape,
}

#[derive(Clone, Copy)]
struct Saved {
    row: usize,
    col: usize,
    pen: Pen,
}

/// One decoded screen position, as [`Terminal::snapshot`] and the text helpers see it.
#[derive(Clone, Copy)]
struct Glyph {
    ch: char,
    kind: u8,
}

pub struct Terminal {
    grid: Vec<Row>,
    row: usize,
    col: usize,
    // Set after writing the last column; the next printable byte wraps first.
    wrap_pending: bool,
    pen: Pen,
    link: u16,
    saved: Saved,
    top: usize,
    bottom: usize,
    cursor_visible: bool,
    state: State,
    params: [u16; MAX_PARAMS],
    param_count: usize,
    private: u8,
    intermediate: bool,
    osc: Vec<u8>,
    links: Vec<String>,
    replies: Vec<u8>,
    bells: u32,
    sync: bool,
    revision: u64,
}

impl Terminal {
    pub fn new(rows: usize) -> Self {
        let rows = rows.clamp(MIN_ROWS, MAX_ROWS);
        Terminal {
            grid: vec![BLANK_ROW; rows],
            row: 0,
            col: 0,
            wrap_pending: false,
            pen: Pen::DEFAULT,
            link: 0,
            saved: Saved { row: 0, col: 0, pen: Pen::DEFAULT },
            top: 0,
            bottom: rows - 1,
            cursor_visible: true,
            state: State::Ground,
            params: [0; MAX_PARAMS],
            param_count: 0,
            private: 0,
            intermediate: false,
            osc: Vec::new(),
            links: Vec::new(),
            replies: Vec::new(),
            bells: 0,
            sync: false,
            revision: 0,
        }
    }

    pub fn rows(&self) -> usize {
        self.grid.len()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    /// True between DEC 2026 begin/end markers: the server is mid-frame and the screen
    /// should not be shown yet.
    pub fn in_sync_frame(&self) -> bool {
        self.sync
    }

    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies)
    }

    pub fn take_bells(&mut self) -> u32 {
        std::mem::take(&mut self.bells)
    }

    pub fn reset(&mut self) {
        let revision = self.revision;
        *self = Terminal::new(self.rows());
        self.revision = revision + 1;
    }

    /// Changes the row count, keeping the top of the screen. The server repaints after
    /// it learns the new size, so nothing clever is done with the content.
    pub fn resize(&mut self, rows: usize) {
        let rows = rows.clamp(MIN_ROWS, MAX_ROWS);
        if rows == self.rows() {
            return;
        }
        self.grid.resize(rows, BLANK_ROW);
        self.top = 0;
        self.bottom = rows - 1;
        self.row = self.row.min(rows - 1);
        self.saved.row = self.saved.row.min(rows - 1);
        self.wrap_pending = false;
        self.revision += 1;
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        for &byte in bytes {
            self.advance(byte);
        }
        self.revision += 1;
    }

    fn advance(&mut self, byte: u8) {
        match self.state {
            State::Ground => self.ground(byte),
            State::Escape => self.escape(byte),
            State::EscapeIntermediate => self.state = State::Ground,
            State::Csi => self.csi(byte),
            State::Osc => match byte {
                0x07 => {
                    self.state = State::Ground;
                    self.osc_dispatch();
                }
                0x1B => self.state = State::OscEscape,
                _ => {
                    if self.osc.len() < MAX_OSC {
                        self.osc.push(byte);
                    }
                }
            },
            State::OscEscape => {
                if byte == b'\\' {
                    self.state = State::Ground;
                    self.osc_dispatch();
                } else {
                    // A stray ESC ends the string; what follows is a new sequence.
                    self.state = State::Escape;
                    self.escape(byte);
                }
            }
        }
    }

    fn ground(&mut self, byte: u8) {
        match byte {
            0x1B => self.state = State::Escape,
            0x00..=0x1F => self.control(byte),
            0x7F => {}
            _ => self.put(byte),
        }
    }

    fn control(&mut self, byte: u8) {
        match byte {
            0x07 => self.bells += 1,
            0x08 => {
                self.col = self.col.saturating_sub(1);
                self.wrap_pending = false;
            }
            0x09 => {
                self.col = ((self.col / 8 + 1) * 8).min(COLS - 1);
                self.wrap_pending = false;
            }
            0x0A..=0x0C => {
                self.wrap_pending = false;
                self.index();
            }
            0x0D => {
                self.col = 0;
                self.wrap_pending = false;
            }
            _ => {}
        }
    }

    fn put(&mut self, byte: u8) {
        if self.wrap_pending {
            self.wrap_pending = false;
            self.col = 0;
            self.index();
        }
        self.grid[self.row][self.col] = Cell { byte, pen: self.pen, link: self.link };
        if self.col == COLS - 1 {
            self.wrap_pending = true;
        } else {
            self.col += 1;
        }
    }

    fn escape(&mut self, byte: u8) {
        self.state = State::Ground;
        match byte {
            b'[' => {
                self.params = [0; MAX_PARAMS];
                self.param_count = 0;
                self.private = 0;
                self.intermediate = false;
                self.state = State::Csi;
            }
            b']' => {
                self.osc.clear();
                self.state = State::Osc;
            }
            b'7' => self.save_cursor(),
            b'8' => self.restore_cursor(),
            b'D' => {
                self.wrap_pending = false;
                self.index();
            }
            b'E' => {
                self.col = 0;
                self.wrap_pending = false;
                self.index();
            }
            b'M' => {
                self.wrap_pending = false;
                self.reverse_index();
            }
            b'c' => self.reset(),
            b'(' | b')' | b'*' | b'+' | b'#' | b'%' => self.state = State::EscapeIntermediate,
            0x1B => self.state = State::Escape,
            0x00..=0x1F => self.control(byte),
            _ => {}
        }
    }

    fn csi(&mut self, byte: u8) {
        match byte {
            b'0'..=b'9' => {
                if self.param_count == 0 {
                    self.param_count = 1;
                }
                if let Some(value) = self.params.get_mut(self.param_count - 1) {
                    *value = value.saturating_mul(10).saturating_add(u16::from(byte - b'0'));
                }
            }
            b';' | b':' => {
                if self.param_count == 0 {
                    self.param_count = 1;
                }
                // Parameters past the limit pile onto a slot nobody reads.
                self.param_count = (self.param_count + 1).min(MAX_PARAMS + 1);
            }
            b'<'..=b'?' => {
                if self.param_count == 0 && self.private == 0 {
                    self.private = byte;
                }
            }
            0x20..=0x2F => self.intermediate = true,
            0x40..=0x7E => {
                self.state = State::Ground;
                self.csi_dispatch(byte);
            }
            0x1B => self.state = State::Escape,
            0x00..=0x1F => self.control(byte),
            _ => {
                self.state = State::Ground;
                self.ground(byte);
            }
        }
    }

    fn param(&self, index: usize, default: u16) -> usize {
        let value = if index < self.param_count.min(MAX_PARAMS) { self.params[index] } else { 0 };
        usize::from(if value == 0 { default } else { value })
    }

    fn csi_dispatch(&mut self, action: u8) {
        if self.private != 0 {
            if self.private == b'?' && matches!(action, b'h' | b'l') {
                self.set_private_modes(action == b'h');
            }
            return;
        }
        if self.intermediate {
            return;
        }
        let last_row = self.rows() - 1;
        let n = self.param(0, 1);
        match action {
            b'A' => self.move_to(self.row.saturating_sub(n), self.col),
            b'B' | b'e' => self.move_to(self.row + n, self.col),
            b'C' | b'a' => self.move_to(self.row, self.col + n),
            b'D' => self.move_to(self.row, self.col.saturating_sub(n)),
            b'E' => self.move_to(self.row + n, 0),
            b'F' => self.move_to(self.row.saturating_sub(n), 0),
            b'G' | b'`' => self.move_to(self.row, n - 1),
            b'd' => self.move_to(n - 1, self.col),
            b'H' | b'f' => self.move_to(n - 1, self.param(1, 1) - 1),
            b'J' => self.erase_display(self.param(0, 0)),
            b'K' => self.erase_line(self.param(0, 0)),
            b'L' => self.insert_lines(n),
            b'M' => self.delete_lines(n),
            b'@' => self.insert_chars(n),
            b'P' => self.delete_chars(n),
            b'X' => {
                let end = (self.col + n).min(COLS);
                self.grid[self.row][self.col..end].fill(Cell::BLANK);
            }
            b'S' => self.scroll_up(self.top, self.bottom, n),
            b'T' => self.scroll_down(self.top, self.bottom, n),
            b'm' => self.select_graphics(),
            b'r' => {
                let top = n - 1;
                let bottom = self.param(1, self.rows() as u16) - 1;
                if top < bottom && bottom <= last_row {
                    self.top = top;
                    self.bottom = bottom;
                    self.move_to(0, 0);
                }
            }
            b's' => self.save_cursor(),
            b'u' => self.restore_cursor(),
            b'n' => match self.param(0, 0) {
                5 => self.replies.extend_from_slice(b"\x1b[0n"),
                6 => {
                    let report = format!("\x1b[{};{}R", self.row + 1, self.col + 1);
                    self.replies.extend_from_slice(report.as_bytes());
                }
                _ => {}
            },
            b'c' if self.param(0, 0) == 0 => {
                self.replies.extend_from_slice(b"\x1b[?1;2c");
            }
            _ => {}
        }
    }

    fn set_private_modes(&mut self, on: bool) {
        for index in 0..self.param_count.min(MAX_PARAMS) {
            match self.params[index] {
                25 => self.cursor_visible = on,
                2026 => self.sync = on,
                _ => {}
            }
        }
    }

    fn select_graphics(&mut self) {
        let count = self.param_count.clamp(1, MAX_PARAMS);
        let mut index = 0;
        while index < count {
            let code = self.params[index];
            index += 1;
            match code {
                0 => self.pen = Pen::DEFAULT,
                1 => self.pen.flags |= BOLD,
                4 => self.pen.flags |= UNDERLINE,
                5 | 6 => self.pen.flags |= BLINK,
                7 => self.pen.flags |= INVERSE,
                22 => self.pen.flags &= !BOLD,
                24 => self.pen.flags &= !UNDERLINE,
                25 => self.pen.flags &= !BLINK,
                27 => self.pen.flags &= !INVERSE,
                30..=37 => self.pen.fg = (code - 30) as u8,
                39 => self.pen.fg = DEFAULT_FG,
                40..=47 => self.pen.bg = (code - 40) as u8,
                49 => self.pen.bg = DEFAULT_BG,
                90..=97 => {
                    self.pen.fg = (code - 90) as u8;
                    self.pen.flags |= BOLD;
                }
                100..=107 => self.pen.bg = (code - 100) as u8,
                // Extended colours carry their own arguments; skip them whole so they are
                // not read as unrelated attributes.
                38 | 48 => {
                    index += match self.params.get(index) {
                        Some(5) => 2,
                        Some(2) => 4,
                        _ => 0,
                    };
                }
                _ => {}
            }
        }
    }

    fn osc_dispatch(&mut self) {
        let osc = std::mem::take(&mut self.osc);
        // OSC 8 ; params ; URI. An empty URI closes the link.
        if let Some(rest) = osc.strip_prefix(b"8;")
            && let Some(split) = rest.iter().position(|&b| b == b';')
        {
            let uri = &rest[split + 1..];
            self.link = match std::str::from_utf8(uri) {
                Ok(uri) if is_web_url(uri) => self.intern_link(uri),
                _ => 0,
            };
        }
        self.osc = osc;
        self.osc.clear();
    }

    fn intern_link(&mut self, uri: &str) -> u16 {
        if let Some(found) = self.links.iter().rposition(|known| known == uri) {
            return found as u16 + 1;
        }
        if self.links.len() >= MAX_LINKS {
            self.collect_links();
        }
        if self.links.len() >= MAX_LINKS {
            return 0;
        }
        self.links.push(uri.to_owned());
        self.links.len() as u16
    }

    /// Drops link targets no cell points at any more and renumbers the rest.
    fn collect_links(&mut self) {
        let mut remap = vec![0u16; self.links.len() + 1];
        let mut kept = Vec::new();
        for row in &mut self.grid {
            for cell in row.iter_mut().filter(|cell| cell.link != 0) {
                let old = usize::from(cell.link);
                if remap[old] == 0 {
                    kept.push(std::mem::take(&mut self.links[old - 1]));
                    remap[old] = kept.len() as u16;
                }
                cell.link = remap[old];
            }
        }
        self.links = kept;
        self.link = 0;
    }

    fn move_to(&mut self, row: usize, col: usize) {
        self.row = row.min(self.rows() - 1);
        self.col = col.min(COLS - 1);
        self.wrap_pending = false;
    }

    fn save_cursor(&mut self) {
        self.saved = Saved { row: self.row, col: self.col, pen: self.pen };
    }

    fn restore_cursor(&mut self) {
        let Saved { row, col, pen } = self.saved;
        self.move_to(row, col);
        self.pen = pen;
    }

    fn index(&mut self) {
        if self.row == self.bottom {
            self.scroll_up(self.top, self.bottom, 1);
        } else if self.row < self.rows() - 1 {
            self.row += 1;
        }
    }

    fn reverse_index(&mut self) {
        if self.row == self.top {
            self.scroll_down(self.top, self.bottom, 1);
        } else {
            self.row = self.row.saturating_sub(1);
        }
    }

    fn scroll_up(&mut self, top: usize, bottom: usize, count: usize) {
        let region = &mut self.grid[top..=bottom];
        let count = count.min(region.len());
        region.rotate_left(count);
        let len = region.len();
        region[len - count..].fill(BLANK_ROW);
    }

    fn scroll_down(&mut self, top: usize, bottom: usize, count: usize) {
        let region = &mut self.grid[top..=bottom];
        let count = count.min(region.len());
        region.rotate_right(count);
        region[..count].fill(BLANK_ROW);
    }

    fn insert_lines(&mut self, count: usize) {
        if (self.top..=self.bottom).contains(&self.row) {
            self.scroll_down(self.row, self.bottom, count);
            self.col = 0;
            self.wrap_pending = false;
        }
    }

    fn delete_lines(&mut self, count: usize) {
        if (self.top..=self.bottom).contains(&self.row) {
            self.scroll_up(self.row, self.bottom, count);
            self.col = 0;
            self.wrap_pending = false;
        }
    }

    fn insert_chars(&mut self, count: usize) {
        let tail = &mut self.grid[self.row][self.col..];
        let count = count.min(tail.len());
        tail.rotate_right(count);
        tail[..count].fill(Cell::BLANK);
    }

    fn delete_chars(&mut self, count: usize) {
        let tail = &mut self.grid[self.row][self.col..];
        let count = count.min(tail.len());
        tail.rotate_left(count);
        let len = tail.len();
        tail[len - count..].fill(Cell::BLANK);
    }

    // pfterm resets the pen before it erases because terminals disagree on which
    // background an erase paints. Blank cells with default colours match what it expects.
    fn erase_display(&mut self, mode: usize) {
        match mode {
            0 => {
                self.grid[self.row][self.col..].fill(Cell::BLANK);
                self.grid[self.row + 1..].fill(BLANK_ROW);
            }
            1 => {
                self.grid[..self.row].fill(BLANK_ROW);
                self.grid[self.row][..=self.col].fill(Cell::BLANK);
            }
            2 | 3 => {
                self.grid.fill(BLANK_ROW);
                self.links.clear();
                self.link = 0;
            }
            _ => {}
        }
    }

    fn erase_line(&mut self, mode: usize) {
        let row = &mut self.grid[self.row];
        match mode {
            0 => row[self.col..].fill(Cell::BLANK),
            1 => row[..=self.col].fill(Cell::BLANK),
            2 => row.fill(Cell::BLANK),
            _ => {}
        }
    }

    fn glyphs(&self, row: usize) -> [Glyph; COLS] {
        let cells = &self.grid[row];
        let mut out = [Glyph { ch: ' ', kind: KIND_SINGLE }; COLS];
        let mut col = 0;
        while col < COLS {
            let byte = cells[col].byte;
            if big5::is_lead(byte)
                && col + 1 < COLS
                && let Some(ch) = big5::decode(byte, cells[col + 1].byte)
            {
                out[col] = Glyph { ch, kind: KIND_LEAD };
                out[col + 1] = Glyph { ch, kind: KIND_TRAIL };
                col += 2;
                continue;
            }
            out[col].ch = match byte {
                0x20..=0x7E => byte as char,
                0x00..=0x1F | 0x7F => ' ',
                _ => '?',
            };
            col += 1;
        }
        out
    }

    /// Serialises the visible screen for the renderer.
    ///
    /// Layout, little endian: a [`SNAPSHOT_HEADER`]-byte header (version, rows, cursor row,
    /// cursor column, cursor visibility, 3 reserved, revision as u64), then one
    /// [`SNAPSHOT_CELL`]-byte record per cell, row by row: code point (u16), foreground
    /// 0-15, background 0-7, `CELL_*` flags, `KIND_*`, 2 reserved.
    pub fn snapshot(&self, out: &mut Vec<u8>) {
        out.clear();
        out.reserve(SNAPSHOT_HEADER + self.rows() * COLS * SNAPSHOT_CELL);
        out.extend_from_slice(&[
            SNAPSHOT_VERSION,
            self.rows() as u8,
            self.row as u8,
            self.col as u8,
            u8::from(self.cursor_visible),
            0,
            0,
            0,
        ]);
        out.extend_from_slice(&self.revision.to_le_bytes());
        for row in 0..self.rows() {
            let glyphs = self.glyphs(row);
            for (cell, glyph) in self.grid[row].iter().zip(glyphs) {
                let pen = cell.pen;
                let bright = if pen.flags & BOLD != 0 { 8 } else { 0 };
                let (fg, bg) =
                    if pen.flags & INVERSE != 0 { (pen.bg, pen.fg) } else { (pen.fg + bright, pen.bg) };
                let mut flags = 0;
                if pen.flags & BLINK != 0 {
                    flags |= CELL_BLINK;
                }
                if pen.flags & UNDERLINE != 0 {
                    flags |= CELL_UNDERLINE;
                }
                let code = u32::from(glyph.ch) as u16;
                out.extend_from_slice(&code.to_le_bytes());
                out.extend_from_slice(&[fg, bg, flags, glyph.kind, 0, 0]);
            }
        }
    }

    /// The text of one row with trailing blanks removed.
    pub fn row_text(&self, row: usize) -> String {
        if row >= self.rows() {
            return String::new();
        }
        let mut text: String =
            self.glyphs(row).iter().filter(|glyph| glyph.kind != KIND_TRAIL).map(|glyph| glyph.ch).collect();
        text.truncate(text.trim_end().len());
        text
    }

    /// Text between two cells in reading order, both inclusive. Rows are joined with
    /// newlines and lose their trailing blanks.
    pub fn text_between(&self, start: (usize, usize), end: (usize, usize)) -> String {
        let (start, end) = if start <= end { (start, end) } else { (end, start) };
        let last_row = end.0.min(self.rows() - 1);
        let mut text = String::new();
        for row in start.0..=last_row {
            let glyphs = self.glyphs(row);
            let mut from = if row == start.0 { start.1.min(COLS - 1) } else { 0 };
            let mut to = if row == end.0 { end.1.min(COLS - 1) } else { COLS - 1 };
            // Selecting half of a wide character takes the whole character.
            if glyphs[from].kind == KIND_TRAIL {
                from -= 1;
            }
            if glyphs[to].kind == KIND_LEAD {
                to += 1;
            }
            let line: String = glyphs[from..=to]
                .iter()
                .filter(|glyph| glyph.kind != KIND_TRAIL)
                .map(|glyph| glyph.ch)
                .collect();
            text.push_str(line.trim_end());
            if row != last_row {
                text.push('\n');
            }
        }
        text
    }

    /// The link under a cell: the server's OSC 8 target when it sent one, otherwise a
    /// URL found in the row text.
    pub fn link_at(&self, row: usize, col: usize) -> Option<String> {
        let cells = self.grid.get(row)?;
        let cell = cells.get(col)?;
        if cell.link != 0 {
            return self.links.get(usize::from(cell.link) - 1).cloned();
        }
        let bytes: Vec<u8> = cells.iter().map(|cell| cell.byte).collect();
        find_urls(&bytes)
            .into_iter()
            .find(|range| range.contains(&col))
            .map(|range| String::from_utf8_lossy(&bytes[range]).into_owned())
    }
}

fn is_web_url(uri: &str) -> bool {
    let lower = uri.get(..8).unwrap_or(uri).to_ascii_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://"))
        && uri.bytes().all(|b| (0x20..=0x7E).contains(&b))
}

fn is_url_byte(byte: u8) -> bool {
    byte.is_ascii_graphic() && !matches!(byte, b'"' | b'<' | b'>' | b'`' | b'{' | b'}' | b'|' | b'\\' | b'^')
}

/// Finds `http(s)://` runs in a row of raw bytes.
fn find_urls(row: &[u8]) -> Vec<std::ops::Range<usize>> {
    let mut found = Vec::new();
    let mut at = 0;
    while at < row.len() {
        let rest = &row[at..];
        let scheme = if rest.len() >= 8 && rest[..8].eq_ignore_ascii_case(b"https://") {
            8
        } else if rest.len() >= 7 && rest[..7].eq_ignore_ascii_case(b"http://") {
            7
        } else {
            at += 1;
            continue;
        };
        let mut end = at + scheme;
        while end < row.len() && is_url_byte(row[end]) {
            end += 1;
        }
        // Sentence punctuation right after a URL is almost never part of it.
        while end > at + scheme
            && matches!(row[end - 1], b'.' | b',' | b';' | b':' | b'!' | b'?' | b')' | b']' | b'\'')
        {
            end -= 1;
        }
        if end > at + scheme {
            found.push(at..end);
        }
        at = end.max(at + scheme);
    }
    found
}
