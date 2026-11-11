//! Drafted configuration: a client the user lets draft (`allow_drafts`)
//! may PROPOSE a prompt template and one Agent handover rule of a project it
//! is authorized for. A proposal is a draft and nothing more: it lives in
//! the ledger, Deck shows it under Needs attention, and only the user's own
//! click there writes it — through the webview's ordinary Board transaction
//! and settings writer, the same writes the template manager and the
//! automations editor make. Every approval a rule then carries is the one
//! the user signs in that click (`automation-model.js` `approveRule`).
//!
//! # Contract
//! - Three tools. `deck_config_describe` (read) explains the fields, gives
//!   copyable examples and lists the client's projects with their columns,
//!   templates (name and structure, never the steps' text) and Agent rules.
//!   `deck_config_propose` validates a draft (`validate_only` checks and
//!   previews, writes nothing) or records it as pending; every problem comes
//!   back as `{field, code, message, fix}`, so what reaches the user can be
//!   saved. `deck_config_proposal_get` reads one of the client's proposals.
//! - A client never approves, applies, deletes or reads its proposals'
//!   effect anywhere but here. No proposal state is authority: a decision
//!   is bookkeeping the webview records after it wrote what the user chose
//!   (`mcp_proposal_decide`), so the client learns the outcome.
//! - Scope (D4): templates and Agent rules (`source: agent`) only — no
//!   Slack badge, clock or channel rule, no other setting. A rule's command
//!   passes the remote admission, Codex only with `--no-daemon`; its
//!   directory is inside the client's authorized roots of that project; its
//!   template is the drafted one or one the project has, and no step may
//!   begin with a `{{msg.*}}` placeholder (the channel admission's rule).
//!   A draft may name an existing template or rule: the user then sees what
//!   changes, and the approval covers the new version only.
//! - Bounded: at most `MAX_PENDING` pending proposals per client, the
//!   newest `MAX_KEPT` per client kept; an identical pending draft is not
//!   recorded twice; a request id is idempotent (same draft → the same
//!   proposal, another draft → `REQUEST_ID_REUSED`).
//! - Withdrawing the client's draft choice, or revoking the client, turns
//!   its pending proposals `withdrawn`; the user can no longer approve them.
//! - A ledger that holds a proposal or the draft choice is written at
//!   `STATE_VERSION_PROPOSALS` (sticky), so an older Deck refuses it.

use super::*;

pub(super) const MAX_PENDING: usize = 4;
pub(super) const MAX_KEPT: usize = 16;
const MAX_NOTE_CHARS: usize = 300;
const TEMPLATE_NAME_MAX_CHARS: usize = 120;
const TEMPLATE_STEPS_MAX: usize = 20;
const TEMPLATES_MAX: usize = 50;
const FOLLOW_UPS_MAX: usize = 3;
const RULE_NAME_MAX_CHARS: usize = 120;
const RULE_CMD_MAX_CHARS: usize = 200;

pub(super) const PENDING: &str = "pending";
pub(super) const STATUSES: [&str; 5] = [PENDING, "approved", "edited", "rejected", "withdrawn"];
/// The closed refusal of a decision on a proposal that is no longer pending.
pub(super) const PROPOSAL_DECIDED: &str = "mcp-proposal-decided";

/// A drafted prompt template: the steps one run sends in order and its
/// fixed follow-up replies.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct TemplateDraft {
    pub(super) name: String,
    pub(super) steps: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) follow_ups: Vec<String>,
}

/// A drafted Agent handover rule (`source: agent`).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct RuleDraft {
    pub(super) automation_id: String,
    #[serde(default)]
    pub(super) name: String,
    pub(super) column_id: String,
    #[serde(default)]
    pub(super) dir: String,
    pub(super) cmd: String,
    pub(super) template: String,
    #[serde(default = "default_finish")]
    pub(super) finish: String,
    #[serde(default)]
    pub(super) review_each: bool,
    /// Ask the user to approve sending steps 2..N automatically.
    #[serde(default)]
    pub(super) auto_continue: bool,
    /// With `auto_continue`: also the steps carrying the handed-over text.
    #[serde(default)]
    pub(super) external: bool,
    /// Ask the user to let step 1 go to a freshly started agent at once.
    #[serde(default)]
    pub(super) first_send: bool,
}

fn default_finish() -> String {
    "keep".into()
}

/// One recorded draft and what became of it.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Proposal {
    pub(super) id: String,
    pub(super) client_id: String,
    pub(super) request_id: String,
    pub(super) request_hash: String,
    /// The draft alone (without request id and note): an identical pending
    /// draft is not recorded twice.
    pub(super) content_hash: String,
    pub(super) project_id: String,
    #[serde(default)]
    pub(super) note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) template: Option<TemplateDraft>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) rule: Option<RuleDraft>,
    pub(super) created_at: u64,
    pub(super) status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) decided_at: Option<u64>,
    /// The id of the rule the user's decision saved (the drawer labels it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) rule_id: Option<String>,
}

/// `deck_config_propose`.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProposeArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) request_id: Option<String>,
    pub(super) project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) template: Option<TemplateDraft>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) rule: Option<RuleDraft>,
    #[serde(default)]
    pub(super) validate_only: bool,
}

