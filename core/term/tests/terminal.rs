use tui_term::screen::{LoginAssist, Screen, classify};
use tui_term::{
    CELL_BLINK, COLS, KIND_LEAD, KIND_SINGLE, KIND_TRAIL, SNAPSHOT_CELL, SNAPSHOT_HEADER, Terminal, big5,
};

fn big5(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    big5::encode_str(text, &mut out);
    out
}

fn term_with(text: &str) -> Terminal {
    let mut term = Terminal::new(24);
    term.feed(&big5(text));
    term
}

struct Cell {
    ch: char,
    fg: u8,
    bg: u8,
    flags: u8,
    kind: u8,
}

fn cell(term: &Terminal, row: usize, col: usize) -> Cell {
    let mut snapshot = Vec::new();
    term.snapshot(&mut snapshot);
    let at = SNAPSHOT_HEADER + (row * COLS + col) * SNAPSHOT_CELL;
    let code = u16::from_le_bytes([snapshot[at], snapshot[at + 1]]);
    Cell {
        ch: char::from_u32(code.into()).unwrap(),
        fg: snapshot[at + 2],
        bg: snapshot[at + 3],
        flags: snapshot[at + 4],
        kind: snapshot[at + 5],
    }
}

#[test]
fn prints_text_and_moves_with_cr_lf() {
    let term = term_with("hello\r\nworld");
    assert_eq!(term.row_text(0), "hello");
    assert_eq!(term.row_text(1), "world");
    assert_eq!(term.cursor(), (1, 5));
}

#[test]
fn decodes_big5_into_wide_cells() {
    let term = term_with("推 PTT 鄉民");
    assert_eq!(term.row_text(0), "推 PTT 鄉民");
    let lead = cell(&term, 0, 0);
    let trail = cell(&term, 0, 1);
    assert_eq!((lead.ch, lead.kind), ('推', KIND_LEAD));
    assert_eq!((trail.ch, trail.kind), ('推', KIND_TRAIL));
    assert_eq!(cell(&term, 0, 2).kind, KIND_SINGLE);
    assert_eq!(term.cursor(), (0, 11));
}

#[test]
fn keeps_separate_colours_for_each_half_of_a_character() {
    let [lead, trail] = big5::encode('推').unwrap();
    let mut term = Terminal::new(24);
    term.feed(b"\x1b[1;31m");
    term.feed(&[lead]);
    term.feed(b"\x1b[0;32;44m");
    term.feed(&[trail]);
    let left = cell(&term, 0, 0);
    let right = cell(&term, 0, 1);
    assert_eq!((left.ch, left.fg, left.bg), ('推', 9, 0));
    assert_eq!((right.ch, right.fg, right.bg), ('推', 2, 4));
}

#[test]
fn orphaned_half_of_a_character_becomes_a_placeholder() {
    let mut term = term_with("推 A");
    term.feed(b"\x1b[1;1Hx");
    assert_eq!(term.row_text(0), "x? A");
    assert_eq!(cell(&term, 0, 1).kind, KIND_SINGLE);
}

#[test]
fn addresses_and_erases_like_pfterm_expects() {
    let mut term = Terminal::new(24);
    term.feed(b"\x1b[H\x1b[2J");
    term.feed(b"\x1b[3;5Habcdef\x1b[3;8H\x1b[K");
    assert_eq!(term.row_text(2), "    abc");
    term.feed(b"\x1b[5;1Hkeep\x1b[6;1Hgone\x1b[6;1H\x1b[J");
    assert_eq!(term.row_text(4), "keep");
    assert_eq!(term.row_text(5), "");
    term.feed(b"\x1b[2J");
    assert!((0..24).all(|row| term.row_text(row).is_empty()));
    assert_eq!(term.cursor(), (5, 0));
}

#[test]
fn erasing_drops_colours() {
    let mut term = Terminal::new(24);
    term.feed(b"\x1b[1;37;44mtitle\x1b[1;1H\x1b[K");
    let erased = cell(&term, 0, 0);
    assert_eq!((erased.ch, erased.fg, erased.bg), (' ', 7, 0));
}

