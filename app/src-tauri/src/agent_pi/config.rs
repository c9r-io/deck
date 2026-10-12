//! Pi's one settings entry: which of Deck's three bundled observer entries
//! Pi loads, if any. The two Settings switches are derived from it and
//! written through it; nothing else stores them.
//!
//! # Contract
//! - **One source of truth, in Pi's own file.** Pi's agent-directory
//!   `settings.json` has an `extensions` array. Deck owns at most ONE string
//!   in it: the absolute path of `signal.ts`, `lifecycle.ts` or `both.ts`
//!   inside the Deck bundle (`<bundle>/Contents/Resources/pi/`). Which one is
//!   there IS the pair of switches; no entry is both off. Deck keeps no
//!   boolean of its own, and the backend's admission asks this file too
//!   (`enabled`). Turning one switch replaces the entry by the one that
//!   carries the resulting pair, in one write: there is never a moment with
//!   two entries or with a capability nobody asked for.
//! - **Nothing executable under the home directory.** The observer's code
//!   stays in the signed bundle; the settings file gets a path. Before a
//!   path is written, the four files it leads to must be regular files in
//!   that bundle with exactly the bytes this build was compiled with.
//! - **Only from a stable install.** A write needs Deck to run from
//!   `/Applications/deck.app` or `~/Applications/deck.app`
//!   (`tmux_lifecycle::stable_installed_bundle`), turning a switch off
//!   included. A development or smoke bundle reads the file and writes
//!   nothing (`unsupported-location`).
//! - **Ownership is an exact path.** An entry is Deck's when it equals one
//!   of those three files under one of the two stable bundle locations —
//!   never a file name, a substring or a pattern. Every other string in the
//!   array, every other key and every other byte of the file is the user's
//!   and stays as written: the text is edited in place with the span tools
//!   of `agent_status` and is used only when it parses back to exactly the
//!   value that was decided. The file keeps its mode and, when it is a
//!   symlink, stays one (`datadir::atomic_write_through_link`).
//! - **What is not Deck's to repair is refused.** A file that is not JSON,
//!   holds a repeated key anywhere, whose `extensions` is not an array of
//!   strings, or whose layout the span tools cannot follow is left alone and
//!   reported (`unreadable`). A missing agent directory is not created: a
//!   switch is refused. That the directory is absent is all Deck knows — it
//!   is no fact about whether Pi ever ran on this Mac, and is not worded as
//!   one.
//! - **The user's own disabling wins.** Pi reads `-<path>` as "never load
//!   this". While the array holds such an entry for one of Deck's paths,
//!   nothing Deck does enables an observer: a switch is refused, the boot
//!   migration writes nothing, and the backend admits none (`enabled`).
//! - **One observer, never two.** Several entries of Deck's would make Pi
//!   load several observers. That is `conflict`: the backend admits none of
//!   them, and the next switch — or the boot migration — replaces them with
//!   the one entry that carries their capabilities together.
//! - **Boot migration moves, it never enrolls.** It rewrites entries that
//!   are already there to this bundle's path (a moved install), keeping the
//!   capabilities they carried. No entry, or a disabling one: nothing.
//! - **Another writer is not overwritten knowingly.** The file is read
//!   again immediately before the write; if it changed, the edit is computed
//!   once more from what is there now, and a second change refuses the
//!   write. Pi itself writes this file under its own lock, which Deck does
//!   not take: a change that lands between that last read and the rename is
//!   lost. That window is short and is not closed.
//! - **Configured is not loaded.** A switch says what Pi will load when it
//!   next starts or reloads. Whether a running Pi has an observer is known
//!   only from a stream the transport verified. Turning a switch OFF acts on
//!   a running Pi at once (its stream loses that capability and keeps the
//!   other, `transport`); turning one ON does not: `needs-reload` says that
//!   a running Pi does something else than the file names now and will go
//!   on doing so until it loads its extensions again — its stream may do
//!   other things than the entry says, or its observer was ended for its
//!   capabilities and does not come back by itself.

use std::path::{Path, PathBuf};