/// `deck_config_proposal_get`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProposalGetArgs {
    pub(super) proposal_id: String,
}

pub(super) fn uses_proposals(doc: &DiskDoc) -> bool {
    !doc.proposals.is_empty() || doc.config.clients.iter().any(|c| c.allow_drafts)
}

pub(super) fn valid_proposals(doc: &DiskDoc) -> bool {
    let mut ids = HashSet::new();
    doc.proposals.len() <= MAX_CLIENTS * MAX_KEPT
        && doc.proposals.iter().all(|p| {
            valid_id(&p.id)
                && ids.insert(p.id.clone())
                && valid_id(&p.client_id)
                && valid_id(&p.request_id)
                && valid_id(&p.project_id)
                && p.request_hash.len() == 64
                && p.content_hash.len() == 64
                && STATUSES.contains(&p.status.as_str())
                && (p.template.is_some() || p.rule.is_some())
        })
}

fn drafting<'a>(doc: &'a DiskDoc, client_id: &str) -> Result<&'a Client, DeckError> {
    let client = client(doc, client_id)?;
    if !client.allow_drafts {
        return Err(DeckError::new(
            ErrorKind::Perm,
            "client may not draft configuration",
        ));
    }
    Ok(client)
}

/// The committed Board (`None` while it cannot answer).
fn board() -> Option<Value> {
    crate::documents::board_authority()
}

fn unavailable() -> Value {
    error_value(
        "CONFIG_UNAVAILABLE",
        "Deck cannot read the Board or its settings right now",
        "Retry later; if it persists, ask the local Deck user to check the Board.",
    )
}

fn board_project<'a>(board: &'a Value, project_id: &str) -> Option<&'a Value> {
    board["projects"]
        .as_array()?
        .iter()
        .find(|p| p["id"].as_str() == Some(project_id))
}

fn columns(project: &Value) -> Vec<Value> {
    project["columns"]
        .as_array()
        .map(|columns| {
            columns
                .iter()
                .map(|c| json!({"id": c["id"], "name": c["name"]}))
                .collect()
        })
        .unwrap_or_default()
}

fn templates(project: &Value) -> Vec<Value> {
    project["templates"].as_array().cloned().unwrap_or_default()
}

