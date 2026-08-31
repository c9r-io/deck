use crate::model::{self, Board, Card, COLUMNS};
use crate::tmux;
use anyhow::Result;
use std::collections::HashSet;
use std::path::PathBuf;

pub enum Mode {
    Normal,
    Input { kind: InputKind, buffer: String },
    Confirm(ConfirmKind),
}

#[derive(Clone)]
pub enum InputKind {
    NewTitle,
    NewCommand { title: String },
    NewDir { title: String, command: String },
    EditTitle { card_id: String },
    EditCommand { card_id: String },
}

#[derive(Clone)]
pub enum ConfirmKind {
    DeleteCard { card_id: String },
    KillSession { card_id: String },
}

/// Actions that require suspending the TUI; executed by the main loop.
pub enum Action {
    Attach { session: String },
    EditNotes { card_id: String },
}

pub struct App {
    pub board: Board,
    pub sel_col: usize,
    pub sel_row: usize,
    pub mode: Mode,
    pub status: String,
    pub live_sessions: HashSet<String>,
    pub tail: Vec<String>,
    pub quit: bool,
    pending: Option<Action>,
    data_dir: PathBuf,
}

impl App {
    pub fn load() -> Result<Self> {
        let data_dir = model::data_dir();
        let board = Board::load()?;
        let mut app = App {
            board,
            sel_col: 0,
            sel_row: 0,
            mode: Mode::Normal,
            status: String::from("n:new card  Enter:attach  ?:keys"),
            live_sessions: HashSet::new(),
            tail: Vec::new(),
            quit: false,
            pending: None,
            data_dir,
        };
        app.refresh();
        Ok(app)
    }

    #[cfg(test)]
    pub(crate) fn test_with_board(board: Board) -> Self {
        App {
            board,
            sel_col: 0,
            sel_row: 0,
            mode: Mode::Normal,
            status: "ready".into(),
            live_sessions: HashSet::new(),
            tail: Vec::new(),
            quit: false,
            pending: None,
            data_dir: std::env::temp_dir(),
        }
    }

    pub fn take_action(&mut self) -> Option<Action> {
        self.pending.take()
    }

    /// Poll tmux for live sessions and the selected card's pane tail.
    pub fn refresh(&mut self) {
        self.live_sessions = tmux::sessions();
        self.tail = match self.selected_card() {
            Some(c) if self.live_sessions.contains(&c.session) => tmux::capture_tail(&c.session, 8),
            _ => Vec::new(),
        };
    }

    pub fn selected_card(&self) -> Option<&Card> {
        let idx = self.board.column_indices(self.sel_col);
        idx.get(self.sel_row).map(|&i| &self.board.cards[i])
    }

    fn selected_global_index(&self) -> Option<usize> {
        self.board
            .column_indices(self.sel_col)
            .get(self.sel_row)
            .copied()
    }

    fn clamp_row(&mut self) {
        let len = self.board.column_indices(self.sel_col).len();
        if len == 0 {
            self.sel_row = 0;
        } else if self.sel_row >= len {
            self.sel_row = len - 1;
        }
    }

    fn save(&mut self) {
        if let Err(e) = self.board.save_to(&self.data_dir) {
            self.status = format!("save failed: {e}");
        }
    }

    fn card_mut_by_id(&mut self, id: &str) -> Option<&mut Card> {
        self.board.cards.iter_mut().find(|c| c.id == id)
    }

    // ---- key handling ----------------------------------------------------

    pub fn on_key(&mut self, code: KeyCode) {
        match &self.mode {
            Mode::Normal => self.on_key_normal(code),
            Mode::Input { .. } => self.on_key_input(code),
            Mode::Confirm(_) => self.on_key_confirm(code),
        }
    }

