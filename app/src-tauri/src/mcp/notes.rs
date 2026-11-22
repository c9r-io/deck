//! Agent notes: what an Agent leaves in its own card's scratchpad for the
//! user to judge when they come back (approved 2026-11-23).
//!
//! # Contract
//! - `deck_card_note` writes, revises or withdraws ONE note in the card the
//!   call comes from. WHICH card is never a claim: `origin.rs` proves it
//!   from the kernel (the adapter's process, its pane, the card); a call
//!   from no card is `NOT_IN_A_CARD`. Any locally authorized client may
//!   write (no separate choice: a note is presentation data in the
//!   caller's own card), and nothing else: it cannot name another card.
//! - A note is the `agent` kind of scratchpad entry (`buffer-model.js`,
//!   `documents.rs` `valid_agent_note`): a one-line title of at most
//!   `AGENT_NOTE_TITLE_MAX` characters, a text of at most
//!   `AGENT_NOTE_TEXT_MAX_BYTES`, whether the Agent already went ahead with
//!   its own recommendation (`acted`), the client's own `key`, the client's
//!   id and name. The same key revises the same note while it is open; a
//!   note the user handled is closed to its writer (`NOTE_HANDLED`: write a
//!   new key). Withdrawing removes an open note. At most
//!   `AGENT_NOTES_OPEN_MAX` open notes a card, `NOTES_PER_MINUTE` writes a
//!   card a minute.
//! - The write is a Board intent (`CARD_NOTE_KIND`) the webview's one
//!   transaction commits or refuses (`mcp.js` `putNote`), checked here
//!   first against the committed CURRENT Board; the caller follows it with
//!   `deck_operation_get`. Idempotent by request id; a Deck restart never
//!   replays it (it becomes ambiguous).
//! - `deck_card_notes` reads the caller's OWN notes of its card (key,
//!   title, acted, open or handled) and nothing else: never the user's
//!   notes, never another client's.
//! - A note is data at rest. It queues, sends, wakes, moves, closes and
//!   authorizes nothing, and the user cannot queue it (`addQueueCopy`
//!   refuses the kind); they mark it handled or answer it with a note of
//!   their own. Like any kept scratchpad it keeps the card from every
//!   automatic close (`retainedBuffer`). No notification reads it.
//! - Text is logged nowhere; app.log gets closed words only.

use super::*;

/// The Board intent a `deck_card_note` journals for the webview's one
/// transaction (`mcp.js` `putNote`).
pub(super) const CARD_NOTE_KIND: &str = "card-note";
/// Note writes one card may make in a minute.
const NOTES_PER_MINUTE: usize = 12;
const NOTE: &str = "A note is for the local user to judge when they come back: a question you need them to decide, or a choice you made for them. Progress belongs in your final message, not here. Nothing you write here is sent anywhere or read back to you except through deck_card_notes.";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NoteArgs {
    request_id: String,
    /// The client's own name for this note: the same key revises it.
    key: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    text: Option<String>,
    /// The Agent already went ahead with its own recommendation.
    #[serde(default)]
    acted: bool,
    #[serde(default)]
    withdraw: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NotesArgs {}

fn not_in_a_card() -> Value {
    error_value(
        "NOT_IN_A_CARD",
        "Deck does not see this call coming from an agent running in one of its cards",
        "Only an agent running in a Deck card can leave a note in that card. Otherwise put the question in your final message.",
    )
}

fn card_gone() -> Value {
    error_value(
        "CARD_NOT_FOUND",
        "the card this call comes from is not on the current Board",
        "The card may be starting or closing; put the question in your final message.",
    )
}

fn window() -> &'static Mutex<HashMap<String, Vec<Instant>>> {
    static WINDOW: OnceLock<Mutex<HashMap<String, Vec<Instant>>>> = OnceLock::new();
    WINDOW.get_or_init(|| Mutex::new(HashMap::new()))
}

fn within_rate(card: &str) -> bool {
    let mut window = window().lock_or_recover();
    window.retain(|_, at| {
        at.retain(|t| t.elapsed() < Duration::from_secs(60));
        !at.is_empty()
    });
    let seen = window.entry(card.to_string()).or_default();
    if seen.len() >= NOTES_PER_MINUTE {
        return false;
    }
    seen.push(Instant::now());
    true
}

/// A title as a client may write it: one line, trimmed, no control
/// characters, at most `AGENT_NOTE_TITLE_MAX` characters.
fn clean_title(title: &str) -> Option<String> {
    let title = title.trim();
    (!title.is_empty()
        && title.chars().count() <= crate::documents::AGENT_NOTE_TITLE_MAX
        && !title.chars().any(char::is_control))
    .then(|| title.to_string())
}

/// The scratchpad entry id of a client's note `key`: the same for every
/// revision, never another client's.
fn entry_id(client_id: &str, key: &str) -> String {
    let digest = sha(format!("deck-agent-note-v1\0{client_id}\0{key}").as_bytes());
    let hex: String = digest
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(24)
        .collect();
    format!("A{hex}")
}