fn template_steps(template: &Value) -> Vec<String> {
    template["steps"]
        .as_array()
        .map(|steps| {
            steps
                .iter()
                .map(|s| s.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn step_class(step: &str) -> &'static str {
    if crate::scheduler::authority::has_placeholder(step) {
        "bounded"
    } else {
        "fixed"
    }
}

/// The channel admission's rule (channel-model.js `LEADING_MESSAGE`): a
/// step that begins with a message placeholder lets the handed-over text
/// lead the prompt.
fn leads_with_message(step: &str) -> bool {
    crate::scheduler::authority::placeholder_at(step.trim_start()).is_some()
}

/// `{{msg.<name>}}` placeholders that are not one of the four known names:
/// they stay literal text, which is never what a draft means.
fn unknown_placeholders(step: &str) -> Vec<String> {
    step.char_indices()
        .filter_map(|(i, _)| crate::scheduler::authority::placeholder_at(&step[i..]))
        .map(|(name, _)| name)
        .filter(|name| !crate::scheduler::authority::INBOUND_PLACEHOLDERS.contains(name))
        .map(str::to_owned)
        .collect()
}

fn rules_of(config: &crate::inbound::Config) -> &[crate::inbound::Rule] {
    &config.rules
}

fn agent_rule<'a>(
    config: &'a crate::inbound::Config,
    automation_id: &str,
) -> Option<&'a crate::inbound::Rule> {
    rules_of(config)
        .iter()
        .find(|r| r.source == crate::inbound_agent::SOURCE && r.badge == automation_id)
}

fn problem(field: &str, code: &str, message: &str, fix: &str) -> Value {
    json!({"field": field, "code": code, "message": message, "fix": fix})
}

/* ---------- describe ---------- */

const WORKFLOW: [&str; 6] = [
    "1. Read this description; reuse an existing template or rule when one fits.",
    "2. Call deck_config_propose with validate_only: true and fix every problem it returns (each names the field and how to fix it).",
    "3. Submit the same draft without validate_only and a fresh request_id. Submit one proposal per change; never resubmit an identical draft.",
    "4. Tell the user what you drafted: it waits in Deck's Needs attention until they approve, open it in the editor, or reject it.",
    "5. Poll deck_config_proposal_get (seldom: the user decides when they come back). Nothing you drafted works before it is approved.",
    "6. Once approved, deck_automation_list lists the rule; trigger it with deck_automation_trigger (needs the user's separate automation permission).",
];

fn fields_help() -> Value {
    json!({
        "template": {
            "name": format!("1–{TEMPLATE_NAME_MAX_CHARS} characters, unique in the project; a draft that names an existing template replaces its steps."),
            "steps": format!("1–{TEMPLATE_STEPS_MAX} prompts typed into the card's agent in order, one per turn, each at most 2000 characters (multi-line allowed). Placeholders {{{{msg.text}}}}, {{{{msg.from}}}}, {{{{msg.where}}}}, {{{{msg.link}}}} are filled with what deck_automation_trigger hands over, fenced as external content. A step must not BEGIN with a placeholder: write 'Task: {{{{msg.text}}}}', not '{{{{msg.text}}}}'."),
            "follow_ups": format!("optional, at most {FOLLOW_UPS_MAX} fixed replies shown as buttons on the card (no placeholders).")
        },
        "rule": {
            "automation_id": "the id you will pass to deck_automation_trigger: lowercase letters, digits, _ + - (at most 64). An existing Agent rule's id proposes a change to that rule.",
            "name": format!("display name, at most {RULE_NAME_MAX_CHARS} characters"),
            "column_id": "a column id of the project (listed below)",
            "dir": "the card's working directory: inside your authorized directory; the authorized directory itself when empty",
            "cmd": "the agent the card starts: `claude` or `codex --no-daemon`, optionally with simple flags (letters, digits and -_./:=+,@ only)",
            "template": "the drafted template's name, or one the project already has",
            "finish": "keep (default) or close: close retires the card once its steps are sent and the agent exited",
            "review_each": "true: every step waits for the user's check before it is sent",
            "auto_continue": "ask the user to approve sending steps 2..N automatically after each turn ends; otherwise the user sends each step",
            "external": "with auto_continue and placeholder steps: also ask to approve steps carrying the handed-over text",
            "first_send": "ask the user to let step 1 go to the freshly started agent at once, without waiting for its first interaction (risk: a startup dialog may take it)"
        }
    })
}

fn examples() -> Value {
    json!([
        {
            "purpose": "A one-step worker automation the orchestrator triggers with a task",
            "draft": {
                "template": {"name": "Worker task", "steps": ["Task: {{msg.text}}\n\nWork in this directory. When done, summarize what changed in one paragraph."]},
                "rule": {"automation_id": "worker", "name": "Worker", "column_id": "<a column id>", "cmd": "claude", "template": "Worker task", "auto_continue": false, "first_send": true}
            }
        },
        {
            "purpose": "A three-step code review automation that continues by itself",
            "draft": {
                "template": {"name": "Code review", "steps": [
                    "Review request: {{msg.text}} ({{msg.link}}). Read the pull request and its diff.",
                    "Run the project's tests and linters; report failures with file and line.",
                    "Write the review as Markdown in review.md: blocking issues first, then suggestions."
                ], "follow_ups": ["Post nothing; wait for me."]},
                "rule": {"automation_id": "code-review", "name": "Code review", "column_id": "<a column id>", "cmd": "claude", "template": "Code review", "finish": "keep", "auto_continue": true, "external": true}
            }
        },
        {
            "purpose": "An image job on a Codex worker (results as files: Codex forwards no final message)",
            "draft": {
                "template": {"name": "Icons", "steps": ["Generate the images described here and save them as PNG files under ./assets: {{msg.text}}"]},
                "rule": {"automation_id": "icons", "name": "Icons", "column_id": "<a column id>", "cmd": "codex --no-daemon", "template": "Icons"}
            }
        }
    ])
}

pub(super) fn config_describe(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    parse::<Empty>(arguments)?;
    let (client, pending) = runtime
        .read(|doc| {
            let client = drafting(doc, client_id)?.clone();
            let pending: Vec<Value> = doc
                .proposals
                .iter()
                .filter(|p| p.client_id == client_id && p.status == PENDING)
                .map(proposal_view)
                .collect();
            Ok((client, pending))
        })
        .map_err(map_error)?
        .map_err(map_error)?;
    let (Some(board), Some(config)) = (board(), super::automation::automation_config()) else {
        return Err(unavailable());
    };
    let projects: Vec<Value> = client_scopes(&client)
        .iter()
        .filter_map(|scope| {
            let project = board_project(&board, &scope.project_id)?;
            let templates: Vec<Value> = templates(project)
                .iter()
                .map(|t| {
                    let steps = template_steps(t);
                    json!({
                        "name": t["name"],
                        "steps": steps.len(),
                        "classes": steps.iter().map(|s| step_class(s)).collect::<Vec<_>>(),
                        "followUps": t["followUps"].as_array().map_or(0, Vec::len),
                    })
                })
                .collect();
            let rules: Vec<Value> = rules_of(&config)
                .iter()
                .filter(|r| {
                    r.source == crate::inbound_agent::SOURCE && r.project_id == scope.project_id
                })
                .map(|r| {
                    json!({
                        "automation_id": r.badge,
                        "name": r.name,
                        "column_id": r.column_id,
                        "dir": r.dir,
                        "cmd": r.cmd,
                        "template": r.template,
                        "finish": if r.finish == "close" { "close" } else { "keep" },
                        "review_each": r.review_each,
                        "auto_continue": r.auto_send.is_some(),
                        "first_send": r.first_send_without_readiness,
                        "enabled": r.enabled,
                    })
                })
                .collect();
            Some(json!({
                "project_id": scope.project_id,
                "name": project["name"],
                "authorizedDirectories": scope.roots,
                "columns": columns(project),
                "templates": templates,
                "agentRules": rules,
            }))
        })
        .collect();
    Ok(json!({
        "ok": true,
        "workflow": WORKFLOW,
        "scope": "Templates and Agent handover rules (triggered by deck_automation_trigger) only. You draft; the local user approves in Deck. You cannot approve, delete or change anything yourself.",
        "fields": fields_help(),
        "examples": examples(),
        "projects": projects,
        "pendingProposals": pending,
        "limits": {"pendingPerClient": MAX_PENDING, "templateSteps": TEMPLATE_STEPS_MAX, "followUps": FOLLOW_UPS_MAX}
    }))
}

/* ---------- validation and preview ---------- */

/// What the draft would save, checked against the Board, the settings and
/// the client's scope: the problems, and the normalized draft with its
/// preview when there are none.
struct Checked {
    problems: Vec<Value>,
    template: Option<TemplateDraft>,
    rule: Option<RuleDraft>,
    preview: Value,
}

fn check(
    client: &Client,
    args: &ProposeArgs,
    board: &Value,
    config: &crate::inbound::Config,
) -> Result<Checked, Value> {
    let scope = scoped_project(client, &args.project_id).map_err(|error| {
        if error.message() == NO_PROJECT_DIRECTORY {
            error_value(
                "PERMISSION_DENIED",
                NO_PROJECT_DIRECTORY,
                "Tell the local Deck user; Deck works in a project only through its default directory.",
            )
        } else {
            error_value(
                "PERMISSION_DENIED",
                "this client is not authorized for that project",
                "Use a project_id that deck_config_describe lists.",
            )
        }
    })?;
    let scope = &scope;
    let project = board_project(board, &args.project_id).ok_or_else(|| {
        error_value(
            "PROJECT_MISSING",
            "the project is not on the Board",
            "Use a project_id that deck_config_describe lists.",
        )
    })?;
    let mut problems = Vec::new();
    if args.template.is_none() && args.rule.is_none() {
        problems.push(problem(
            "",
            "EMPTY_DRAFT",
            "a draft needs a template, a rule or both",
            "Add `template` and/or `rule`.",
        ));
    }
    if args
        .note
        .as_ref()
        .is_some_and(|n| n.chars().count() > MAX_NOTE_CHARS || n.chars().any(char::is_control))
    {
        problems.push(problem(
            "note",
            "NOTE_INVALID",
            "the note is too long or has control characters",
            "Keep the note to one line of at most 300 characters.",
        ));
    }
    let existing_templates = templates(project);
    let find_template = |name: &str| {
        existing_templates
            .iter()
            .find(|t| t["name"].as_str() == Some(name))
    };
    let template = args.template.as_ref().map(|t| TemplateDraft {
        name: t.name.trim().to_string(),
        steps: t
            .steps
            .iter()
            .map(|s| crate::scheduler::authority::normalize_template_step(s))
            .collect(),
        follow_ups: t
            .follow_ups
            .iter()
            .map(|s| crate::scheduler::authority::normalize_template_step(s))
            .collect(),
    });
    if let (Some(raw), Some(t)) = (args.template.as_ref(), template.as_ref()) {
        if t.name.is_empty()
            || t.name.chars().count() > TEMPLATE_NAME_MAX_CHARS
            || t.name.chars().any(char::is_control)
        {
            problems.push(problem(
                "template.name",
                "TEMPLATE_NAME_INVALID",
                "the template name is empty, too long or has control characters",
                "Use a short name of 1–120 characters on one line.",
            ));
        }
        if find_template(&t.name).is_none() && existing_templates.len() >= TEMPLATES_MAX {
            problems.push(problem(
                "template.name",
                "TEMPLATES_FULL",
                "the project already has 50 templates",
                "Reuse or change an existing template (name it) instead of adding one.",
            ));
        }
        if t.steps.is_empty() || t.steps.len() > TEMPLATE_STEPS_MAX {
            problems.push(problem(
                "template.steps",
                "TEMPLATE_STEPS_COUNT",
                "a template has 1–20 steps",
                "Merge or drop steps so there are 1–20.",
            ));
        }
        for (i, (raw_step, step)) in raw.steps.iter().zip(&t.steps).enumerate() {
            let field = format!("template.steps[{i}]");
            if step.is_empty() {
                problems.push(problem(
                    &field,
                    "STEP_EMPTY",
                    "the step is empty",
                    "Write the prompt or remove the step.",
                ));
            } else if raw_step.trim().chars().count()
                > crate::scheduler::authority::TEMPLATE_STEP_MAX
            {
                problems.push(problem(
                    &field,
                    "STEP_TOO_LONG",
                    "the step is longer than 2000 characters",
                    "Shorten it, or ask the agent to read longer instructions from a file.",
                ));
            }
            if leads_with_message(step) {
                problems.push(problem(
                    &field,
                    "STEP_LEADS_WITH_MESSAGE",
                    "a step may not begin with a {{msg.*}} placeholder",
                    "Put your own words first, e.g. \"Task: {{msg.text}}\".",
                ));
            }
            let unknown = unknown_placeholders(step);
            if !unknown.is_empty() {
                problems.push(problem(
                    &field,
                    "PLACEHOLDER_UNKNOWN",
                    &format!("unknown placeholder msg.{}", unknown.join(", msg.")),
                    "Use only {{msg.text}}, {{msg.from}}, {{msg.where}} or {{msg.link}}.",
                ));
            }
        }
        if t.follow_ups.len() > FOLLOW_UPS_MAX
            || t.follow_ups
                .iter()
                .any(|f| f.is_empty() || (f.contains("{{") && f.contains("msg.")))
        {
            problems.push(problem(
                "template.follow_ups",
                "FOLLOW_UPS_INVALID",
                "at most 3 non-empty follow-ups, without placeholders",
                "Keep up to three fixed replies; nothing fills a placeholder in a follow-up.",
            ));
        }
    }
    let rule = args.rule.as_ref().map(|raw| {
        check_rule(
            raw,
            scope,
            project,
            config,
            template.as_ref(),
            &find_template,
            &mut problems,
        )
    });
    let preview = preview(
        project,
        template.as_ref(),
        rule.as_ref(),
        &find_template,
        config,
    );
    Ok(Checked {
        problems,
        template,
        rule,
        preview,
    })
}

fn check_rule<'a>(
    raw: &RuleDraft,
    scope: &ProjectScope,
    project: &Value,
    config: &crate::inbound::Config,
    template: Option<&TemplateDraft>,
    find_template: &dyn Fn(&str) -> Option<&'a Value>,
    problems: &mut Vec<Value>,
) -> RuleDraft {
    let mut rule = raw.clone();
    rule.name = rule.name.trim().to_string();
    rule.cmd = rule.cmd.trim().to_string();
    rule.template = rule.template.trim().to_string();
    rule.external = rule.auto_continue && rule.external;
    if !crate::inbound::valid_badge(&rule.automation_id) {
        problems.push(problem(
            "rule.automation_id",
            "AUTOMATION_ID_INVALID",
            "the automation id is not lowercase letters, digits, _ + - (1–64)",
            "Use an id like `code-review`.",
        ));
    }
    let existing = agent_rule(config, &rule.automation_id);
    if existing.is_some_and(|r| r.project_id != scope.project_id) {
        problems.push(problem(
            "rule.automation_id",
            "AUTOMATION_ID_TAKEN",
            "another project's Agent rule has this id",
            "Choose another automation id.",
        ));
    }
    if existing.is_none() && rules_of(config).len() >= crate::inbound::MAX_RULES {
        problems.push(problem(
            "rule",
            "RULES_FULL",
            "Deck already holds 32 automations",
            "Propose a change to an existing Agent rule instead.",
        ));
    }
    if rule.name.chars().count() > RULE_NAME_MAX_CHARS || rule.name.chars().any(char::is_control) {
        problems.push(problem(
            "rule.name",
            "RULE_NAME_INVALID",
            "the name is too long or has control characters",
            "Use at most 120 characters on one line.",
        ));
    }
    let column_ids: Vec<String> = columns(project)
        .iter()
        .filter_map(|c| c["id"].as_str().map(str::to_owned))
        .collect();
    if !column_ids.contains(&rule.column_id) {
        problems.push(problem(
            "rule.column_id",
            "COLUMN_MISSING",
            "the project has no column with that id",
            &format!("Use one of: {}.", column_ids.join(", ")),
        ));
    }
    match crate::admission::channel_agent_command(&rule.cmd) {
        _ if rule.cmd.chars().count() > RULE_CMD_MAX_CHARS => problems.push(problem(
            "rule.cmd",
            "COMMAND_INVALID",
            "the command is longer than 200 characters",
            "Use `claude` or `codex --no-daemon` with few flags.",
        )),
        None => problems.push(problem(
            "rule.cmd",
            "COMMAND_INVALID",
            "the command must be an interactive `claude` or `codex` with simple flags",
            "Use e.g. `claude` or `claude --permission-mode acceptEdits` (letters, digits and -_./:=+,@ only; no quotes, pipes or ;).",
        )),
        Some("codex") if !rule.cmd.split(' ').any(|w| w == "--no-daemon") => {
            problems.push(problem(
                "rule.cmd",
                "COMMAND_CODEX_DAEMON",
                "a Codex card must run with --no-daemon (Deck cannot attribute the shared daemon's signals, so its steps would never be sent)",
                "Use `codex --no-daemon`.",
            ))
        }
        Some(_) => {}
    }
    let dir = if rule.dir.trim().is_empty() {
        scope.roots.first().cloned().unwrap_or_default()
    } else {
        rule.dir.trim().to_string()
    };
    match canonical_scope(&dir, &scope.roots) {
        Ok(path) => rule.dir = path.display().to_string(),
        Err(_) => problems.push(problem(
            "rule.dir",
            "DIR_OUTSIDE",
            "the directory does not exist or is outside your authorized directory",
            &format!(
                "Use an existing directory inside {}.",
                scope.roots.join(" or ")
            ),
        )),
    }
    let steps = match template.filter(|t| t.name == rule.template) {
        Some(t) => Some(t.steps.clone()),
        None => find_template(&rule.template).map(template_steps),
    };
    match &steps {
        None => problems.push(problem(
            "rule.template",
            "TEMPLATE_MISSING",
            "the template is neither drafted here nor in the project",
            "Name the drafted template, or one deck_config_describe lists.",
        )),
        Some(steps) => {
            if template.is_none_or(|t| t.name != rule.template)
                && steps.iter().any(|s| leads_with_message(s))
            {
                problems.push(problem(
                    "rule.template",
                    "TEMPLATE_BLOCKED",
                    "that template has a step beginning with a {{msg.*}} placeholder, which Deck refuses for handed-over items",
                    "Draft a corrected version of the template in the same proposal.",
                ));
            }
            if rule.auto_continue
                && !rule.external
                && steps.iter().any(|s| step_class(s) == "bounded")
            {
                problems.push(problem(
                    "rule.external",
                    "EXTERNAL_CONSENT_NEEDED",
                    "auto_continue cannot cover steps with placeholders unless external is also asked for",
                    "Set external: true (the user approves the handed-over text going to the agent), or set auto_continue: false.",
                ));
            }
        }
    }
    if !matches!(rule.finish.as_str(), "keep" | "close") {
        problems.push(problem(
            "rule.finish",
            "FINISH_INVALID",
            "finish is keep or close",
            "Use `keep` unless the card should go once its steps are sent and the agent exited.",
        ));
    }
    rule
}