use serde::de::{DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::Value;

use super::{Capabilities, Capability};
use crate::agent_status::{
    items, layout, read_settings_text, rebuilt, skip_space, value_end, written,
};
use crate::applog::applog;
use crate::error::{DeckError, ErrorKind};
use crate::sync::LockRecover;

/// One of the three bundled entries: the capability pair it carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Variant {
    Signal,
    Lifecycle,
    Both,
}

impl Variant {
    const ALL: [Variant; 3] = [Variant::Signal, Variant::Lifecycle, Variant::Both];

    fn file(self) -> &'static str {
        match self {
            Variant::Signal => "signal.ts",
            Variant::Lifecycle => "lifecycle.ts",
            Variant::Both => "both.ts",
        }
    }

    pub(crate) fn capabilities(self) -> Capabilities {
        Capabilities {
            signal: matches!(self, Variant::Signal | Variant::Both),
            lifecycle: matches!(self, Variant::Lifecycle | Variant::Both),
        }
    }

    /// The entry that carries exactly `capabilities`; none carries nothing.
    fn of(capabilities: Capabilities) -> Option<Variant> {
        Variant::ALL
            .into_iter()
            .find(|variant| variant.capabilities() == capabilities)
    }
}

/// The observer's files as this build was compiled with them: what a bundle
/// must hold, byte for byte, before a path into it is written anywhere.
pub(crate) const RESOURCES: [(&str, &str); 4] = [
    (
        "observer.ts",
        include_str!("../../resources/pi/observer.ts"),
    ),
    ("signal.ts", include_str!("../../resources/pi/signal.ts")),
    (
        "lifecycle.ts",
        include_str!("../../resources/pi/lifecycle.ts"),
    ),
    ("both.ts", include_str!("../../resources/pi/both.ts")),
];

/// Where the observer's files are inside a bundle.
const RESOURCE_DIR: &str = "Contents/Resources/pi";

/// What can be said about Deck's entry in Pi's settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Installation {
    /// No entry of Deck's.
    #[default]
    Absent,
    /// Exactly this bundle's one entry.
    Current,
    /// As `Current`, and a connected observer still has other capabilities:
    /// that Pi loaded its extensions before the entry changed.
    NeedsReload,
    /// Deck's entries are not one entry of this bundle, or the user's own
    /// entry disables one. Nothing is observed until that is resolved.
    Conflict,
    /// The file cannot be read as the document Deck may edit.
    Unreadable,
    /// This Deck does not run from a stable install and writes nothing.
    UnsupportedLocation,
}

/// The two switches and the entry's state, all read from the file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Status {
    pub(crate) signal: bool,
    pub(crate) lifecycle: bool,
    pub(crate) installation: Installation,
}

/// Where everything is for one read or edit. Production's is `place`; a
/// test names its own directories.
#[derive(Clone, Debug)]
pub(crate) struct Place {
    /// Pi's agent-directory settings file.
    pub(crate) settings: PathBuf,
    /// The bundle this Deck runs from, only when it is a stable install.
    pub(crate) bundle: Option<PathBuf>,
    /// The bundle locations whose entries are Deck's.
    pub(crate) owned: Vec<PathBuf>,
}

const NEVER_RAN: &str = "pi-never-ran";

fn refused(kind: ErrorKind, code: &'static str) -> DeckError {
    DeckError::new(kind, code)
}

/// Pi's agent directory by Pi's own public rule, resolved once per call:
/// `PI_CODING_AGENT_DIR` when Deck's environment has it (with Pi's `~`
/// expansion), else `~/.pi/agent`. A variable set only in a shell's startup
/// files is not visible here and is not looked for: Deck then configures
/// the default directory, and a Pi that reads another one never connects.
fn agent_directory(home: &Path, variable: Option<std::ffi::OsString>) -> Option<PathBuf> {
    let Some(given) = variable.filter(|value| !value.is_empty()) else {
        return Some(home.join(".pi").join("agent"));
    };
    let given = given.to_str()?;
    let expanded = if given == "~" {
        home.to_path_buf()
    } else if let Some(rest) = given.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(given)
    };
    // a relative directory depends on each Pi's own working directory
    expanded.is_absolute().then_some(expanded)
}

