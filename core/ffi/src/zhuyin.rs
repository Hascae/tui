//! Zhuyin input for the on-screen keyboard, backed by the Chewing engine.

use std::sync::Arc;
use std::sync::mpsc::{Sender, SyncSender, channel, sync_channel};

use chewing::editor::{BasicEditor, Editor, EditorKeyBehavior};
use chewing::input::keymap::{QWERTY_MAP, map_ascii};
use chewing::input::{KeyboardEvent, keycode, keysym};

const DICTIONARIES: [&str; 2] = ["word.dat", "tsi.dat"];

/// What the keyboard shows after a key press.
#[derive(uniffi::Record, Debug, Default, PartialEq, Eq)]
pub struct Composition {
    /// False when the engine had nothing to do with the key and it belongs to PTT.
    pub handled: bool,
    /// Text ready to send.
    pub commit: String,
    /// Converted text still being edited.
    pub preedit: String,
    /// Bopomofo of the syllable in progress.
    pub syllable: String,
    /// Alternatives for the word at the end of `preedit`.
    pub candidates: Vec<String>,
}

enum Action {
    Key(u8),
    Space,
    Backspace,
    Commit,
    Choose(u32),
    Clear,
    Peek,
}

struct Request {
    action: Action,
    reply: SyncSender<(Composition, bool)>,
}

/// The Chewing editor is not `Send`, so it lives on a thread of its own and is driven
/// through a channel.
#[derive(uniffi::Object)]
pub struct Zhuyin {
    requests: Sender<Request>,
}

#[uniffi::export]
impl Zhuyin {
    /// `data_dir` holds the Chewing dictionaries; learned phrases go to `user_file`.
    #[uniffi::constructor]
    pub fn new(data_dir: String, user_file: String) -> Arc<Self> {
        let (requests, inbox) = channel::<Request>();
        let worker = std::thread::Builder::new().name("tui-zhuyin".into());
        // If the thread cannot start, every call below answers with an idle composition
        // and the keyboard degrades to plain keys.
        let _ = worker.spawn(move || {
            let mut editor = Editor::chewing(Some(data_dir), Some(user_file), &DICTIONARIES);
            editor.set_editor_options(|options| {
                // The cursor sits at the end while typing, so candidates must look backwards.
                options.phrase_choice_rearward = true;
                options.auto_shift_cursor = true;
                options.esc_clear_all_buffer = true;
            });
            for request in inbox {
                let composition = perform(&mut editor, request.action);
                let composing = !editor.is_empty() || editor.entering_syllable();
                let _ = request.reply.send((composition, composing));
            }
        });
        Arc::new(Zhuyin { requests })
    }

    /// A key of the standard zhuyin layout, named by the ASCII character printed on the
    /// same key of a US keyboard.
    pub fn key(&self, ascii: u8) -> Composition {
        self.ask(Action::Key(ascii)).0
    }

    /// Space finishes a first-tone syllable; with only finished text it commits.
    pub fn space(&self) -> Composition {
        self.ask(Action::Space).0
    }

    pub fn backspace(&self) -> Composition {
        self.ask(Action::Backspace).0
    }

    /// Commits everything typed so far.
    pub fn commit(&self) -> Composition {
        self.ask(Action::Commit).0
    }

    /// Replaces the word at the end of the preedit with one of its candidates.
    pub fn choose(&self, index: u32) -> Composition {
        self.ask(Action::Choose(index)).0
    }

    /// Drops everything in progress.
    pub fn clear(&self) {
        self.ask(Action::Clear);
    }

    pub fn is_composing(&self) -> bool {
        self.ask(Action::Peek).1
    }
}

impl Zhuyin {
    fn ask(&self, action: Action) -> (Composition, bool) {
        let (reply, answer) = sync_channel(1);
        if self.requests.send(Request { action, reply }).is_err() {
            return Default::default();
        }
        answer.recv().unwrap_or_default()
    }
}

fn perform(editor: &mut Editor, action: Action) -> Composition {
    let idle = editor.is_empty() && !editor.entering_syllable();
    match action {
        Action::Key(ascii) => {
            let behavior = editor.process_keyevent(map_ascii(&QWERTY_MAP, ascii));
            composition(editor, behavior)
        }
        Action::Space if editor.entering_syllable() => {
            let behavior = editor.process_keyevent(map_ascii(&QWERTY_MAP, b' '));
            composition(editor, behavior)
        }
        Action::Space | Action::Commit | Action::Backspace if idle => Composition::default(),
        Action::Space | Action::Commit => {
            editor.cancel_entering_syllable();
            let behavior =
                if editor.commit().is_ok() { EditorKeyBehavior::Commit } else { EditorKeyBehavior::Absorb };
            composition(editor, behavior)
        }
        Action::Backspace => {
            let event =
                KeyboardEvent::builder().code(keycode::KEY_BACKSPACE).ksym(keysym::SYM_BACKSPACE).build();
            let behavior = editor.process_keyevent(event);
            composition(editor, behavior)
        }
        Action::Choose(index) => {
            if editor.start_selecting().is_ok() && editor.select(index as usize).is_err() {
                let _ = editor.cancel_selecting();
            }
            composition(editor, EditorKeyBehavior::Absorb)
        }
        Action::Clear => {
            editor.clear();
            Composition::default()
        }
        Action::Peek => Composition::default(),
    }
}