fn preview<'a>(
    project: &Value,
    template: Option<&TemplateDraft>,
    rule: Option<&RuleDraft>,
    find_template: &dyn Fn(&str) -> Option<&'a Value>,
    config: &crate::inbound::Config,
) -> Value {
    let column_name = |id: &str| {
        columns(project)
            .iter()
            .find(|c| c["id"].as_str() == Some(id))
            .and_then(|c| c["name"].as_str().map(str::to_owned))
            .unwrap_or_default()
    };
    let mut changes = Vec::new();
    let mut happens = Vec::new();
    if let Some(t) = template {
        match find_template(&t.name) {
            Some(before) => {
                let old = template_steps(before);
                changes.push(json!({"field": "template.steps", "before": old.len(), "after": t.steps.len(), "changed": old != t.steps}));
                happens.push(format!("The project's template \"{}\" is replaced by the drafted steps once the user approves; rules using it lose their approval until approved again.", t.name));
            }
            None => happens.push(format!(
                "A new template \"{}\" with {} step(s) is added.",
                t.name,
                t.steps.len()
            )),
        }
    }
    if let Some(r) = rule {
        if let Some(before) = agent_rule(config, &r.automation_id) {
            for (field, old, new) in [
                ("name", before.name.clone(), r.name.clone()),
                ("column_id", before.column_id.clone(), r.column_id.clone()),
                ("dir", before.dir.clone(), r.dir.clone()),
                ("cmd", before.cmd.clone(), r.cmd.clone()),
                ("template", before.template.clone(), r.template.clone()),
                (
                    "finish",
                    if before.finish == "close" {
                        "close"
                    } else {
                        "keep"
                    }
                    .into(),
                    r.finish.clone(),
                ),
                (
                    "review_each",
                    before.review_each.to_string(),
                    r.review_each.to_string(),
                ),
                (
                    "auto_continue",
                    before.auto_send.is_some().to_string(),
                    r.auto_continue.to_string(),
                ),
                (
                    "first_send",
                    before.first_send_without_readiness.to_string(),
                    r.first_send.to_string(),
                ),
            ] {
                if old != new {
                    changes.push(
                        json!({"field": format!("rule.{field}"), "before": old, "after": new}),
                    );
                }
            }
        }
        happens.push(format!(
            "Each deck_automation_trigger naming \"{}\" opens one card in column \"{}\" running `{}` in {}.",
            r.automation_id,
            column_name(&r.column_id),
            r.cmd,
            r.dir
        ));
        happens.push(if r.first_send {
            "Step 1 is typed into the new agent at once (if the user also approves that).".into()
        } else {
            "Step 1 waits until the agent has interacted once, or the user sends it.".into()
        });
        happens.push(if r.auto_continue {
            "Later steps are sent after each turn ends, if the user approves auto-continue; otherwise the user sends each.".into()
        } else {
            "Later steps wait for the user to send each one.".into()
        });
    }
    json!({
        "template": template.map(|t| json!({
            "name": t.name,
            "new": find_template(&t.name).is_none(),
            "steps": t.steps,
            "classes": t.steps.iter().map(|s| step_class(s)).collect::<Vec<_>>(),
            "follow_ups": t.follow_ups,
        })),
        "rule": rule.map(|r| json!({
            "automation_id": r.automation_id,
            "new": agent_rule(config, &r.automation_id).is_none(),
            "name": r.name,
            "column": {"id": r.column_id, "name": column_name(&r.column_id)},
            "dir": r.dir,
            "cmd": r.cmd,
            "template": r.template,
            "finish": r.finish,
            "review_each": r.review_each,
            "auto_continue": r.auto_continue,
            "external": r.external,
            "first_send": r.first_send,
        })),
        "changes": changes,
        "whatHappens": happens,
    })
}