/// Production's place; `None` without a home directory or with an agent
/// directory Deck cannot name.
fn place() -> Option<Place> {
    let home = dirs::home_dir()?;
    let directory = agent_directory(&home, std::env::var_os("PI_CODING_AGENT_DIR"))?;
    let bundle = std::env::current_exe()
        .ok()
        .as_deref()
        .and_then(crate::tmux_lifecycle::app_bundle_root)
        .filter(|bundle| crate::tmux_lifecycle::stable_installed_bundle(bundle))
        .map(Path::to_path_buf);
    Some(Place {
        settings: directory.join("settings.json"),
        bundle,
        owned: vec![
            PathBuf::from("/Applications/deck.app"),
            home.join("Applications").join("deck.app"),
        ],
    })
}

/// The settings string that names `variant` inside `bundle`.
fn entry_path(bundle: &Path, variant: Variant) -> Option<String> {
    bundle
        .join(RESOURCE_DIR)
        .join(variant.file())
        .to_str()
        .map(str::to_owned)
}

/// The four files in `bundle` are regular files with this build's bytes.
fn resources_present(bundle: &Path) -> bool {
    RESOURCES.iter().all(|(name, content)| {
        let path = bundle.join(RESOURCE_DIR).join(name);
        std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file())
            && std::fs::read(&path).is_ok_and(|bytes| bytes == content.as_bytes())
    })
}

// ---------- reading: a strict document ------------------------------------------

/// `serde_json::Value`, refusing a repeated key anywhere: serde keeps the
/// last of two equal keys, Pi's own parser does the same, and Deck must not
/// edit a document that means something else to whoever reads it next.
struct Strict;

impl<'de> DeserializeSeed<'de> for Strict {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(StrictVisitor)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a JSON value without repeated keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Value, E> {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("not a number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = seq.next_element_seed(Strict)? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        use serde::de::Error;
        let mut object = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if object.contains_key(&key) {
                return Err(A::Error::custom("repeated key"));
            }
            let value = map.next_value_seed(Strict)?;
            object.insert(key, value);
        }
        Ok(Value::Object(object))
    }
}

fn strict(text: &str) -> Option<Value> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let value = Strict.deserialize(&mut deserializer).ok()?;
    deserializer.end().ok()?;
    Some(value)
}

/// The document a settings text holds and its `extensions` strings. `None`
/// entries: the key is absent. An error: not a document Deck may edit.
fn document(text: Option<&str>) -> Result<(Value, Option<Vec<String>>), DeckError> {
    let unreadable = || refused(ErrorKind::InvalidDoc, "pi-config-unreadable");
    let value = match text {
        Some(text) => strict(text).ok_or_else(unreadable)?,
        None => serde_json::json!({}),
    };
    let object = value.as_object().ok_or_else(unreadable)?;
    let entries = match object.get("extensions") {
        None => None,
        Some(Value::Array(list)) => Some(
            list.iter()
                .map(|entry| entry.as_str().map(str::to_owned))
                .collect::<Option<Vec<String>>>()
                .ok_or_else(unreadable)?,
        ),
        Some(_) => return Err(unreadable()),
    };
    Ok((value, entries))
}

// ---------- ownership --------------------------------------------------------------

/// Which of Deck's own files an entry names, by exact path.
fn owned_variant(path: &str, place: &Place) -> Option<Variant> {
    place.owned.iter().find_map(|root| {
        Variant::ALL
            .into_iter()
            .find(|variant| entry_path(root, *variant).as_deref() == Some(path))
    })
}

/// What Deck's own entries of one array say.
#[derive(Debug, Default, PartialEq, Eq)]
struct Owned {
    /// (index, the entry it is) of every entry that loads one of Deck's files
    positive: Vec<(usize, Variant)>,
    /// the user disabled one of Deck's files (`-<path>`)
    disabled: bool,
}

impl Owned {
    fn of(entries: &[String], place: &Place) -> Self {
        let mut owned = Owned::default();
        for (index, entry) in entries.iter().enumerate() {
            if let Some(variant) = owned_variant(entry, place) {
                owned.positive.push((index, variant));
            } else if entry
                .strip_prefix('-')
                .is_some_and(|path| owned_variant(path, place).is_some())
            {
                owned.disabled = true;
            }
        }
        owned
    }

