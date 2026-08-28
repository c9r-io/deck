use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const COLUMNS: [&str; 5] = ["Backlog", "Active", "Waiting", "Review", "Done"];

#[derive(Serialize, Deserialize, Clone)]
pub struct Card {
    pub id: String,
    pub title: String,
    /// Command sent into the session's shell on start (e.g. "claude", "codex").
    pub command: String,
    /// Working directory for the session.
    pub dir: String,
    /// Index into COLUMNS.
    pub column: usize,
    /// tmux session name backing this card.
    pub session: String,
    pub created_at: u64,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Board {
    pub cards: Vec<Card>,
    #[serde(default)]
    pub archived: Vec<Card>,
}

pub fn data_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".deck")
}

// ---------- private-by-construction persistence -------------------------------
//
// Everything under ~/.deck can carry user content (card titles, shell
// commands, notes), so the whole tree is user-only: directories 0700 and
// files 0600 FROM CREATION — never "create world-readable, chmod later",
// which is a real race. Renames preserve the creation mode, so an atomic
// write through a 0600 temp file yields a 0600 destination.
// (The app backend states the same contract in app/src-tauri/src/storage.rs;
// the TUI keeps its own small copy rather than depending on that crate.)

/// Create `dir` (and parents) and restrict it to the user.
pub fn create_private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| "creating the deck data directory")?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
        .with_context(|| "restricting the deck data directory")?;
    Ok(())
}

/// Open a file for writing, user-only from the moment it exists.
fn open_private(path: &Path) -> std::io::Result<fs::File> {
    let f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // the mode above only applies on CREATION; a file an older deck left
    // 0644 keeps its bits until we say otherwise
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    Ok(f)
}

/// Boot-time, idempotent migration of a tree an older deck (or the plain
/// umask) may have left group/world-readable: directories 0700, regular
/// files 0600. Symlinks are skipped — chmod would follow them out of the
/// tree. Best effort: a single unreadable entry must not stop deck.
pub fn harden_data_dir(dir: &Path) -> Result<()> {
    if !dir.exists() {
        return create_private_dir(dir);
    }
    let mut errs = 0u32;
    let set = |p: &Path, mode: u32, errs: &mut u32| {
        if fs::set_permissions(p, fs::Permissions::from_mode(mode)).is_err() {
            *errs += 1;
        }
    };
    set(dir, 0o700, &mut errs);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else {
            errs += 1;
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() => {
                    set(&p, 0o700, &mut errs);
                    stack.push(p);
                }
                Ok(t) if t.is_file() => set(&p, 0o600, &mut errs),
                _ => {}
            }
        }
    }
    if errs > 0 {
        return Err(anyhow!(
            "could not restrict {errs} file(s) under the deck data directory to user-only access"
        ));
    }
    Ok(())
}