/// `deck_card_note`.
pub(super) fn card_note(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let args: NoteArgs = parse(arguments)?;
    let invalid = |message: &str| {
        error_value(
            "INVALID_ARGUMENTS",
            message,
            "Send a fresh request_id and a key (letters, digits, - and _), then a title of one line (at most 120 characters) and optionally text (at most 8 KiB) and acted; or withdraw: true alone to take an open note back.",
        )
    };
    if !valid_id(&args.request_id) || !valid_id(&args.key) {
        return Err(invalid("request_id or key is out of shape"));
    }
    let title =
        if args.withdraw {
            if args.title.is_some() || args.text.is_some() || args.acted {
                return Err(invalid("withdraw takes no title, text or acted"));
            }
            None
        } else {
            Some(args.title.as_deref().and_then(clean_title).ok_or_else(|| {
                invalid("a note needs a title of one line of at most 120 characters")
            })?)
        };
    let text = args.text.as_deref().map(str::trim).unwrap_or_default();
    if text.len() > crate::documents::AGENT_NOTE_TEXT_MAX_BYTES || text.contains('\0') {
        return Err(invalid("text is at most 8192 bytes"));
    }
    let hash = request_hash("deck_card_note", &args);
    let replay = runtime
        .read(|doc| -> Result<Option<Operation>, DeckError> {
            client(doc, client_id)?;
            Ok(existing_operation(doc, client_id, &args.request_id, &hash)?.cloned())
        })
        .map_err(map_error)?
        .map_err(map_error)?;
    if let Some(operation) = replay {
        return Ok(operation_view(&operation));
    }
    let Some(card) = super::origin::orchestrator() else {
        return Err(not_in_a_card());
    };
    if !crate::documents::board_is_current() {
        return Err(super::cards::source_unavailable());
    }
    let notes = crate::documents::card_agent_notes(&card).ok_or_else(card_gone)?;
    let own = notes
        .iter()
        .find(|n| n.client_id == client_id && n.key == args.key);
    if own.is_some_and(|n| n.handled) {
        return Err(error_value(
            "NOTE_HANDLED",
            "the user already handled the note with this key",
            "It is closed to you now; write a new note with a new key if something new needs the user.",
        ));
    }
    if args.withdraw && own.is_none() {
        return Ok(json!({"ok": true, "state": "committed", "unchanged": true}));
    }
    let open = notes.iter().filter(|n| !n.handled).count();
    if own.is_none() && open >= crate::documents::AGENT_NOTES_OPEN_MAX {
        return Err(error_value(
            "CAPACITY_EXCEEDED",
            "this card already holds 32 notes the user has not handled",
            "Revise or withdraw one of your open notes (deck_card_notes lists them) instead of adding another.",
        ));
    }
    if !within_rate(&card) {
        return Err(error_value(
            "RATE_LIMITED",
            "this card wrote twelve notes in the last minute",
            "Leave a note only for what needs the user; wait a minute before the next one.",
        ));
    }
    let operation = runtime
        .write(|doc| -> Result<Operation, DeckError> {
            if let Some(existing) = existing_operation(doc, client_id, &args.request_id, &hash)? {
                return Ok(existing.clone());
            }
            let name = client(doc, client_id)?.name.clone();
            reserve_operation(runtime, doc, client_id, Slot::Ordinary)?;
            let mut plan = json!({
                "cardId": card,
                "entryId": entry_id(client_id, &args.key),
                "key": args.key,
                "clientId": client_id,
                "client": name,
                "withdraw": args.withdraw,
            });
            if let Some(title) = &title {
                plan["title"] = json!(title);
                plan["text"] = json!(text);
                plan["acted"] = json!(args.acted);
            }
            let operation = Operation {
                result: Some(plan),
                ..Operation::new(
                    random_id("op_", 16)?,
                    client_id,
                    args.request_id.clone(),
                    hash.clone(),
                    CARD_NOTE_KIND,
                    "accepted",
                )
            };
            doc.operations.push(operation.clone());
            Ok(operation)
        })
        .map_err(map_error)?;
    emit_changed(runtime);
    crate::applog::applog("[mcp] card note accepted");
    let mut view = operation_view(&operation);
    view["nextAction"] = json!("Deck writes the note into your card's scratchpad; deck_operation_get with this operationId says committed once it is there (rejected with a code otherwise).");
    Ok(view)
}

/// `deck_card_notes`.
pub(super) fn card_notes(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let _: NotesArgs = parse(arguments)?;
    runtime
        .read(|doc| client(doc, client_id).map(|_| ()))
        .map_err(map_error)?
        .map_err(map_error)?;
    let Some(card) = super::origin::orchestrator() else {
        return Err(not_in_a_card());
    };
    if !crate::documents::board_is_current() {
        return Err(super::cards::source_unavailable());
    }
    let notes = crate::documents::card_agent_notes(&card).ok_or_else(card_gone)?;
    Ok(json!({
        "ok": true,
        "cardId": card,
        "notes": notes
            .iter()
            .filter(|n| n.client_id == client_id)
            .map(|n| json!({
                "key": n.key,
                "title": n.title,
                "acted": n.acted,
                "state": if n.handled { "handled" } else { "open" },
                "updatedAt": n.updated_at,
            }))
            .collect::<Vec<_>>(),
        "note": NOTE,
    }))
}