/* ---------- propose and get ---------- */

pub(super) fn config_propose(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let args: ProposeArgs = parse(arguments)?;
    if !valid_id(&args.project_id) || args.request_id.as_ref().is_some_and(|id| !valid_id(id)) {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "project_id or request_id is out of shape",
            "Use a project_id from deck_config_describe and a fresh request id (letters, digits, _ -).",
        ));
    }
    let client = runtime
        .read(|doc| drafting(doc, client_id).cloned())
        .map_err(map_error)?
        .map_err(map_error)?;
    let (Some(board), Some(config)) = (board(), super::automation::automation_config()) else {
        return Err(unavailable());
    };
    let checked = check(&client, &args, &board, &config)?;
    if args.validate_only {
        return Ok(json!({
            "ok": true,
            "valid": checked.problems.is_empty(),
            "problems": checked.problems,
            "preview": checked.preview,
            "nextAction": if checked.problems.is_empty() {
                "Submit the same draft without validate_only and with a fresh request_id."
            } else {
                "Fix each problem (field, fix) and validate again."
            }
        }));
    }
    let Some(request_id) = args.request_id.clone() else {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "a proposal needs a request_id",
            "Add a fresh request_id; reuse it on every retry of this submission.",
        ));
    };
    if !checked.problems.is_empty() {
        return Err(json!({"ok": false, "error": {
            "code": "INVALID_DRAFT",
            "message": "the draft has problems; nothing was recorded",
            "nextAction": "Fix each problem and validate again with validate_only: true.",
            "problems": checked.problems,
        }}));
    }
    let request_hash = request_hash("deck_config_propose", &args);
    let content_hash = sha(serde_json::to_string(&json!({
        "project": args.project_id,
        "template": checked.template,
        "rule": checked.rule,
    }))
    .unwrap_or_default()
    .as_bytes());
    let recorded = runtime
        .write(|doc| -> Result<Result<(Proposal, bool), Value>, DeckError> {
            drafting(doc, client_id)?;
            if let Some(existing) = doc
                .proposals
                .iter()
                .find(|p| p.client_id == client_id && p.request_id == request_id)
            {
                if existing.request_hash != request_hash {
                    return Ok(Err(error_value(
                        "REQUEST_ID_REUSED",
                        "this request_id was used for another draft",
                        "Use a fresh request_id for a new draft.",
                    )));
                }
                return Ok(Ok((existing.clone(), false)));
            }
            let mine = |p: &&Proposal| p.client_id == client_id && p.status == PENDING;
            if let Some(same) = doc
                .proposals
                .iter()
                .filter(mine)
                .find(|p| p.content_hash == content_hash)
            {
                return Ok(Err(json!({"ok": false, "error": {
                    "code": "DUPLICATE_PROPOSAL",
                    "message": "an identical draft is already waiting for the user",
                    "nextAction": "Do not resubmit; poll deck_config_proposal_get with this proposalId.",
                    "proposalId": same.id,
                }})));
            }
            if doc.proposals.iter().filter(mine).count() >= MAX_PENDING {
                return Ok(Err(error_value(
                    "TOO_MANY_PENDING",
                    "four of this client's proposals already wait for the user",
                    "Wait until the user decides (deck_config_proposal_get) before drafting more.",
                )));
            }
            let proposal = Proposal {
                id: random_id("prop_", 16)?,
                client_id: client_id.into(),
                request_id: request_id.clone(),
                request_hash: request_hash.clone(),
                content_hash: content_hash.clone(),
                project_id: args.project_id.clone(),
                note: args.note.clone().unwrap_or_default().trim().to_string(),
                template: checked.template.clone(),
                rule: checked.rule.clone(),
                created_at: now_ms(),
                status: PENDING.into(),
                decided_at: None,
                rule_id: None,
            };
            doc.proposals.push(proposal.clone());
            prune(doc, client_id);
            audit(
                doc,
                "config-proposed",
                AuditLink {
                    principal_id: Some(client_id),
                    ..Default::default()
                },
            )?;
            Ok(Ok((proposal, true)))
        })
        .map_err(map_error)??;
    if recorded.1 {
        emit_proposals(runtime);
    }
    Ok(json!({
        "ok": true,
        "proposalId": recorded.0.id,
        "status": recorded.0.status,
        "preview": checked.preview,
        "nextAction": "Tell the user a draft waits in Deck's Needs attention. Poll deck_config_proposal_get later; do not resubmit."
    }))
}

