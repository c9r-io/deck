//! Request validation: applicability, payload shapes, admission board checks.
//!
//! Split out of the one-file `connector/mod.rs` on 2026-09-23; the contract
//! stays in `connector/mod.rs`.

use super::*;

pub(super) fn buffer_operation_id(handle: &str, entry_id: &str) -> String {
    let digest = Sha256::digest(format!("connector-buffer:{handle}:{entry_id}").as_bytes());
    format!("B{}", hex(&digest[..16]))
}

pub(super) fn validate_admission_board(
    handle: &str,
    request: &CommandRequest,
    board: &Value,
) -> Result<(), DeckError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Payload {
        entry_ids: Vec<String>,
    }
    if request.kind != "buffer-queue" {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "command is not a buffer admission",
        ));
    }
    let payload: Payload = serde_json::from_value(request.payload.clone())
        .map_err(|_| DeckError::new(ErrorKind::Invalid, "command payload is invalid"))?;
    let card_id = request
        .card_id
        .as_deref()
        .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "card is required"))?;
    let card = board
        .get("cards")
        .and_then(Value::as_array)
        .and_then(|cards| {
            cards
                .iter()
                .find(|card| card.get("id").and_then(Value::as_str) == Some(card_id))
        })
        .ok_or_else(|| DeckError::new(ErrorKind::ContextChanged, "buffer-copies-missing"))?;
    require_queue_target(card)?;
    let entries = card
        .get("buffer")
        .and_then(|buffer| buffer.get("entries"))
        .and_then(Value::as_array)
        .ok_or_else(|| DeckError::new(ErrorKind::ContextChanged, "buffer-copies-missing"))?;
    for entry_id in payload.entry_ids {
        let entry = entries
            .iter()
            .find(|entry| entry.get("id").and_then(Value::as_str) == Some(&entry_id))
            .ok_or_else(|| DeckError::new(ErrorKind::ContextChanged, "buffer-copies-missing"))?;
        let expected = buffer_operation_id(handle, &entry_id);
        let count = entry
            .get("copies")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|copy| copy.get("operationId").and_then(Value::as_str) == Some(&expected))
            .count();
        if count != 1 {
            return Err(DeckError::new(
                ErrorKind::ContextChanged,
                "buffer-copies-missing",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_applicable(request: &CommandRequest) -> Result<(), DeckError> {
    let (revision, board) = board_value()?;
    if request.kind == "task-create" {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct P {
            project_id: String,
            preset_id: String,
        }
        let p: P = serde_json::from_value(request.payload.clone())
            .map_err(|_| DeckError::new(ErrorKind::Invalid, "invalid task preset"))?;
        if request.expected_revision.as_deref() != Some(&revision) {
            return Err(DeckError::new(
                ErrorKind::ContextChanged,
                "revision-changed",
            ));
        }
        let project = board
            .get("projects")
            .and_then(Value::as_array)
            .and_then(|a| {
                a.iter()
                    .find(|x| x.get("id").and_then(Value::as_str) == Some(&p.project_id))
            })
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "task project not found"))?;
        let presets = project
            .get("presets")
            .and_then(Value::as_array)
            .filter(|a| a.len() <= 50)
            .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "task presets are invalid"))?;
        let preset = presets
            .iter()
            .find(|x| x.get("id").and_then(Value::as_str) == Some(&p.preset_id))
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "task preset not found"))?;
        let bounded = |field: &str, max: usize| {
            preset
                .get(field)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty() && v.len() <= max && !v.chars().any(char::is_control))
        };
        let column_id = bounded("columnId", 128)
            .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "task preset is invalid"))?;
        if bounded("id", 128).is_none()
            || bounded("name", 120).is_none()
            || bounded("title", 120).is_none()
            || bounded("dir", 1024).is_none()
            || project
                .get("columns")
                .and_then(Value::as_array)
                .is_none_or(|columns| {
                    !columns
                        .iter()
                        .any(|c| c.get("id").and_then(Value::as_str) == Some(column_id))
                })
        {
            return Err(DeckError::new(ErrorKind::Invalid, "task preset is invalid"));
        }
        let command = bounded("cmd", 200).and_then(crate::admission::channel_agent_command);
        if command.is_none() {
            return Err(DeckError::new(
                ErrorKind::Invalid,
                "task preset command is not supported",
            ));
        }
        let steps = preset
            .get("steps")
            .and_then(Value::as_array)
            .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "task preset steps are invalid"))?;
        if steps.len() > 20
            || steps
                .iter()
                .any(|x| x.as_str().is_none_or(|s| s.is_empty() || s.len() > 2000))
        {
            return Err(DeckError::new(
                ErrorKind::Invalid,
                "task preset steps are invalid",
            ));
        }
        return Ok(());
    }
    if request.kind.starts_with("buffer-") {
        let id = request
            .card_id
            .as_deref()
            .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "card is required"))?;
        let card = board
            .get("cards")
            .and_then(Value::as_array)
            .and_then(|a| {
                a.iter()
                    .find(|c| c.get("id").and_then(Value::as_str) == Some(id))
            })
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "card not found"))?;
        validate_buffer_target(request, card)?;
    }
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CommandResult {
    pub(super) id: String,
    pub(super) state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) result: Option<Value>,
}

