//! Project list/read/search over `mcp_fs` with cursor paging and re-checked roots.
//!
//! Split out of the one-file `mcp.rs` on 2026-09-23; the contract stays in
//! `mcp/mod.rs`.

use super::*;

pub(super) fn read_root(
    runtime: &Runtime,
    client_id: &str,
    project_id: &str,
    root_index: usize,
) -> Result<(PathBuf, Vec<String>), Value> {
    runtime
        .read(|doc| {
            let project = scoped_project(client(doc, client_id)?, project_id)?;
            let root = project
                .roots
                .get(root_index)
                .ok_or_else(|| DeckError::new(ErrorKind::Perm, "read root is not authorized"))?;
            Ok((PathBuf::from(root), project.roots.clone()))
        })
        .map_err(map_error)?
        .map_err(map_error)
}

pub(super) fn recheck_read(
    runtime: &Runtime,
    client_id: &str,
    project_id: &str,
    roots: &[String],
) -> Result<(), Value> {
    runtime
        .read(|doc| {
            let project = scoped_project(client(doc, client_id)?, project_id)?;
            if project.roots != roots {
                return Err(DeckError::new(
                    ErrorKind::ContextChanged,
                    "project read authorization changed during the request",
                ));
            }
            Ok(())
        })
        .map_err(map_error)?
        .map_err(map_error)
}

pub(super) fn page_cursor(
    runtime: &Runtime,
    client_id: &str,
    project_id: &str,
    root_index: usize,
    query: &str,
    snapshot: &str,
    offset: usize,
) -> String {
    let payload = format!(
        "{}\0{client_id}\0{project_id}\0{root_index}\0{query}\0{snapshot}\0{offset}",
        runtime.service_instance
    );
    format!("{offset}.{}", sha(payload.as_bytes()))
}

pub(super) fn cursor_offset(
    runtime: &Runtime,
    cursor: Option<&str>,
    client_id: &str,
    project_id: &str,
    root_index: usize,
    query: &str,
    snapshot: &str,
) -> Result<usize, Value> {
    let Some(cursor) = cursor else { return Ok(0) };
    let (offset, _) = cursor.split_once('.').ok_or_else(|| {
        error_value(
            "OUTPUT_CURSOR_INVALID",
            "read cursor is invalid",
            "Restart the read without a cursor.",
        )
    })?;
    let offset = offset.parse::<usize>().map_err(|_| {
        error_value(
            "OUTPUT_CURSOR_INVALID",
            "read cursor is invalid",
            "Restart the read without a cursor.",
        )
    })?;
    if page_cursor(
        runtime, client_id, project_id, root_index, query, snapshot, offset,
    ) != cursor
    {
        return Err(error_value(
            "CONTENT_CHANGED",
            "the target or authorization changed since the cursor was issued",
            "Restart the read or search from the beginning.",
        ));
    }
    Ok(offset)
}

/// One closed code per structured-read failure class. Absent and excluded
/// paths share READ_DENIED so a denial never reveals out-of-scope names.
pub(super) fn fs_error(error: crate::mcp_fs::FsError, denied_action: &str) -> Value {
    use crate::mcp_fs::FsErrorKind;
    let (code, next_action) = match error.kind {
        FsErrorKind::Invalid => (
            "INVALID_ARGUMENTS",
            "Use a relative path without traversal and arguments within the documented bounds.",
        ),
        FsErrorKind::Denied => ("READ_DENIED", denied_action),
        FsErrorKind::Limit => (
            "READ_LIMIT",
            "The target exceeds the structured read size bound; choose a smaller file.",
        ),
        FsErrorKind::Changed => (
            "CONTENT_CHANGED",
            "Restart the read or search without a cursor.",
        ),
        FsErrorKind::Cancelled => (
            "CONTEXT_CHANGED",
            "Call deck_capabilities to confirm the current project scope before retrying.",
        ),
    };
    error_value(code, error.message, next_action)
}

pub(super) fn project_list(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let args: ProjectPathArgs = parse(arguments)?;
    let (root, roots) = read_root(runtime, client_id, &args.project_id, args.root_index)?;
    let listing = crate::mcp_fs::list(&root, &args.path).map_err(|error| {
        fs_error(
            error,
            "Choose an existing, non-sensitive directory inside the approved root.",
        )
    })?;
    recheck_read(runtime, client_id, &args.project_id, &roots)?;
    Ok(
        json!({"ok":true,"projectId":args.project_id,"rootIndex":args.root_index,"path":args.path,"entries":listing.entries,"version":listing.version,"truncated":listing.truncated}),
    )
}