#[test]
fn wraps_only_when_the_next_character_arrives() {
    let mut term = Terminal::new(24);
    term.feed(&[b'x'; COLS]);
    assert_eq!(term.cursor(), (0, COLS - 1));
    term.feed(b"y");
    assert_eq!(term.cursor(), (1, 1));
    assert_eq!(term.row_text(1), "y");
}

#[test]
fn cursor_move_cancels_a_pending_wrap() {
    let mut term = Terminal::new(24);
    term.feed(&[b'x'; COLS]);
    term.feed(b"\x1b[1;1Hy");
    assert_eq!(term.cursor(), (0, 1));
    assert_eq!(term.row_text(1), "");
}

#[test]
fn scrolls_with_line_feed_at_the_bottom_and_reverse_index_at_the_top() {
    let mut term = Terminal::new(24);
    term.feed(b"first\x1b[24;1Hlast\n");
    assert_eq!(term.row_text(0), "");
    assert_eq!(term.row_text(22), "last");
    assert_eq!(term.cursor(), (23, 4));
    term.feed(b"\x1b[1;1H\x1bM");
    assert_eq!(term.row_text(23), "last");
    assert_eq!(term.cursor(), (0, 0));
}

#[test]
fn scroll_region_limits_scrolling() {
    let mut term = Terminal::new(24);
    term.feed(b"head\x1b[24;1Hfoot\x1b[2;23r\x1b[23;1Hbody\n");
    assert_eq!(term.row_text(0), "head");
    assert_eq!(term.row_text(21), "body");
    assert_eq!(term.row_text(23), "foot");
}

#[test]
fn inserts_and_deletes_lines_and_characters() {
    let mut term = term_with("one\r\ntwo\r\nthree");
    term.feed(b"\x1b[2;1H\x1b[L");
    assert_eq!(term.row_text(1), "");
    assert_eq!(term.row_text(2), "two");
    term.feed(b"\x1b[M");
    assert_eq!(term.row_text(1), "two");
    term.feed(b"\x1b[2;1H\x1b[2@");
    assert_eq!(term.row_text(1), "  two");
    term.feed(b"\x1b[3P");
    assert_eq!(term.row_text(1), "wo");
}

#[test]
fn applies_and_resets_graphic_attributes() {
    let mut term = Terminal::new(24);
    term.feed(b"\x1b[1;5;33;45mA\x1b[mB\x1b[7mC\x1b[0;38;5;200;32mD");
    let a = cell(&term, 0, 0);
    assert_eq!((a.fg, a.bg, a.flags & CELL_BLINK), (11, 5, CELL_BLINK));
    let b = cell(&term, 0, 1);
    assert_eq!((b.fg, b.bg, b.flags), (7, 0, 0));
    let c = cell(&term, 0, 2);
    assert_eq!((c.fg, c.bg), (0, 7));
    // 38;5;200 is skipped whole; the trailing 32 still applies.
    assert_eq!(cell(&term, 0, 3).fg, 2);
}

#[test]
fn sequences_survive_being_split_across_reads() {
    let [lead, trail] = big5::encode('鄉').unwrap();
    let mut stream = b"\x1b[2;3H\x1b[1;36m".to_vec();
    stream.extend_from_slice(&[lead, trail]);
    stream.extend_from_slice(b"\x1b]8;;https://www.ptt.cc/\x1b\\x\x1b]8;;\x1b\\");

    let mut whole = Terminal::new(24);
    whole.feed(&stream);
    let mut expected = Vec::new();
    whole.snapshot(&mut expected);

    for split in 1..stream.len() {
        let mut term = Terminal::new(24);
        term.feed(&stream[..split]);
        term.feed(&stream[split..]);
        let mut actual = Vec::new();
        term.snapshot(&mut actual);
        // The revision counts reads, so compare everything after the header.
        assert_eq!(actual[SNAPSHOT_HEADER..], expected[SNAPSHOT_HEADER..], "split at {split}");
        assert_eq!(term.link_at(1, 4).as_deref(), Some("https://www.ptt.cc/"));
    }
}