    /// The capabilities Deck's entries carry together.
    fn capabilities(&self) -> Capabilities {
        self.positive
            .iter()
            .fold(Capabilities::NONE, |all, (_, variant)| Capabilities {
                signal: all.signal || variant.capabilities().signal,
                lifecycle: all.lifecycle || variant.capabilities().lifecycle,
            })
    }
}

// ---------- the edit ----------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edit {
    /// A Settings switch.
    Set(Capability, bool),
    /// The boot migration.
    Migrate,
}

/// The file's text after `edit`, or `None` when it already is what the edit
/// asks for.
fn edited(
    place: &Place,
    bundle: &Path,
    original: Option<&str>,
    edit: Edit,
) -> Result<Option<String>, DeckError> {
    let (value, entries) = document(original)?;
    let entries = entries.unwrap_or_default();
    let owned = Owned::of(&entries, place);
    let desired = match edit {
        Edit::Set(capability, on) => Variant::of(owned.capabilities().with(capability, on)),
        // moves what is there; enrolls nobody and re-enables nothing
        Edit::Migrate if owned.positive.is_empty() || owned.disabled => return Ok(None),
        Edit::Migrate => Variant::of(owned.capabilities()),
    };
    let target = match desired {
        Some(variant) => {
            if owned.disabled {
                return Err(refused(ErrorKind::Other, "pi-config-conflict"));
            }
            if !resources_present(bundle) {
                return Err(refused(ErrorKind::Missing, "pi-resource-missing"));
            }
            Some(
                entry_path(bundle, variant)
                    .ok_or_else(|| refused(ErrorKind::Other, "pi-config-unsupported-location"))?,
            )
        }
        None => None,
    };
    // already exactly what is asked for: nothing is written
    let current: Vec<&str> = owned
        .positive
        .iter()
        .map(|(index, _)| entries[*index].as_str())
        .collect();
    if current == target.as_deref().into_iter().collect::<Vec<_>>() {
        return Ok(None);
    }
    // the decided array: Deck's entries out, the one target where the first was
    let first = owned.positive.first().map(|(index, _)| *index);
    let mut next: Vec<String> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        if Some(index) == first {
            next.extend(target.clone());
        } else if !owned.positive.iter().any(|(at, _)| *at == index) {
            next.push(entry.clone());
        }
    }
    if first.is_none() {
        next.extend(target.clone());
    }
    let emptied = next.is_empty() && !owned.positive.is_empty();
    let mut decided = value.clone();
    if let Some(object) = decided.as_object_mut() {
        if emptied {
            // an array only Deck's entry populated leaves no husk
            object.remove("extensions");
        } else {
            object.insert("extensions".into(), serde_json::json!(next));
        }
    }
    let Some(original) = original else {
        let pretty = serde_json::to_string_pretty(&decided).map_err(DeckError::from)?;
        return Ok(Some(format!("{pretty}\n")));
    };
    text_edited(original, &owned, target.as_deref(), emptied)
        .filter(|text| strict(text).is_some_and(|value| value == decided))
        .map(Some)
        .ok_or_else(|| refused(ErrorKind::Other, "pi-config-layout"))
}