    fn on_key_normal(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Left | KeyCode::Char('h') => {
                self.sel_col = self.sel_col.saturating_sub(1);
                self.clamp_row();
                self.refresh();
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.sel_col = (self.sel_col + 1).min(COLUMNS.len() - 1);
                self.clamp_row();
                self.refresh();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.sel_row = self.sel_row.saturating_sub(1);
                self.refresh();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let len = self.board.column_indices(self.sel_col).len();
                if len > 0 {
                    self.sel_row = (self.sel_row + 1).min(len - 1);
                }
                self.refresh();
            }
            KeyCode::Char('H') | KeyCode::Char('[') => self.move_card(-1),
            KeyCode::Char('L') | KeyCode::Char(']') => self.move_card(1),
            KeyCode::Char('K') => self.reorder_card(-1),
            KeyCode::Char('J') => self.reorder_card(1),
            KeyCode::Char('n') => {
                self.mode = Mode::Input {
                    kind: InputKind::NewTitle,
                    buffer: String::new(),
                };
            }
            KeyCode::Char('e') => {
                if let Some(c) = self.selected_card() {
                    self.mode = Mode::Input {
                        kind: InputKind::EditTitle {
                            card_id: c.id.clone(),
                        },
                        buffer: c.title.clone(),
                    };
                }
            }
            KeyCode::Char('c') => {
                if let Some(c) = self.selected_card() {
                    self.mode = Mode::Input {
                        kind: InputKind::EditCommand {
                            card_id: c.id.clone(),
                        },
                        buffer: c.command.clone(),
                    };
                }
            }
            KeyCode::Enter => {
                if let Some(c) = self.selected_card() {
                    let (id, session, dir, command, title) = (
                        c.id.clone(),
                        c.session.clone(),
                        c.dir.clone(),
                        c.command.clone(),
                        c.title.clone(),
                    );
                    if !self.live_sessions.contains(&session) {
                        match tmux::new_session(&session, &dir, &command) {
                            Ok(()) => {
                                // A fresh session on an Active-bound card moves it along.
                                if let Some(card) = self.card_mut_by_id(&id) {
                                    if card.column == 0 {
                                        card.column = 1;
                                    }
                                }
                                self.save();
                            }
                            Err(e) => {
                                self.status = format!("start failed: {e}");
                                return;
                            }
                        }
                    }
                    self.status = format!("attached: {title}");
                    self.pending = Some(Action::Attach { session });
                }
            }
            KeyCode::Char('s') => {
                if let Some(c) = self.selected_card() {
                    let (id, session, dir, command) = (
                        c.id.clone(),
                        c.session.clone(),
                        c.dir.clone(),
                        c.command.clone(),
                    );
                    if self.live_sessions.contains(&session) {
                        self.status = "session already running".into();
                    } else {
                        match tmux::new_session(&session, &dir, &command) {
                            Ok(()) => {
                                if let Some(card) = self.card_mut_by_id(&id) {
                                    if card.column == 0 {
                                        card.column = 1;
                                    }
                                }
                                self.save();
                                self.status = format!("started {session}");
                                self.refresh();
                            }
                            Err(e) => self.status = format!("start failed: {e}"),
                        }
                    }
                }
            }
            KeyCode::Char('x') => {
                if let Some(c) = self.selected_card() {
                    if self.live_sessions.contains(&c.session) {
                        self.mode = Mode::Confirm(ConfirmKind::KillSession {
                            card_id: c.id.clone(),
                        });
                    } else {
                        self.status = "no live session".into();
                    }
                }
            }
            KeyCode::Char('d') => {
                if let Some(c) = self.selected_card() {
                    self.mode = Mode::Confirm(ConfirmKind::DeleteCard {
                        card_id: c.id.clone(),
                    });
                }
            }
            KeyCode::Char('o') => {
                if let Some(c) = self.selected_card() {
                    self.pending = Some(Action::EditNotes {
                        card_id: c.id.clone(),
                    });
                }
            }
            KeyCode::Char('r') => {
                self.refresh();
                self.status = "refreshed".into();
            }
            _ => {}
        }
    }

    fn on_key_input(&mut self, code: KeyCode) {
        let Mode::Input { kind, buffer } = &mut self.mode else {
            return;
        };
        match code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.status = "cancelled".into();
            }
            KeyCode::Backspace => {
                buffer.pop();
            }
            KeyCode::Char(ch) => buffer.push(ch),
            KeyCode::Enter => {
                let text = buffer.trim().to_string();
                let kind = kind.clone();
                match kind {
                    InputKind::NewTitle => {
                        if text.is_empty() {
                            self.status = "title required".into();
                            return;
                        }
                        self.mode = Mode::Input {
                            kind: InputKind::NewCommand { title: text },
                            buffer: "claude".into(),
                        };
                    }
                    InputKind::NewCommand { title } => {
                        self.mode = Mode::Input {
                            kind: InputKind::NewDir {
                                title,
                                command: text,
                            },
                            buffer: std::env::current_dir()
                                .map(|p| p.display().to_string())
                                .unwrap_or_else(|_| "~".into()),
                        };
                    }
                    InputKind::NewDir { title, command } => {
                        let dir = if text.is_empty() {
                            "~".to_string()
                        } else {
                            text
                        };
                        let dir = expand_tilde(&dir);
                        let id = model::new_id();
                        let session = model::session_name(&title, &id);
                        self.board.cards.push(Card {
                            id,
                            title: title.clone(),
                            command,
                            dir,
                            column: 0,
                            session,
                            created_at: model::now_epoch(),
                        });
                        self.save();
                        self.mode = Mode::Normal;
                        self.sel_col = 0;
                        self.sel_row = self.board.column_indices(0).len().saturating_sub(1);
                        self.status = format!("created: {title}  (Enter to start & attach)");
                        self.refresh();
                    }
                    InputKind::EditTitle { card_id } => {
                        if !text.is_empty() {
                            if let Some(c) = self.card_mut_by_id(&card_id) {
                                c.title = text;
                            }
                            self.save();
                        }
                        self.mode = Mode::Normal;
                    }
                    InputKind::EditCommand { card_id } => {
                        if let Some(c) = self.card_mut_by_id(&card_id) {
                            c.command = text;
                        }
                        self.save();
                        self.mode = Mode::Normal;
                    }
                }
            }
            _ => {}
        }
    }

    fn on_key_confirm(&mut self, code: KeyCode) {
        let Mode::Confirm(kind) = &self.mode else {
            return;
        };
        let kind = kind.clone();
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                match kind {
                    ConfirmKind::DeleteCard { card_id } => {
                        if let Some(pos) = self.board.cards.iter().position(|c| c.id == card_id) {
                            let card = self.board.cards.remove(pos);
                            if self.live_sessions.contains(&card.session) {
                                let _ = tmux::kill_session(&card.session);
                            }
                            self.status = format!("deleted: {}", card.title);
                            self.board.archived.push(card);
                            self.save();
                            self.clamp_row();
                            self.refresh();
                        }
                    }
                    ConfirmKind::KillSession { card_id } => {
                        if let Some(c) = self.board.cards.iter().find(|c| c.id == card_id) {
                            let session = c.session.clone();
                            match tmux::kill_session(&session) {
                                Ok(()) => self.status = format!("killed {session}"),
                                Err(e) => self.status = format!("kill failed: {e}"),
                            }
                            self.refresh();
                        }
                    }
                }
                self.mode = Mode::Normal;
            }
            _ => {
                self.mode = Mode::Normal;
                self.status = "cancelled".into();
            }
        }
    }

    fn move_card(&mut self, delta: i32) {
        let Some(i) = self.selected_global_index() else {
            return;
        };
        let col = self.board.cards[i].column as i32 + delta;
        if col < 0 || col >= COLUMNS.len() as i32 {
            return;
        }
        self.board.cards[i].column = col as usize;
        self.save();
        self.sel_col = col as usize;
        // Follow the card into its new column.
        let idx = self.board.column_indices(self.sel_col);
        self.sel_row = idx.iter().position(|&j| j == i).unwrap_or(0);
        self.refresh();
    }

    fn reorder_card(&mut self, delta: i32) {
        let idx = self.board.column_indices(self.sel_col);
        let Some(&cur) = idx.get(self.sel_row) else {
            return;
        };
        let target_row = self.sel_row as i32 + delta;
        if target_row < 0 || target_row >= idx.len() as i32 {
            return;
        }
        let other = idx[target_row as usize];
        self.board.cards.swap(cur, other);
        self.sel_row = target_row as usize;
        self.save();
    }
}

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~") {
        if let Some(home) = dirs::home_dir() {
            return format!("{}{}", home.display(), rest);
        }
    }
    path.to_string()
}

