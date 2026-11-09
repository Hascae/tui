//! Recognising a handful of PTT screens from their text.
//!
//! PTT rewords its screens from time to time, so every caller must treat
//! [`Screen::Unknown`] as a normal answer and keep working without it.

use crate::Terminal;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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

pub fn classify(term: &Terminal) -> Screen {
    let prompt = term.row_text(term.cursor().0);
    if prompt.contains("請輸入您的密碼") {
        return Screen::Password;
    }
    if prompt.contains("請輸入代號") {
        return Screen::Login;
    }

    let top = term.row_text(0);
    let footer = term.row_text(term.rows() - 1);
    if footer.contains("瀏覽")
        && footer.contains('頁')
        && (footer.contains(")離開") || footer.contains("目前顯示"))
    {
        return Screen::Article;
    }
    if footer.contains("文章選讀") || footer.contains("文章列表") {
        return Screen::ArticleList;
    }
    if footer.contains("任意鍵") {
        return Screen::AnyKey;
    }
    if top.contains("【主功能表】") || top.starts_with("主選單") {
        return Screen::MainMenu;
    }
    if top.contains("【看板列表】") || top.contains("【分類看板】") {
        return Screen::BoardList;
    }
    Screen::Unknown
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Account,
    Password,
    Done,
}

/// Types a saved account and password into the login prompts, once each.
///
/// It only ever answers a prompt it can see, never retries, and stops for good after the
/// password. Anything else PTT asks during login is left to the person at the keyboard.
pub struct LoginAssist {
    account: Vec<u8>,
    password: Vec<u8>,
    stage: Stage,
}

impl LoginAssist {
    pub fn new(account: String, password: String) -> Self {
        LoginAssist { account: account.into_bytes(), password: password.into_bytes(), stage: Stage::Account }
    }

    pub fn finished(&self) -> bool {
        self.stage == Stage::Done
    }

    /// Returns the bytes to send for the prompt on screen, if it is our turn.
    pub fn step(&mut self, term: &Terminal) -> Option<Vec<u8>> {
        let screen = classify(term);
        let field = match (self.stage, screen) {
            (Stage::Account, Screen::Login) => {
                self.stage = Stage::Password;
                &self.account
            }
            (Stage::Account | Stage::Password, Screen::Password) => {
                self.stage = Stage::Done;
                &self.password
            }
            _ => return None,
        };
        let mut keys = field.clone();
        keys.push(b'\r');
        if self.stage == Stage::Done {
            self.forget();
        }
        Some(keys)
    }

    fn forget(&mut self) {
        self.account.fill(0);
        self.password.fill(0);
        self.account.clear();
        self.password.clear();
    }
}

impl Drop for LoginAssist {
    fn drop(&mut self) {
        self.forget();
    }
}