/// Atomic, durable write: unique same-directory temp (0600) → fsync →
/// rename → fsync of the directory. A failure anywhere leaves the previous
/// file exactly as it was.
fn atomic_write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow!("no parent directory"))?;
    let tmp = dir.join(format!(
        ".{}.tmp.{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .with_context(|| "creating the temp file")?;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

fn board_path_in(dir: &Path) -> PathBuf {
    dir.join("board.json")
}

fn bak_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_owned();
    os.push(".bak");
    PathBuf::from(os)
}

/// Notes live at `<dir>/notes/<card id>.md`. Card ids are deck-generated
/// base36, but the path is built from an id that also comes off disk, so the
/// alphabet is enforced here: no separator, no `..`, nothing that could
/// escape the notes directory.
pub fn notes_path_in(dir: &Path, card_id: &str) -> Result<PathBuf> {
    if card_id.is_empty()
        || card_id.len() > 64
        || !card_id.chars().all(|c| c.is_ascii_alphanumeric())
    {
        return Err(anyhow!("refusing a card id that is not plain alphanumeric"));
    }
    Ok(dir.join("notes").join(format!("{card_id}.md")))
}

/// Create the notes file (and its directory) privately before handing the
/// path to $EDITOR — an editor creating it would use the ambient umask.
pub fn prepare_notes(dir: &Path, card_id: &str) -> Result<PathBuf> {
    let path = notes_path_in(dir, card_id)?;
    create_private_dir(path.parent().unwrap())?;
    if !path.exists() {
        open_private(&path)?;
    } else {
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

impl Board {
    pub fn load() -> Result<Self> {
        Self::load_from(&data_dir())
    }

    /// Read the board, falling back to the `.bak` copy when the main file is
    /// unreadable or malformed (a truncated file must not silently become an
    /// empty board). A genuinely absent file is a first run.
    pub fn load_from(dir: &Path) -> Result<Self> {
        let path = board_path_in(dir);
        let parse = |p: &Path| -> Result<Self> {
            let raw = fs::read_to_string(p).with_context(|| "reading the board file")?;
            serde_json::from_str(&raw).with_context(|| "parsing the board file")
        };
        if !path.exists() {
            return match bak_path(&path).exists() {
                true => parse(&bak_path(&path)),
                false => Ok(Board::default()),
            };
        }
        match parse(&path) {
            Ok(b) => Ok(b),
            Err(e) => {
                if bak_path(&path).exists() {
                    if let Ok(b) = parse(&bak_path(&path)) {
                        return Ok(b);
                    }
                }
                Err(e)
            }
        }
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&data_dir())
    }

    /// Atomic and durable: the previous version is kept as `board.json.bak`
    /// (same 0600 atomic write), then the new one replaces the main file in
    /// a single rename. A failure at any point leaves the old board intact.
    pub fn save_to(&self, dir: &Path) -> Result<()> {
        create_private_dir(dir)?;
        let path = board_path_in(dir);
        let out = serde_json::to_string_pretty(self)?;
        if let Ok(cur) = fs::read(&path) {
            atomic_write_private(&bak_path(&path), &cur).with_context(|| "writing the backup")?;
        }
        atomic_write_private(&path, out.as_bytes())
    }

    /// Indices (into self.cards) of cards in the given column, in board order.
    pub fn column_indices(&self, col: usize) -> Vec<usize> {
        self.cards
            .iter()
            .enumerate()
            .filter(|(_, c)| c.column == col)
            .map(|(i, _)| i)
            .collect()
    }
}

pub fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Compact unique id: epoch millis in base36.
pub fn new_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    to_base36(millis)
}

fn to_base36(mut n: u128) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

/// tmux session name derived from title + id suffix, e.g. "deck-fix-login-abc1".
/// The output alphabet ([a-z0-9-], never leading '-') stays inside what tmux
/// accepts as a target and what can never parse as a CLI flag.
pub fn session_name(title: &str, id: &str) -> String {
    let slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.is_empty() { "card" } else { &slug };
    let slug: String = slug.chars().take(24).collect();
    let tail: String = id
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("deck-{}-{}", slug.trim_matches('-'), tail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str, column: usize) -> Card {
        Card {
            id: id.into(),
            title: format!("card {id}"),
            command: String::new(),
            dir: String::new(),
            column,
            session: session_name(&format!("card {id}"), id),
            created_at: 0,
        }
    }

    #[test]
    fn session_names_are_tmux_safe() {
        assert_eq!(session_name("Fix login", "abc123"), "deck-fix-login-c123");
        // punctuation, spaces and unicode collapse to the safe alphabet
        for (title, id) in [
            ("héllo: wörld!!", "zz99"),
            ("  --weird--  ", "1"),
            ("", "abcd"),
            ("日本語だけ", "xy12"),
        ] {
            let s = session_name(title, id);
            assert!(s.starts_with("deck-"), "{s}");
            assert!(
                s.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "unsafe char in {s}"
            );
            assert!(!s.contains("--") || !s.ends_with('-'), "{s}");
        }
        // empty/symbol-only titles still yield a usable name
        assert_eq!(session_name("", "abcd"), "deck-card-abcd");
        // long titles are truncated but keep the unique id tail
        let long = session_name(&"x".repeat(100), "tail9999");
        assert!(long.len() <= 5 + 24 + 1 + 4, "{long}");
        assert!(long.ends_with("-9999"));
    }

    #[test]
    fn column_indices_filter_and_preserve_board_order() {
        let b = Board {
            cards: vec![card("a", 0), card("b", 1), card("c", 0), card("d", 4)],
            archived: Vec::new(),
        };
        assert_eq!(b.column_indices(0), vec![0, 2]);
        assert_eq!(b.column_indices(1), vec![1]);
        assert_eq!(b.column_indices(2), Vec::<usize>::new());
        assert_eq!(b.column_indices(4), vec![3]);
    }

    #[test]
    fn legacy_board_json_without_archived_still_loads() {
        // pre-archive board.json files have no "archived" key
        let raw = r#"{"cards":[{"id":"a","title":"t","command":"","dir":"/tmp",
                       "column":2,"session":"deck-t-a","created_at":5}]}"#;
        let b: Board = serde_json::from_str(raw).expect("legacy shape loads");
        assert_eq!(b.cards.len(), 1);
        assert_eq!(b.cards[0].column, 2);
        assert!(b.archived.is_empty(), "archived defaults to empty");
        // and round-trips with the field present
        let again: Board = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
        assert_eq!(again.cards[0].id, "a");
    }

    // ---------- persistence: real filesystem metadata, never ~/.deck ----------

    fn tdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("deck-tui-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }
    fn mode_of(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }
    fn set_mode(p: &Path, mode: u32) {
        fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
    }
    fn board_with(id: &str) -> Board {
        Board {
            cards: vec![card(id, 1)],
            archived: Vec::new(),
        }
    }

    #[test]
    fn a_first_save_creates_a_private_tree() {
        let d = tdir("first");
        board_with("a").save_to(&d).unwrap();
        assert_eq!(mode_of(&d), 0o700, "data dir is user-only");
        assert_eq!(mode_of(&d.join("board.json")), 0o600, "board file");
        // no temp litter left behind
        assert!(!fs::read_dir(&d).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".tmp.")));
        assert_eq!(Board::load_from(&d).unwrap().cards[0].id, "a");
    }

    #[test]
    fn a_second_save_keeps_the_backup_private_too() {
        let d = tdir("second");
        board_with("a").save_to(&d).unwrap();
        board_with("b").save_to(&d).unwrap();
        assert_eq!(mode_of(&d.join("board.json")), 0o600);
        assert_eq!(mode_of(&d.join("board.json.bak")), 0o600, "backup");
        assert_eq!(Board::load_from(&d).unwrap().cards[0].id, "b");
    }

    #[test]
    fn legacy_lax_permissions_are_migrated_idempotently() {
        let d = tdir("harden");
        board_with("a").save_to(&d).unwrap();
        board_with("b").save_to(&d).unwrap();
        let notes = d.join("notes");
        fs::create_dir_all(&notes).unwrap();
        let note = notes.join("a.md");
        fs::write(&note, "secret note").unwrap();
        // simulate what a pre-0.4.29 deck (plain umask) left behind
        for p in [d.join("board.json"), d.join("board.json.bak"), note.clone()] {
            set_mode(&p, 0o644);
        }
        set_mode(&notes, 0o755);
        set_mode(&d, 0o755);

        harden_data_dir(&d).unwrap();
        assert_eq!(mode_of(&d), 0o700);
        assert_eq!(mode_of(&notes), 0o700);
        for p in [d.join("board.json"), d.join("board.json.bak"), note] {
            assert_eq!(mode_of(&p), 0o600, "{}", p.display());
        }
        // running it again changes nothing and still succeeds
        harden_data_dir(&d).unwrap();
        assert_eq!(mode_of(&d), 0o700);
        // and a missing directory is simply created private
        let fresh = tdir("harden-fresh");
        harden_data_dir(&fresh).unwrap();
        assert_eq!(mode_of(&fresh), 0o700);
    }

    #[test]
    fn a_failed_save_leaves_the_previous_board_intact() {
        let d = tdir("failsave");
        board_with("keep").save_to(&d).unwrap();
        let before = fs::read_to_string(d.join("board.json")).unwrap();
        // an unwritable backup target (here: a non-empty directory in its
        // place) makes the save fail before the main file is ever replaced
        fs::create_dir(d.join("board.json.bak")).unwrap();
        fs::write(d.join("board.json.bak").join("x"), "x").unwrap();
        let err = board_with("lost").save_to(&d).unwrap_err();
        assert!(format!("{err:#}").contains("backup"), "{err:#}");
        assert_eq!(
            fs::read_to_string(d.join("board.json")).unwrap(),
            before,
            "the old board survived untouched"
        );
        assert_eq!(Board::load_from(&d).unwrap().cards[0].id, "keep");
    }

    #[test]
    fn a_damaged_board_falls_back_to_the_backup() {
        let d = tdir("recover");
        board_with("old").save_to(&d).unwrap();
        board_with("new").save_to(&d).unwrap(); // .bak now holds "old"
        fs::write(d.join("board.json"), "{truncated").unwrap();
        assert_eq!(Board::load_from(&d).unwrap().cards[0].id, "old");
        // both damaged is an error, never a silently empty board
        fs::write(d.join("board.json.bak"), "{worse").unwrap();
        assert!(Board::load_from(&d).is_err());
        // a genuinely absent file IS a first run
        let fresh = tdir("recover-fresh");
        fs::create_dir_all(&fresh).unwrap();
        assert!(Board::load_from(&fresh).unwrap().cards.is_empty());
    }

    #[test]
    fn notes_are_private_and_cannot_escape_their_directory() {
        let d = tdir("notes");
        let p = prepare_notes(&d, "abc123").unwrap();
        assert_eq!(mode_of(&d.join("notes")), 0o700, "notes dir");
        assert_eq!(mode_of(&p), 0o600, "notes file");
        assert!(p.starts_with(d.join("notes")));
        // an existing lax file is tightened on the next open
        set_mode(&p, 0o644);
        prepare_notes(&d, "abc123").unwrap();
        assert_eq!(mode_of(&p), 0o600);
        // ids that could climb out of the notes directory are refused
        for bad in [
            "../../etc/passwd",
            "..",
            "a/b",
            "a.md",
            "",
            "with space",
            "-flag",
        ] {
            assert!(notes_path_in(&d, bad).is_err(), "{bad:?} was accepted");
        }
        assert!(notes_path_in(&d, &"x".repeat(65)).is_err(), "absurd id");
        // every id deck itself generates is accepted
        assert!(notes_path_in(&d, &new_id()).is_ok());
    }

    #[test]
    fn base36_ids_are_compact_and_ordered_by_time() {
        assert_eq!(to_base36(0), "0");
        assert_eq!(to_base36(35), "z");
        assert_eq!(to_base36(36), "10");
        let id = new_id();
        assert!(!id.is_empty());
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric()));
    }
}