/// Keep the newest `MAX_KEPT` proposals of the client; a pending one is
/// never dropped (at most `MAX_PENDING` exist).
fn prune(doc: &mut DiskDoc, client_id: &str) {
    let mut kept = 0;
    let mut keep = vec![true; doc.proposals.len()];
    for (i, p) in doc.proposals.iter().enumerate().rev() {
        if p.client_id != client_id {
            continue;
        }
        kept += 1;
        keep[i] = p.status == PENDING || kept <= MAX_KEPT;
    }
    let mut flags = keep.into_iter();
    doc.proposals.retain(|_| flags.next().unwrap_or(true));
}

fn next_action(status: &str) -> &'static str {
    match status {
        PENDING => "The user has not decided yet. Poll again later; do not resubmit.",
        "approved" => "Saved as drafted. deck_automation_list lists the rule (if one was drafted); trigger it with its id and current revision.",
        "edited" => "The user saved it with changes and without automatic sending; they finish it in the editor. Read deck_automation_list (and deck_config_describe) before relying on it.",
        "rejected" => "The user rejected this draft. Ask them what they want before drafting again.",
        _ => "The user withdrew this client's draft permission; the draft can no longer be approved.",
    }
}

fn proposal_view(p: &Proposal) -> Value {
    json!({
        "proposalId": p.id,
        "status": p.status,
        "projectId": p.project_id,
        "template": p.template.as_ref().map(|t| t.name.clone()),
        "automationId": p.rule.as_ref().map(|r| r.automation_id.clone()),
        "createdAt": p.created_at,
        "decidedAt": p.decided_at,
        "nextAction": next_action(&p.status),
    })
}