#[test]
fn follows_server_hyperlinks() {
    let mut term = Terminal::new(24);
    term.feed(b"see \x1b]8;;https://www.ptt.cc/bbs/a.html\x1b\\here\x1b]8;;\x1b\\ ok");
    assert_eq!(term.link_at(0, 3), None);
    assert_eq!(term.link_at(0, 4).as_deref(), Some("https://www.ptt.cc/bbs/a.html"));
    assert_eq!(term.link_at(0, 7).as_deref(), Some("https://www.ptt.cc/bbs/a.html"));
    assert_eq!(term.link_at(0, 8), None);
}

#[test]
fn refuses_hyperlinks_that_are_not_web_addresses() {
    let mut term = Terminal::new(24);
    term.feed(b"\x1b]8;;javascript:alert(1)\x07tap\x1b]8;;\x07");
    assert_eq!(term.link_at(0, 0), None);
    term.feed(b"\x1b]8;;intent://evil\x1b\\tap");
    assert_eq!(term.link_at(0, 4), None);
}

#[test]
fn finds_plain_urls_in_text() {
    let term = term_with("※ 文章網址: https://www.ptt.cc/bbs/Gossiping/M.1.A.html.");
    let start = "※ 文章網址: ".chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum::<usize>();
    let url = "https://www.ptt.cc/bbs/Gossiping/M.1.A.html";
    assert_eq!(term.link_at(0, start).as_deref(), Some(url));
    assert_eq!(term.link_at(0, start + url.len() - 1).as_deref(), Some(url));
    assert_eq!(term.link_at(0, start + url.len()), None);
    assert_eq!(term.link_at(0, 0), None);
}

#[test]
fn link_table_survives_heavy_reuse() {
    let mut term = Terminal::new(24);
    for n in 0..5000 {
        term.feed(format!("\x1b[1;1H\x1b]8;;https://example.com/{n}\x1b\\x\x1b]8;;\x1b\\").as_bytes());
    }
    assert_eq!(term.link_at(0, 0).as_deref(), Some("https://example.com/4999"));
}

#[test]
fn answers_status_requests() {
    let mut term = Terminal::new(24);
    term.feed(b"\x1b[5;9H\x1b[6n\x1b[5n");
    assert_eq!(term.take_replies(), b"\x1b[5;9R\x1b[0n");
    assert!(term.take_replies().is_empty());
}

#[test]
fn tracks_bells_and_synchronised_frames() {
    let mut term = Terminal::new(24);
    term.feed(b"\x07\x1b[?2026hpartial");
    assert!(term.in_sync_frame());
    assert_eq!(term.take_bells(), 1);
    term.feed(b"\x1b[?2026l");
    assert!(!term.in_sync_frame());
    assert_eq!(term.take_bells(), 0);
}

#[test]
fn hides_and_shows_the_cursor() {
    let mut term = Terminal::new(24);
    let visible = |term: &Terminal| {
        let mut snapshot = Vec::new();
        term.snapshot(&mut snapshot);
        snapshot[4] == 1
    };
    assert!(visible(&term));
    term.feed(b"\x1b[?25l");
    assert!(!visible(&term));
    term.feed(b"\x1b[?25h");
    assert!(visible(&term));
}

#[test]
fn resizing_keeps_the_top_and_clamps_the_cursor() {
    let mut term = Terminal::new(40);
    term.feed(b"top\x1b[40;1Hbottom");
    term.resize(24);
    assert_eq!(term.rows(), 24);
    assert_eq!(term.row_text(0), "top");
    assert_eq!(term.cursor().0, 23);
    term.resize(1000);
    assert_eq!(term.rows(), 100);
    term.feed(b"\x1b[100;1Hend");
    assert_eq!(term.row_text(99), "end");
}

#[test]
fn copies_text_across_rows() {
    let term = term_with("推文 one   \r\n噓文 two\r\n→ three");
    assert_eq!(term.text_between((0, 0), (1, 3)), "推文 one\n噓文");
    // Starting or ending on half a character takes the whole character.
    assert_eq!(term.text_between((0, 1), (0, 2)), "推文");
    assert_eq!(term.text_between((1, 3), (0, 0)), "推文 one\n噓文");
}