/// `text` with Deck's entries of the `extensions` array cut out and `target`
/// put where the first of them was (at the end when there was none); with
/// `emptied`, the key itself goes. Every other byte stays where it was.
/// `None` when the text is not laid out the way the span tools read it.
fn text_edited(text: &str, owned: &Owned, target: Option<&str>, emptied: bool) -> Option<String> {
    let bytes = text.as_bytes();
    let start = skip_space(bytes, 0);
    if bytes.get(start) != Some(&b'{') {
        return None;
    }
    let root = start..value_end(bytes, start)?;
    let members = items(text, &root)?;
    let replaced = |span: &std::ops::Range<usize>, with: String| {
        format!("{}{with}{}", &text[..span.start], &text[span.end..])
    };
    let Some(member) = members
        .iter()
        .find(|member| member.key.as_deref() == Some("extensions"))
    else {
        // no array yet: the key goes after the last one the document has
        let lead = layout(text, &root, &members).lead;
        let list = written(&serde_json::json!([target?]), text, &lead)?;
        let kept: Vec<Option<String>> = members
            .iter()
            .map(|member| Some(text[member.whole.clone()].to_string()))
            .collect();
        let added = [format!("\"extensions\": {list}")];
        return Some(replaced(
            &root,
            rebuilt(text, &root, &members, &kept, &added),
        ));
    };
    if emptied {
        let kept: Vec<Option<String>> = members
            .iter()
            .map(|other| {
                (other.whole != member.whole).then(|| text[other.whole.clone()].to_string())
            })
            .collect();
        return Some(replaced(&root, rebuilt(text, &root, &members, &kept, &[])));
    }
    let elements = items(text, &member.value)?;
    let target = match target {
        Some(target) => Some(serde_json::to_string(target).ok()?),
        None => None,
    };
    let first = owned.positive.first().map(|(index, _)| *index);
    let kept: Vec<Option<String>> = elements
        .iter()
        .enumerate()
        .map(|(index, element)| {
            if Some(index) == first {
                target.clone()
            } else if owned.positive.iter().any(|(at, _)| *at == index) {
                None
            } else {
                Some(text[element.whole.clone()].to_string())
            }
        })
        .collect();
    let added: Vec<String> = if first.is_none() {
        target.into_iter().collect()
    } else {
        Vec::new()
    };
    Some(replaced(
        &member.value,
        rebuilt(text, &member.value, &elements, &kept, &added),
    ))
}

/// Apply one edit to the file. Says whether it was written.
fn apply(place: &Place, edit: Edit) -> Result<bool, DeckError> {
    apply_between(place, edit, &|| {})
}

/// `apply`, with `between` run after the edit was computed and before the
/// file is read again (a test's stand-in for another writer).
fn apply_between(place: &Place, edit: Edit, between: &dyn Fn()) -> Result<bool, DeckError> {
    let bundle = place
        .bundle
        .as_deref()
        .ok_or_else(|| refused(ErrorKind::Other, "pi-config-unsupported-location"))?;
    let unreadable = |_| refused(ErrorKind::InvalidDoc, "pi-config-unreadable");
    let mut original = read_settings_text(&place.settings).map_err(unreadable)?;
    let mut recomputed = false;
    loop {
        let Some(next) = edited(place, bundle, original.as_deref(), edit)? else {
            return Ok(false);
        };
        between();
        // what another writer did meanwhile is not overwritten knowingly:
        // the edit is made once more from what is there now, and no more
        let now = read_settings_text(&place.settings).map_err(unreadable)?;
        if now != original {
            if recomputed {
                return Err(refused(ErrorKind::Other, "pi-config-busy"));
            }
            recomputed = true;
            original = now;
            continue;
        }
        crate::agent_status::write_agent_config(&place.settings, next.as_bytes(), NEVER_RAN)?;
        return Ok(true);
    }
}

// ---------- what the file says --------------------------------------------------------

fn status_in(place: &Place, mismatched: impl FnOnce(Capabilities) -> bool) -> Status {
    let read = read_settings_text(&place.settings)
        .ok()
        .and_then(|text| document(text.as_deref()).ok());
    let Some((_, entries)) = read else {
        return Status {
            installation: Installation::Unreadable,
            ..Status::default()
        };
    };
    let entries = entries.unwrap_or_default();
    let owned = Owned::of(&entries, place);
    let capabilities = owned.capabilities();
    let installation = match (owned.positive.as_slice(), place.bundle.as_deref()) {
        // several entries, or one the user's own entry disables
        ([_, _, ..], _) => Installation::Conflict,
        ([_], _) if owned.disabled => Installation::Conflict,
        (_, None) => Installation::UnsupportedLocation,
        ([], Some(_)) => Installation::Absent,
        ([(index, variant)], Some(bundle)) => {
            // an entry of the other stable location is not what this
            // install writes: the next switch, or the next boot, moves it
            if entry_path(bundle, *variant).as_deref() != Some(entries[*index].as_str()) {
                Installation::Conflict
            } else if mismatched(capabilities) {
                Installation::NeedsReload
            } else {
                Installation::Current
            }
        }
    };
    Status {
        signal: capabilities.signal,
        lifecycle: capabilities.lifecycle,
        installation,
    }
}