pub(super) fn proposal_get(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let args: ProposalGetArgs = parse(arguments)?;
    runtime
        .read(|doc| {
            drafting(doc, client_id)?;
            Ok(doc
                .proposals
                .iter()
                .find(|p| p.id == args.proposal_id && p.client_id == client_id)
                .map(|p| {
                    let mut view = proposal_view(p);
                    view["ok"] = json!(true);
                    view
                }))
        })
        .map_err(map_error)?
        .map_err(map_error)?
        .ok_or_else(|| {
            error_value(
                "PROPOSAL_NOT_FOUND",
                "this client has no proposal with that id",
                "Use a proposalId deck_config_propose returned; old decided proposals are dropped.",
            )
        })
}

/* ---------- the user's side ---------- */

fn emit_proposals(runtime: &Runtime) {
    if let Some(app) = runtime.app.as_ref() {
        let _ = app.emit("mcp-proposals-changed", ());
    }
}

/// Every kept proposal with its client's name, for Needs attention (the
/// pending ones) and the automations drawer (who drafted a rule).
pub(super) fn proposals_ui(runtime: &Runtime) -> Result<Vec<Value>, DeckError> {
    runtime.read(|doc| {
        doc.proposals
            .iter()
            .map(|p| {
                let client = doc.config.clients.iter().find(|c| c.id == p.client_id);
                json!({
                    "id": p.id,
                    "clientId": p.client_id,
                    "clientName": client.map(|c| c.name.clone()).unwrap_or_default(),
                    "projectId": p.project_id,
                    "note": p.note,
                    "status": p.status,
                    "createdAt": p.created_at,
                    "decidedAt": p.decided_at,
                    "ruleId": p.rule_id,
                    "template": p.template.as_ref().map(|t| json!({
                        "name": t.name, "steps": t.steps, "followUps": t.follow_ups,
                    })),
                    "rule": p.rule.as_ref().map(|r| json!({
                        "automationId": r.automation_id,
                        "name": r.name,
                        "columnId": r.column_id,
                        "dir": r.dir,
                        "cmd": r.cmd,
                        "template": r.template,
                        "finish": r.finish,
                        "reviewEach": r.review_each,
                        "autoContinue": r.auto_continue,
                        "external": r.external,
                        "firstSend": r.first_send,
                    })),
                })
            })
            .collect()
    })
}