pub(super) fn project_read(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let args: FileReadArgs = parse(arguments)?;
    let (root, roots) = read_root(runtime, client_id, &args.project_id, args.root_index)?;
    // Read metadata/content once at offset zero to obtain the descriptor-bound
    // version. Subsequent cursor validation binds that version before slicing.
    let file_version = crate::mcp_fs::identity(&root, &args.path).map_err(|error| {
        fs_error(
            error,
            "Choose a regular UTF-8, non-sensitive file inside the approved root.",
        )
    })?;
    let snapshot = sha(&serde_json::to_vec(&file_version).unwrap_or_default());
    let offset = cursor_offset(
        runtime,
        args.cursor.as_deref(),
        client_id,
        &args.project_id,
        args.root_index,
        &args.path,
        &snapshot,
    )?;
    let (content, next, truncated, version) = crate::mcp_fs::read(
        &root,
        &args.path,
        offset as u64,
        args.max_bytes.unwrap_or(MAX_READ_BYTES),
    )
    .map_err(|error| {
        fs_error(
            error,
            "Choose a regular UTF-8, non-sensitive file inside the approved root.",
        )
    })?;
    if version != file_version {
        return Err(error_value(
            "CONTENT_CHANGED",
            "the file changed during the read",
            "Restart reading without a cursor.",
        ));
    }
    recheck_read(runtime, client_id, &args.project_id, &roots)?;
    let cursor = truncated.then(|| {
        page_cursor(
            runtime,
            client_id,
            &args.project_id,
            args.root_index,
            &args.path,
            &snapshot,
            next as usize,
        )
    });
    Ok(
        json!({"ok":true,"projectId":args.project_id,"rootIndex":args.root_index,"path":args.path,"content":content,"version":version,"nextCursor":cursor,"truncated":truncated}),
    )
}

pub(super) fn project_search(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let args: SearchArgs = parse(arguments)?;
    let limit = args.max_results.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "search result limit is invalid",
            "Use max_results from 1 through 100.",
        ));
    }
    let (root, roots) = read_root(runtime, client_id, &args.project_id, args.root_index)?;
    let deadline = Instant::now() + Duration::from_millis(750);
    let outcome = crate::mcp_fs::search_controlled(&root, &args.path, &args.query, || {
        if Instant::now() >= deadline {
            return crate::mcp_fs::SearchControl::Deadline;
        }
        let authorized = runtime
            .read(|doc| {
                scoped_project(client(doc, client_id)?, &args.project_id)
                    .map(|project| project.roots == roots)
            })
            .ok()
            .and_then(Result::ok)
            .unwrap_or(false);
        if authorized {
            crate::mcp_fs::SearchControl::Continue
        } else {
            crate::mcp_fs::SearchControl::Cancelled
        }
    })
    .map_err(|error| {
        fs_error(
            error,
            "Choose an existing, non-sensitive file or directory inside the approved root.",
        )
    })?;
    let results = outcome.matches;
    let search_complete = outcome.complete;
    let stop_reason = outcome.stop_reason;
    let skipped = outcome.skipped;
    let snapshot = sha(&serde_json::to_vec(&results).unwrap_or_default());
    let key = format!("{}\0{}", args.path, args.query);
    let offset = cursor_offset(
        runtime,
        args.cursor.as_deref(),
        client_id,
        &args.project_id,
        args.root_index,
        &key,
        &snapshot,
    )?;
    if offset > results.len() {
        return Err(error_value(
            "CONTENT_CHANGED",
            "search results changed",
            "Restart the search without a cursor.",
        ));
    }
    let mut end = offset.saturating_add(limit).min(results.len());
    recheck_read(runtime, client_id, &args.project_id, &roots)?;
    loop {
        let cursor = (search_complete && end < results.len()).then(|| {
            page_cursor(
                runtime,
                client_id,
                &args.project_id,
                args.root_index,
                &key,
                &snapshot,
                end,
            )
        });
        let response = json!({"ok":true,"projectId":args.project_id,"path":args.path,"query":args.query,"matches":&results[offset..end],"nextCursor":cursor,"truncated":!search_complete || end < results.len(),"complete":search_complete && end == results.len(),"stopReason":stop_reason,"skipped":skipped});
        if serde_json::to_vec(&response).is_ok_and(|bytes| bytes.len() < MAX_RESPONSE_BYTES) {
            return Ok(response);
        }
        if end == offset {
            return Err(error_value(
                "RESPONSE_TOO_LARGE",
                "one encoded search result exceeds the response budget",
                "Narrow the search path or query.",
            ));
        }
        end -= 1;
    }
}