pub(super) fn validate_command(r: &CommandRequest) -> Result<(), DeckError> {
    if r.id.is_empty() || r.id.len() > 128 || r.id.chars().any(|c| c.is_control()) {
        return Err(DeckError::new(ErrorKind::Invalid, "invalid command id"));
    }
    if r.seq == Some(0) {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "invalid command sequence",
        ));
    }
    if !matches!(
        r.kind.as_str(),
        "send-message"
            | "buffer-add"
            | "buffer-edit"
            | "buffer-delete"
            | "buffer-queue"
            | "task-create"
            | "queue-pause"
            | "queue-cancel"
    ) {
        return Err(DeckError::new(ErrorKind::Invalid, "unknown command kind"));
    }
    if r.kind != "task-create"
        && r.card_id
            .as_ref()
            .is_none_or(|v| v.is_empty() || v.len() > 128)
    {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "command card is invalid",
        ));
    }
    if r.kind == "send-message"
        && !matches!(
            &r.expected_generation,
            ExpectedGeneration::Live(v)
                if v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit())
        )
    {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "expected generation is required",
        ));
    }
    if matches!(r.kind.as_str(), "queue-pause" | "queue-cancel") {
        match &r.expected_generation {
            ExpectedGeneration::Stopped => {}
            ExpectedGeneration::Live(v)
                if v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()) => {}
            _ => {
                return Err(DeckError::new(
                    ErrorKind::Invalid,
                    "expected generation is required",
                ));
            }
        }
    }
    if matches!(
        r.kind.as_str(),
        "buffer-add" | "buffer-edit" | "buffer-delete" | "buffer-queue" | "task-create"
    ) && r
        .expected_revision
        .as_ref()
        .is_none_or(|v| v.is_empty() || v.len() > 128)
    {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "expected revision is required",
        ));
    }
    validate_command_payload(r)?;
    Ok(())
}

pub(super) fn command_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}

pub(super) fn command_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TEXT
        && !value
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
}

pub(super) fn validate_command_payload(r: &CommandRequest) -> Result<(), DeckError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Text {
        text: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Edit {
        entry_id: String,
        text: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Entry {
        entry_id: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Entries {
        entry_ids: Vec<String>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Task {
        project_id: String,
        preset_id: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Queue {
        item_id: String,
        #[serde(default)]
        paused: Option<bool>,
        revision: String,
    }

    let valid = match r.kind.as_str() {
        "send-message" | "buffer-add" => {
            serde_json::from_value::<Text>(r.payload.clone()).is_ok_and(|p| command_text(&p.text))
        }
        "buffer-edit" => serde_json::from_value::<Edit>(r.payload.clone())
            .is_ok_and(|p| command_id(&p.entry_id) && command_text(&p.text)),
        "buffer-delete" => serde_json::from_value::<Entry>(r.payload.clone())
            .is_ok_and(|p| command_id(&p.entry_id)),
        "buffer-queue" => serde_json::from_value::<Entries>(r.payload.clone()).is_ok_and(|p| {
            let unique = p.entry_ids.iter().collect::<HashSet<_>>().len() == p.entry_ids.len();
            !p.entry_ids.is_empty()
                && p.entry_ids.len() <= 256
                && unique
                && p.entry_ids.iter().all(|id| command_id(id))
        }),
        "task-create" => serde_json::from_value::<Task>(r.payload.clone())
            .is_ok_and(|p| command_id(&p.project_id) && command_id(&p.preset_id)),
        "queue-pause" | "queue-cancel" => serde_json::from_value::<Queue>(r.payload.clone())
            .is_ok_and(|p| {
                command_id(&p.item_id)
                    && !p.revision.is_empty()
                    && p.revision.len() <= 32
                    && p.revision.bytes().all(|b| b.is_ascii_digit())
                    && if r.kind == "queue-pause" {
                        p.paused.is_some()
                    } else {
                        p.paused.is_none()
                    }
            }),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(DeckError::new(
            ErrorKind::Invalid,
            "command payload is invalid",
        ))
    }
}