/// Record the user's decision on a pending proposal, after the webview
/// wrote what the user chose (or nothing, for a rejection). Bookkeeping for
/// the client, never authority.
pub(super) fn proposal_decide(
    runtime: &Runtime,
    proposal_id: &str,
    decision: &str,
    rule_id: Option<String>,
) -> Result<(), DeckError> {
    if !matches!(decision, "approved" | "edited" | "rejected")
        || rule_id.as_deref().is_some_and(|id| !valid_id(id))
    {
        return Err(DeckError::new(ErrorKind::Invalid, "invalid decision"));
    }
    runtime.write(|doc| {
        let proposal = doc
            .proposals
            .iter_mut()
            .find(|p| p.id == proposal_id)
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, PROPOSAL_DECIDED))?;
        if proposal.status != PENDING {
            return Err(DeckError::new(ErrorKind::ContextChanged, PROPOSAL_DECIDED));
        }
        proposal.status = decision.into();
        proposal.decided_at = Some(now_ms());
        proposal.rule_id = rule_id.filter(|_| decision != "rejected");
        let client = proposal.client_id.clone();
        audit(
            doc,
            "config-decided",
            AuditLink {
                principal_id: Some(&client),
                reason_code: Some(decision),
                ..Default::default()
            },
        )
    })?;
    emit_proposals(runtime);
    Ok(())
}

/// The client may no longer draft (withdrawn or revoked): what waits for the
/// user can no longer be approved. Inside the caller's ledger write.
pub(super) fn withdraw_pending(doc: &mut DiskDoc, client_id: &str) -> bool {
    let mut any = false;
    for p in doc
        .proposals
        .iter_mut()
        .filter(|p| p.client_id == client_id && p.status == PENDING)
    {
        p.status = "withdrawn".into();
        p.decided_at = Some(now_ms());
        any = true;
    }
    any
}

pub(super) fn note_withdrawn(runtime: &Runtime) {
    emit_proposals(runtime);
}