fn composition(editor: &mut Editor, behavior: EditorKeyBehavior) -> Composition {
    let commit = if behavior == EditorKeyBehavior::Commit {
        let text = editor.display_commit().to_owned();
        editor.ack();
        text
    } else {
        String::new()
    };
    let candidates = if !editor.is_empty() && !editor.entering_syllable() && editor.start_selecting().is_ok()
    {
        let candidates = editor.all_candidates().unwrap_or_default();
        let _ = editor.cancel_selecting();
        candidates
    } else {
        Vec::new()
    };
    Composition {
        handled: behavior != EditorKeyBehavior::Ignore,
        commit,
        preedit: editor.display(),
        syllable: editor.syllable_buffer_display(),
        candidates,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zhuyin() -> Arc<Zhuyin> {
        // The dictionaries are fetched by tools/fetch-zhuyin-data.sh, not kept in the repository.
        let data =
            std::env::var("TUI_ZHUYIN_DATA").expect("TUI_ZHUYIN_DATA must point at the Chewing dictionaries");
        assert!(std::path::Path::new(&data).join("tsi.dat").is_file(), "no tsi.dat in {data}");
        let name = format!("tui-zhuyin-{}-{:?}.dat", std::process::id(), std::thread::current().id());
        let user = std::env::temp_dir().join(name);
        let _ = std::fs::remove_file(&user);
        Zhuyin::new(data, user.to_string_lossy().into_owned())
    }

    fn type_keys(zhuyin: &Zhuyin, keys: &str) -> Composition {
        keys.bytes()
            .fold(Composition::default(), |_, key| if key == b' ' { zhuyin.space() } else { zhuyin.key(key) })
    }

    #[test]
    fn composes_and_commits_a_phrase() {
        let zhuyin = zhuyin();
        // ㄒㄧㄤ ㄇㄧㄣˊ
        let typing = type_keys(&zhuyin, "vu; ");
        assert_eq!(typing.syllable, "");
        assert!(typing.commit.is_empty());
        let phrase = type_keys(&zhuyin, "aup6");
        assert_eq!(phrase.preedit, "鄉民");
        assert!(phrase.handled && zhuyin.is_composing());

        let done = zhuyin.commit();
        assert_eq!(done.commit, "鄉民");
        assert!(done.preedit.is_empty() && done.candidates.is_empty());
        assert!(!zhuyin.is_composing());
    }

    #[test]
    fn shows_the_syllable_in_progress() {
        let zhuyin = zhuyin();
        let partial = type_keys(&zhuyin, "vu");
        assert_eq!(partial.syllable, "ㄒㄧ");
        assert!(partial.preedit.is_empty() && partial.candidates.is_empty());
        assert!(zhuyin.is_composing());
    }

    #[test]
    fn offers_and_applies_candidates() {
        let zhuyin = zhuyin();
        // ㄊㄨㄟ
        let typed = type_keys(&zhuyin, "wjo ");
        assert_eq!(typed.preedit.chars().count(), 1);
        let pick =
            typed.candidates.iter().position(|candidate| candidate == "推").expect("推 is a candidate");
        let chosen = zhuyin.choose(pick as u32);
        assert_eq!(chosen.preedit, "推");
        // Listing candidates must leave the editor ready for more typing.
        let more = type_keys(&zhuyin, "jp4");
        assert_eq!(more.preedit.chars().count(), 2);
        assert!(more.preedit.starts_with('推'));
    }

    #[test]
    fn out_of_range_choice_changes_nothing() {
        let zhuyin = zhuyin();
        let typed = type_keys(&zhuyin, "wjo ");
        let after = zhuyin.choose(10_000);
        assert_eq!(after.preedit, typed.preedit);
        assert_eq!(zhuyin.commit().commit, typed.preedit);
    }

    #[test]
    fn backspace_edits_then_falls_through_when_empty() {
        let zhuyin = zhuyin();
        type_keys(&zhuyin, "vu");
        assert_eq!(zhuyin.backspace().syllable, "ㄒ");
        assert!(zhuyin.backspace().handled);
        assert!(!zhuyin.is_composing());
        assert!(!zhuyin.backspace().handled);
    }

    #[test]
    fn space_and_commit_fall_through_when_idle() {
        let zhuyin = zhuyin();
        assert!(!zhuyin.space().handled);
        assert!(!zhuyin.commit().handled);
    }

    #[test]
    fn clear_drops_work_in_progress() {
        let zhuyin = zhuyin();
        type_keys(&zhuyin, "vu; aup");
        zhuyin.clear();
        assert!(!zhuyin.is_composing());
        assert!(!zhuyin.commit().handled);
    }
}