/// The capabilities an observer may have NOW: exactly one entry of Deck's
/// and no disabling one. Anything else — no entry, several, a file Deck
/// cannot read — admits no observer.
fn enabled_in(place: &Place) -> Capabilities {
    let Some((_, entries)) = read_settings_text(&place.settings)
        .ok()
        .and_then(|text| document(text.as_deref()).ok())
    else {
        return Capabilities::NONE;
    };
    let owned = Owned::of(&entries.unwrap_or_default(), place);
    match owned.positive.as_slice() {
        [(_, variant)] if !owned.disabled => variant.capabilities(),
        _ => Capabilities::NONE,
    }
}

/// What `enabled` last read, and of which state of the file.
type FileState = (PathBuf, u64, u64, u64, i64, i64);
static ENABLED: std::sync::Mutex<Option<(Option<FileState>, Capabilities)>> =
    std::sync::Mutex::new(None);

fn file_state(path: &Path) -> Option<FileState> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    Some((
        path.to_path_buf(),
        meta.dev(),
        meta.ino(),
        meta.size(),
        meta.mtime(),
        meta.mtime_nsec(),
    ))
}

/// The capabilities Pi's settings enable right now, for the transport's
/// admission. The file is read again whenever its state changed; one `stat`
/// otherwise. No home directory or no nameable agent directory: none.
pub(crate) fn enabled() -> Capabilities {
    let Some(place) = place() else {
        return Capabilities::NONE;
    };
    let state = file_state(&place.settings);
    let mut cached = ENABLED.lock_or_recover();
    if let Some((known, capabilities)) = cached.as_ref() {
        if *known == state && state.is_some() {
            return *capabilities;
        }
    }
    let capabilities = enabled_in(&place);
    *cached = Some((state, capabilities));
    capabilities
}

/// The two switches and the entry's state (`agent_hooks_status`).
pub(crate) fn status() -> Status {
    place()
        .map(|place| status_in(&place, super::transport::mismatched))
        .unwrap_or_default()
}

/// One Settings switch: the user's explicit choice, written into Pi's
/// settings as the one entry that carries the resulting capabilities. The
/// backend tightens at once; a running Pi follows when it next loads its
/// extensions.
pub(crate) fn set(capability: Capability, enable: bool) -> Result<(), DeckError> {
    let result = place()
        .ok_or_else(|| refused(ErrorKind::Missing, "pi-config-unlocated"))
        .and_then(|place| apply(&place, Edit::Set(capability, enable)));
    applog(&format!(
        "[agent-pi] {} {} {}",
        match capability {
            Capability::Signal => "signal",
            Capability::Lifecycle => "lifecycle",
        },
        if enable { "install" } else { "remove" },
        result.as_ref().map_or_else(|e| e.code(), |_| "ok")
    ));
    if result.is_ok() {
        // what was turned off stops being observed now, not at the next frame
        super::transport::settings_changed();
    }
    result.map(|_written| ())
}

/// The lifecycle switch's own command: observing Pi's native session is an
/// opt-in of its own, beside the signal switch (`agent_hooks_set`). It
/// turns on no Signal, and nothing it observes makes Pi re-enterable.
#[tauri::command]
pub(crate) fn agent_pi_lifecycle_set(enable: bool) -> Result<(), DeckError> {
    if crate::extensions::agent(super::SOURCE).is_none() {
        return Err(DeckError::new(ErrorKind::Other, "unknown agent"));
    }
    set(Capability::Lifecycle, enable)
}

/// The boot migration (stable installs only): entries that are there are
/// moved to this bundle, nothing else. Called beside
/// `agent_status::migrate_hooks_on_boot`.
pub(crate) fn migrate_on_boot() {
    let Some(place) = place().filter(|place| place.bundle.is_some()) else {
        return;
    };
    match apply(&place, Edit::Migrate) {
        Ok(false) => {}
        Ok(true) => applog("[agent-pi] entry rewritten to the current bundle"),
        Err(e) => applog(&format!("[agent-pi] migration FAILED ({})", e.code())),
    }
}

#[cfg(test)]
mod tests;