pub use ratatui::crossterm::event::KeyCode;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(1);

    fn temp_dir(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "deck-app-test-{tag}-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn card(id: &str, column: usize) -> Card {
        Card {
            id: id.into(),
            title: format!("card {id}"),
            command: "echo ready".into(),
            dir: "/tmp".into(),
            column,
            session: format!("deck-card-{id}"),
            created_at: 1,
        }
    }

    fn test_app(tag: &str, cards: Vec<Card>) -> App {
        App {
            board: Board {
                cards,
                archived: Vec::new(),
            },
            sel_col: 0,
            sel_row: 0,
            mode: Mode::Normal,
            status: "ready".into(),
            live_sessions: HashSet::new(),
            tail: Vec::new(),
            quit: false,
            pending: None,
            data_dir: temp_dir(tag),
        }
    }

    fn set_input(app: &mut App, value: &str) {
        let Mode::Input { buffer, .. } = &mut app.mode else {
            panic!("expected input mode")
        };
        *buffer = value.into();
    }

    #[test]
    fn navigation_clamps_to_each_column_and_quit_is_explicit() {
        let mut app = test_app("navigation", vec![card("a", 0), card("b", 0), card("c", 1)]);

        assert_eq!(app.selected_card().unwrap().id, "a");
        app.on_key(KeyCode::Down);
        assert_eq!(app.selected_card().unwrap().id, "b");
        app.on_key(KeyCode::Char('j'));
        assert_eq!(app.sel_row, 1, "selection stops at the end of a column");
        app.on_key(KeyCode::Char('k'));
        assert_eq!(app.sel_row, 0);

        app.sel_row = 1;
        app.on_key(KeyCode::Right);
        assert_eq!((app.sel_col, app.sel_row), (1, 0));
        assert_eq!(app.selected_card().unwrap().id, "c");
        app.on_key(KeyCode::Char('l'));
        assert_eq!((app.sel_col, app.sel_row), (2, 0));
        assert!(app.selected_card().is_none());
        app.on_key(KeyCode::Left);
        app.on_key(KeyCode::Char('h'));
        assert_eq!(app.sel_col, 0);

        app.on_key(KeyCode::Char('q'));
        assert!(app.quit);
    }

    #[test]
    fn new_card_wizard_validates_and_persists_every_stage() {
        let mut app = test_app("new-card", Vec::new());

        app.on_key(KeyCode::Char('n'));
        app.on_key(KeyCode::Enter);
        assert_eq!(app.status, "title required");

        for ch in "draftx".chars() {
            app.on_key(KeyCode::Char(ch));
        }
        app.on_key(KeyCode::Backspace);
        app.on_key(KeyCode::Enter);
        assert!(matches!(
            app.mode,
            Mode::Input {
                kind: InputKind::NewCommand { .. },
                ..
            }
        ));

        set_input(&mut app, "codex");
        app.on_key(KeyCode::Enter);
        assert!(matches!(
            app.mode,
            Mode::Input {
                kind: InputKind::NewDir { .. },
                ..
            }
        ));

        set_input(&mut app, "");
        app.on_key(KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.board.cards.len(), 1);
        let created = &app.board.cards[0];
        assert_eq!(created.title, "draft");
        assert_eq!(created.command, "codex");
        assert_eq!(created.dir, expand_tilde("~"));
        assert!(app.data_dir.join("board.json").is_file());
        assert!(app.status.starts_with("created: draft"));
    }

    #[test]
    fn editing_can_commit_empty_commands_and_cancel_titles() {
        let mut app = test_app("editing", vec![card("a", 0)]);

        app.on_key(KeyCode::Char('e'));
        set_input(&mut app, "discarded");
        app.on_key(KeyCode::Esc);
        assert_eq!(app.board.cards[0].title, "card a");
        assert_eq!(app.status, "cancelled");

        app.on_key(KeyCode::Char('e'));
        set_input(&mut app, "renamed");
        app.on_key(KeyCode::Enter);
        assert_eq!(app.board.cards[0].title, "renamed");

        app.on_key(KeyCode::Char('c'));
        set_input(&mut app, "");
        app.on_key(KeyCode::Enter);
        assert!(app.board.cards[0].command.is_empty());
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn move_and_reorder_follow_the_card_without_crossing_bounds() {
        let mut app = test_app(
            "moving",
            vec![card("a", 0), card("b", 0), card("c", 0), card("z", 1)],
        );

        app.on_key(KeyCode::Char('K'));
        assert_eq!(app.sel_row, 0);
        app.on_key(KeyCode::Char('J'));
        assert_eq!(app.sel_row, 1);
        assert_eq!(app.selected_card().unwrap().id, "a");
        app.on_key(KeyCode::Char('K'));
        assert_eq!(app.sel_row, 0);

        app.on_key(KeyCode::Char('H'));
        assert_eq!(app.board.cards[0].column, 0);
        app.on_key(KeyCode::Char('L'));
        assert_eq!((app.sel_col, app.sel_row), (1, 0));
        assert_eq!(app.selected_card().unwrap().id, "a");
        app.on_key(KeyCode::Char(']'));
        assert_eq!(app.sel_col, 2);
        app.on_key(KeyCode::Char('['));
        assert_eq!(app.sel_col, 1);
    }

    #[test]
    fn actions_and_confirmations_are_tokened_to_the_selected_card() {
        let mut app = test_app("actions", vec![card("a", 0), card("b", 0)]);
        app.live_sessions.insert("deck-card-a".into());

        app.on_key(KeyCode::Enter);
        match app.take_action() {
            Some(Action::Attach { session }) => assert_eq!(session, "deck-card-a"),
            _ => panic!("attach action was not queued"),
        }
        assert!(app.take_action().is_none());

        app.on_key(KeyCode::Char('s'));
        assert_eq!(app.status, "session already running");
        app.on_key(KeyCode::Char('o'));
        match app.take_action() {
            Some(Action::EditNotes { card_id }) => assert_eq!(card_id, "a"),
            _ => panic!("notes action was not queued"),
        }

        app.on_key(KeyCode::Char('x'));
        assert!(matches!(
            app.mode,
            Mode::Confirm(ConfirmKind::KillSession { .. })
        ));
        app.on_key(KeyCode::Char('n'));
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.status, "cancelled");

        app.live_sessions.clear();
        app.on_key(KeyCode::Char('x'));
        assert_eq!(app.status, "no live session");
        app.on_key(KeyCode::Char('d'));
        assert!(matches!(
            app.mode,
            Mode::Confirm(ConfirmKind::DeleteCard { .. })
        ));
        app.on_key(KeyCode::Char('Y'));
        assert_eq!(app.board.cards.len(), 1);
        assert_eq!(app.board.archived.len(), 1);
        assert_eq!(app.board.archived[0].id, "a");
        assert_eq!(app.status, "deleted: card a");
    }

    #[test]
    fn refresh_clears_stale_tail_when_the_selected_session_is_absent() {
        let mut app = test_app("refresh", vec![card("definitely-not-live", 0)]);
        app.tail = vec!["stale".into()];
        app.refresh();
        assert!(app.tail.is_empty());
    }

    #[test]
    fn tilde_expansion_preserves_ordinary_paths() {
        assert_eq!(expand_tilde("/tmp/project"), "/tmp/project");
        assert!(expand_tilde("~/project").ends_with("/project"));
    }
}
