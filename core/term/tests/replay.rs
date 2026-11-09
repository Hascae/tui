//! Replays recorded server streams and compares the resulting screen with a transcript.
//!
//! A fixture is a `name.bin` stream plus a `name.txt` screen. To add one, record a
//! session with the `peek` example in `tui-net`, replay it here with `TUI_BLESS=1` set,
//! and read the generated transcript before committing it.

use std::fs;
use std::path::Path;

use tui_term::Terminal;

fn screen(term: &Terminal) -> String {
    let mut text = String::new();
    for row in 0..term.rows() {
        text.push_str(&term.row_text(row));
        text.push('\n');
    }
    let (row, col) = term.cursor();
    text.push_str(&format!("cursor {row},{col}\n"));
    text
}

fn replay(stream: &[u8], chunk: usize) -> String {
    let mut term = Terminal::new(24);
    for piece in stream.chunks(chunk) {
        term.feed(piece);
    }
    assert!(!term.in_sync_frame(), "stream ends inside a synchronised frame");
    screen(&term)
}

#[test]
fn recorded_streams_paint_the_expected_screens() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut streams: Vec<_> = fs::read_dir(&fixtures)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "bin"))
        .collect();
    streams.sort();
    assert!(!streams.is_empty(), "no fixtures in {}", fixtures.display());

    for path in streams {
        let stream = fs::read(&path).unwrap();
        let transcript = path.with_extension("txt");
        let whole = replay(&stream, stream.len());
        if std::env::var_os("TUI_BLESS").is_some() {
            fs::write(&transcript, &whole).unwrap();
        }
        let expected = fs::read_to_string(&transcript)
            .unwrap_or_else(|_| panic!("{} has no transcript", path.display()));
        assert_eq!(whole, expected, "{}", path.display());
        // Network reads cut the stream anywhere; the result must not depend on where.
        for chunk in [1, 2, 3, 7, 64, 1000] {
            assert_eq!(replay(&stream, chunk), expected, "{} in {chunk}-byte reads", path.display());
        }
    }
}