#[test]
fn out_of_range_requests_are_harmless() {
    let mut term = Terminal::new(24);
    term.feed(b"\x1b[999;999Hx\x1b[0;0r\x1b[99999999999999999999A\x1b[;;;;;;;;;;;;;;;;;;;;;;;;m");
    term.feed(b"\x1b[500L\x1b[500M\x1b[500@\x1b[500P\x1b[500X\x1b[500S\x1b[500T\x1b[500B\x1b[500C");
    assert_eq!(term.row_text(500), "");
    assert_eq!(term.link_at(500, 500), None);
    assert!(term.text_between((0, 0), (500, 500)).trim().is_empty());
}

#[test]
fn arbitrary_bytes_never_panic() {
    // Deterministic noise, weighted towards the bytes that drive the parser.
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let spice = b"\x1b[]?;0123456789HJKLMPSTXmrhl\\\x07\r\n\x08\x09@ABCDsu78c";
    let mut term = Terminal::new(24);
    let mut snapshot = Vec::new();
    for round in 0..2000 {
        let chunk: Vec<u8> = (0..(next() % 96) as usize)
            .map(|_| {
                let value = next();
                if value % 3 == 0 { spice[(value >> 8) as usize % spice.len()] } else { (value >> 16) as u8 }
            })
            .collect();
        term.feed(&chunk);
        if round % 97 == 0 {
            term.resize(24 + (next() % 80) as usize);
        }
        term.snapshot(&mut snapshot);
        assert_eq!(snapshot.len(), SNAPSHOT_HEADER + term.rows() * COLS * SNAPSHOT_CELL);
        let (row, col) = term.cursor();
        assert!(row < term.rows() && col < COLS);
        let _ = term.link_at(row, col);
        let _ = classify(&term);
        term.take_replies();
    }
}

#[test]
fn recognises_screens_by_their_text() {
    assert_eq!(classify(&term_with("\x1b[21;1H請輸入代號，或以 guest 參觀，或以 new 註冊: ")), Screen::Login);
    assert_eq!(classify(&term_with("\x1b[22;1H請輸入您的密碼: ")), Screen::Password);
    assert_eq!(classify(&term_with("\x1b[24;1H請按任意鍵繼續")), Screen::AnyKey);
    assert_eq!(classify(&term_with("【主功能表】           批踢踢實業坊")), Screen::MainMenu);
    assert_eq!(classify(&term_with("主選單   世界郵政日   10/9 週五 15:04")), Screen::MainMenu);
    assert_eq!(classify(&term_with("【看板列表】")), Screen::BoardList);
    assert_eq!(classify(&term_with("\x1b[24;1H 文章選讀  (y)回應(X)推文(^X)轉錄")), Screen::ArticleList);
    assert_eq!(classify(&term_with("\x1b[24;1H 文章列表  (^P)發表")), Screen::ArticleList);
    assert_eq!(
        classify(&term_with(
            "\x1b[24;1H  瀏覽 第 1/3 頁 ( 33%)  目前顯示: 第 01~22 行  (y)回應(X%)推文(h)說明(←)離開"
        )),
        Screen::Article
    );
    assert_eq!(classify(&term_with("something else")), Screen::Unknown);
}

#[test]
fn login_assist_answers_each_prompt_once() {
    let mut assist = LoginAssist::new("someone".into(), "secret".into());
    let mut term = term_with("歡迎\x1b[21;1H請輸入代號，或以 guest 參觀，或以 new 註冊: ");
    assert_eq!(assist.step(&term).as_deref(), Some(&b"someone\r"[..]));
    assert_eq!(assist.step(&term), None);

    term.feed(&big5("\x1b[22;1H請輸入您的密碼: "));
    assert_eq!(assist.step(&term).as_deref(), Some(&b"secret\r"[..]));
    assert!(assist.finished());

    // A rejected password shows the prompts again; the assist must stay out of it.
    term.feed(&big5("\x1b[2J\x1b[21;1H請輸入代號，或以 guest 參觀，或以 new 註冊: "));
    assert_eq!(assist.step(&term), None);
    term.feed(&big5("\x1b[22;1H請輸入您的密碼: "));
    assert_eq!(assist.step(&term), None);
}

#[test]
fn login_assist_waits_for_a_prompt() {
    let mut assist = LoginAssist::new("someone".into(), "secret".into());
    let term = term_with("\x1b[24;1H請按任意鍵繼續");
    assert_eq!(assist.step(&term), None);
    assert!(!assist.finished());
}
