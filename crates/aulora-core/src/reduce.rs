//! Turns Claude Code hook events into session state.
//!
//! Everything here is pure: given a session and an event, update the session
//! and return what should appear in the activity feed.

use serde_json::{Map, Value};

use crate::model::*;
use crate::proto::HookEnvelope;
use crate::util::{first_line, rel_path, squash, truncate};

const RECENT_TOOLS: usize = 14;
const FILES_TOUCHED: usize = 40;
const CONTEXT_HISTORY: usize = 48;
const PROMPT_HISTORY: usize = 12;

fn s<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

pub fn event_name(payload: &Value) -> &str {
    s(payload, "hook_event_name").unwrap_or("")
}

/// Short description of what a tool call touches.
pub fn tool_target(name: &str, input: &Value, base: &str) -> String {
    let path = |k: &str| s(input, k).map(|p| rel_path(p, base));
    let t = match name {
        "Bash" | "BashOutput" | "PowerShell" => s(input, "command")
            .map(|c| format!("$ {}", squash(first_line(c))))
            .or_else(|| s(input, "description").map(str::to_string)),
        "Read" | "Edit" | "MultiEdit" | "Write" => path("file_path"),
        "NotebookEdit" | "NotebookRead" => path("notebook_path").or_else(|| path("file_path")),
        "Grep" => s(input, "pattern").map(|p| match path("path") {
            Some(dir) => format!("/{p}/ in {dir}"),
            None => format!("/{p}/"),
        }),
        "Glob" => s(input, "pattern").map(str::to_string),
        "LS" => path("path"),
        "WebFetch" => s(input, "url").map(|u| u.trim_start_matches("https://").to_string()),
        "WebSearch" => s(input, "query").map(|q| format!("“{q}”")),
        "Task" | "Agent" => {
            let ty = s(input, "subagent_type").unwrap_or("agent");
            Some(match s(input, "description") {
                Some(d) => format!("{ty}: {d}"),
                None => ty.to_string(),
            })
        }
        "TodoWrite" => Some("update todo list".to_string()),
        "AskUserQuestion" => first_question(input),
        "ExitPlanMode" => Some("plan ready".to_string()),
        _ if name.starts_with("mcp__") => {
            let mut parts = name.trim_start_matches("mcp__").splitn(2, "__");
            let server = parts.next().unwrap_or("");
            let tool = parts.next().unwrap_or("");
            Some(format!("{server} · {tool}"))
        }
        _ => None,
    };
    truncate(&t.unwrap_or_default(), 160)
}

fn first_question(input: &Value) -> Option<String> {
    input
        .get("questions")
        .and_then(Value::as_array)
        .and_then(|qs| qs.first())
        .and_then(|q| s(q, "question"))
        .map(str::to_string)
}

fn is_edit_tool(name: &str) -> bool {
    matches!(name, "Edit" | "MultiEdit" | "Write" | "NotebookEdit")
}

/// Pretty tool name for display.
pub fn tool_label(name: &str) -> String {
    if let Some(rest) = name.strip_prefix("mcp__") {
        return rest.split("__").last().unwrap_or(rest).to_string();
    }
    name.to_string()
}

/// The questions of an AskUserQuestion call, with their options.
fn ask_questions(input: &Value) -> Option<Ask> {
    let qs: Vec<AskQuestion> = input
        .get("questions")?
        .as_array()?
        .iter()
        .map(|q| AskQuestion {
            question: q.get("question").and_then(Value::as_str).unwrap_or("").to_string(),
            header: q.get("header").and_then(Value::as_str).map(str::to_string),
            options: q
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|o| AskOption {
                    label: o.get("label").and_then(Value::as_str).unwrap_or("").to_string(),
                    description: o.get("description").and_then(Value::as_str).map(str::to_string),
                })
                .collect(),
            multi: q.get("multiSelect").and_then(Value::as_bool).unwrap_or(false),
        })
        .collect();
    (!qs.is_empty()).then_some(Ask { questions: qs })
}

/// One question with fixed choices (permission prompts, plan approval),
/// numbered as Claude Code numbers them.
fn ask_choices(question: String, choices: &[&str]) -> Ask {
    Ask {
        questions: vec![AskQuestion {
            question,
            header: None,
            options: choices.iter().map(|c| AskOption { label: c.to_string(), description: None }).collect(),
            multi: false,
        }],
    }
}

fn set_state(sess: &mut Session, state: SessionState, at: Ms) {
    if !matches!(state, SessionState::NeedsYou { .. }) {
        sess.ask = None;
    }
    if sess.state != state {
        if !sess.state.same_kind(&state) {
            sess.state_since = at;
        }
        sess.state = state;
    }
}

fn bump_activity(sess: &mut Session, at: Ms) {
    let minute = at / 60_000;
    match sess.activity.last_mut() {
        Some((m, n)) if *m == minute => *n += 1,
        _ => sess.activity.push((minute, 1)),
    }
    let cutoff = minute - 60;
    sess.activity.retain(|(m, _)| *m > cutoff);
}

fn touch_file(sess: &mut Session, path: &str, added: u32, removed: u32, at: Ms) {
    if let Some(i) = sess.files_touched.iter().position(|f| f.path == path) {
        let mut f = sess.files_touched.remove(i);
        f.edits += 1;
        f.added += added;
        f.removed += removed;
        f.last_at = at;
        sess.files_touched.insert(0, f);
    } else {
        sess.files_touched.insert(0, FileTouch { path: path.to_string(), edits: 1, added, removed, last_at: at });
        sess.files_touched.truncate(FILES_TOUCHED);
    }
}

/// Record the context size at the end of a turn.
pub fn push_context(sess: &mut Session, tokens: u64) {
    if sess.context_history.last() != Some(&tokens) {
        sess.context_history.push(tokens);
        if sess.context_history.len() > CONTEXT_HISTORY {
            sess.context_history.remove(0);
        }
    }
}

/// Apply one hook event. Returns entries for the activity feed.
pub fn apply_hook(sess: &mut Session, env: &HookEnvelope) -> Vec<FeedEvent> {
    let p = &env.payload;
    let at = env.at;
    let ev = event_name(p);
    let mut feed = Vec::new();
    let (sid, root) = (sess.id.clone(), sess.project_root.clone());
    let mut push = |kind: FeedKind, text: String| {
        feed.push(FeedEvent {
            at,
            session_id: sid.clone(),
            project_root: root.clone(),
            kind,
            text: truncate(&squash(&text), 1200),
        })
    };

    sess.last_event_at = sess.last_event_at.max(at);
    sess.origin = Origin::Hooks;
    bump_activity(sess, at);
    if let Some(v) = s(p, "cwd") {
        sess.cwd = v.to_string();
    }
    if let Some(v) = s(p, "transcript_path") {
        sess.transcript_path = Some(v.to_string());
    }
    if let Some(v) = s(p, "permission_mode") {
        sess.permission_mode = Some(v.to_string());
    }
    if let Some(v) = p.get("effort").and_then(|e| s(e, "level")) {
        sess.effort = Some(v.to_string());
    }
    if let Some(v) = s(p, "session_title").filter(|t| !t.is_empty()) {
        sess.title = Some(v.to_string());
    }
    if env.claude_pid().is_some() {
        sess.pid = env.claude_pid();
    }
    let e = &env.env;
    if e.term_program.is_some() || e.bundle_id.is_some() || e.pane_id.is_some() {
        sess.terminal = TerminalInfo {
            program: e.term_program.clone(),
            bundle_id: e.bundle_id.clone(),
            pane_id: e.pane_id.clone(),
            tmux: e.tmux,
            editor: sess.terminal.editor.take(),
        };
    }

    let base = sess.cwd.clone();
    let agent = s(p, "agent_type").map(str::to_string);
    let tool_name = s(p, "tool_name").unwrap_or("").to_string();
    let empty = Value::Object(Map::new());
    let tool_input = p.get("tool_input").unwrap_or(&empty);

    match ev {
        "SessionStart" => {
            sess.source = s(p, "source").map(str::to_string);
            if let Some(m) = s(p, "model") {
                sess.model = Some(m.to_string());
            }
            if sess.state.rank() == 9 || sess.turns == 0 {
                set_state(sess, SessionState::Idle, at);
            }
            let src = sess.source.clone().unwrap_or_else(|| "startup".into());
            push(FeedKind::Start, format!("session {src}"));
        }
        "UserPromptSubmit" => {
            let prompt = s(p, "prompt").unwrap_or("").to_string();
            // A background task's notification starts a turn, but it isn't
            // something you asked.
            let typed = !crate::util::is_injected(&prompt);
            if typed {
                sess.last_prompt = Some(prompt.clone());
                sess.last_prompt_at = Some(at);
            }
            sess.live_text = None;
            if typed && !prompt.trim().is_empty() {
                sess.prompts.push(PromptEntry { at, text: truncate(&prompt, 600) });
                if sess.prompts.len() > PROMPT_HISTORY {
                    sess.prompts.remove(0);
                }
            }
            sess.turns += 1;
            sess.turn_started_at = Some(at);
            sess.current_tool = None;
            set_state(sess, SessionState::Working, at);
            push(FeedKind::Prompt, format!("› {}", first_line(&prompt)));
        }
        "PreToolUse" => {
            let target = tool_target(&tool_name, tool_input, &base);
            sess.tool_calls += 1;
            sess.current_tool = Some(ToolCall {
                id: s(p, "tool_use_id").map(str::to_string),
                name: tool_name.clone(),
                target: target.clone(),
                started_at: at,
                duration_ms: None,
                ok: None,
                agent: agent.clone(),
            });
            match tool_name.as_str() {
                "AskUserQuestion" => {
                    set_state(sess, SessionState::NeedsYou { reason: Attention::Question, detail: target.clone() }, at);
                    sess.ask = ask_questions(tool_input);
                    push(FeedKind::Question, format!("asked: {target}"));
                }
                "ExitPlanMode" => {
                    set_state(
                        sess,
                        SessionState::NeedsYou {
                            reason: Attention::PlanApproval,
                            detail: "plan ready for your review".into(),
                        },
                        at,
                    );
                    sess.ask = Some(ask_choices(
                        "Ready to build the plan?".into(),
                        &["Yes, and auto-accept edits", "Yes, and approve each edit", "No, keep planning"],
                    ));
                    push(FeedKind::Question, "plan ready for review".into());
                }
                _ => {
                    if !matches!(sess.state, SessionState::NeedsYou { .. }) {
                        set_state(sess, SessionState::Working, at);
                    }
                    if is_edit_tool(&tool_name) {
                        push(FeedKind::Edit, format!("{} {target}", tool_label(&tool_name)));
                    } else if matches!(tool_name.as_str(), "Bash" | "PowerShell") {
                        push(FeedKind::Tool, target);
                    } else if matches!(tool_name.as_str(), "Task" | "Agent") {
                        push(FeedKind::Agent, format!("spawned {target}"));
                    }
                }
            }
        }
        // Claude Code asks permission for AskUserQuestion too, but what it
        // shows is the question itself, not an allow/deny prompt.
        "PermissionRequest" if tool_name == "AskUserQuestion" => {
            let target = tool_target(&tool_name, tool_input, &base);
            let asked = matches!(sess.state, SessionState::NeedsYou { reason: Attention::Question, .. });
            let ask = ask_questions(tool_input).or_else(|| if asked { sess.ask.take() } else { None });
            set_state(sess, SessionState::NeedsYou { reason: Attention::Question, detail: target.clone() }, at);
            sess.ask = ask;
            if !asked {
                push(FeedKind::Question, format!("asked: {target}"));
            }
        }
        "PermissionRequest" => {
            let target = tool_target(&tool_name, tool_input, &base);
            let detail = if target.is_empty() {
                tool_label(&tool_name)
            } else {
                format!("{} · {target}", tool_label(&tool_name))
            };
            set_state(sess, SessionState::NeedsYou { reason: Attention::Permission, detail: detail.clone() }, at);
            sess.ask = Some(ask_choices(
                format!("Allow {detail}?"),
                &["Yes", "Yes, and don't ask again for this", "No, and tell Claude what to do differently"],
            ));
            push(FeedKind::Permission, format!("needs permission: {detail}"));
        }
        "PostToolUse" | "PostToolUseFailure" => {
            let failed = ev == "PostToolUseFailure";
            let interrupted = p.get("is_interrupt").and_then(Value::as_bool).unwrap_or(false);
            let id = s(p, "tool_use_id");
            let mut call = match sess.current_tool.take() {
                Some(c) if id.is_none() || c.id.as_deref() == id || c.name == tool_name => c,
                other => {
                    // Parallel tool calls: keep the in-flight one, log this one.
                    sess.current_tool = other;
                    ToolCall {
                        id: id.map(str::to_string),
                        name: tool_name.clone(),
                        target: tool_target(&tool_name, tool_input, &base),
                        started_at: at,
                        duration_ms: None,
                        ok: None,
                        agent: agent.clone(),
                    }
                }
            };
            call.duration_ms =
                p.get("duration_ms").and_then(Value::as_u64).or(Some((at - call.started_at).max(0) as u64));
            call.ok = Some(!failed);
            if failed && !interrupted {
                sess.tool_errors += 1;
                let err = s(p, "error").map(first_line).unwrap_or("failed");
                push(FeedKind::Error, format!("{} failed: {err}", tool_label(&tool_name)));
            }
            if !failed && is_edit_tool(&tool_name) {
                let fp = s(tool_input, "file_path").or_else(|| s(tool_input, "notebook_path"));
                if let Some(fp) = fp {
                    let added = tool_input.get("_added").and_then(Value::as_u64).unwrap_or(0);
                    let removed = tool_input.get("_removed").and_then(Value::as_u64).unwrap_or(0);
                    let rel = rel_path(fp, &sess.project_root);
                    touch_file(sess, &rel, added as u32, removed as u32, at);
                }
            }
            sess.recent_tools.insert(0, call);
            sess.recent_tools.truncate(RECENT_TOOLS);
            if matches!(sess.state, SessionState::NeedsYou { .. } | SessionState::Errored { .. }) {
                set_state(sess, SessionState::Working, at);
            }
        }
        "PermissionDenied" => {
            let target = tool_target(&tool_name, tool_input, &base);
            let reason = s(p, "reason").unwrap_or("denied");
            push(FeedKind::Error, format!("auto mode denied {} {target} {reason}", tool_label(&tool_name)));
        }
        "Notification" => {
            let msg = s(p, "message").unwrap_or("").to_string();
            match s(p, "notification_type").unwrap_or("") {
                // Also sent for a question or a plan on screen, which the
                // tool's own events already described better.
                "permission_prompt" => {
                    if !matches!(sess.state, SessionState::NeedsYou { .. }) {
                        set_state(
                            sess,
                            SessionState::NeedsYou { reason: Attention::Permission, detail: msg.clone() },
                            at,
                        );
                        push(FeedKind::Permission, msg);
                    }
                }
                "elicitation_dialog" | "elicitation_url_dialog" | "agent_needs_input" => {
                    set_state(sess, SessionState::NeedsYou { reason: Attention::Input, detail: msg.clone() }, at);
                    push(FeedKind::Question, msg);
                }
                "idle_prompt" => {}
                _ if !msg.is_empty() => push(FeedKind::Info, msg),
                _ => {}
            }
        }
        "Stop" => {
            if let Some(m) = s(p, "last_assistant_message").filter(|m| !m.trim().is_empty()) {
                sess.last_reply = Some(m.to_string());
                sess.last_reply_at = Some(at);
            }
            sess.background_tasks = p.get("_background_tasks").and_then(Value::as_u64).unwrap_or(0) as u32;
            let took = sess.turn_started_at.take().map(|t| (at - t).max(0) as u64);
            sess.last_turn_ms = took.or(sess.last_turn_ms);
            sess.current_tool = None;
            sess.subagents_active = 0;
            set_state(sess, SessionState::YourTurn, at);
            let reply = sess.last_reply.as_deref().map(first_line).unwrap_or("");
            let took = took.map(|t| crate::util::human_duration(t as i64)).unwrap_or_default();
            push(FeedKind::Done, format!("finished in {took} · {reply}"));
        }
        "StopFailure" => {
            let detail = s(p, "error")
                .or_else(|| s(p, "message"))
                .or_else(|| s(p, "reason"))
                .unwrap_or("turn failed")
                .to_string();
            sess.turn_started_at = None;
            sess.current_tool = None;
            set_state(sess, SessionState::Errored { detail: first_line(&detail).to_string() }, at);
            push(FeedKind::Error, detail);
        }
        "SubagentStart" => {
            sess.subagents_active += 1;
            push(FeedKind::Agent, format!("{} agent started", agent.unwrap_or_else(|| "sub".into())));
        }
        "SubagentStop" => {
            sess.subagents_active = sess.subagents_active.saturating_sub(1);
            // The whole summary: it's what the agent found or did.
            let msg = s(p, "last_assistant_message").unwrap_or("");
            push(FeedKind::Agent, format!("{} agent done · {msg}", agent.unwrap_or_else(|| "sub".into())));
        }
        "PreCompact" => {
            set_state(sess, SessionState::Compacting, at);
            let trig = s(p, "trigger").unwrap_or("auto");
            push(FeedKind::Compact, format!("compacting context ({trig})"));
        }
        "PostCompact" => {
            let next = if sess.turn_started_at.is_some() {
                SessionState::Working
            } else if sess.turns > 0 {
                SessionState::YourTurn
            } else {
                SessionState::Idle
            };
            set_state(sess, next, at);
            push(FeedKind::Compact, "context compacted".into());
        }
        "SessionEnd" => {
            let reason = s(p, "reason").unwrap_or("exit").to_string();
            sess.current_tool = None;
            sess.subagents_active = 0;
            set_state(sess, SessionState::Ended { reason: reason.clone() }, at);
            push(FeedKind::End, format!("session ended ({reason})"));
        }
        _ => {}
    }
    feed
}

/// Mark a session ended because its process is gone.
pub fn mark_gone(sess: &mut Session, at: Ms) -> Option<FeedEvent> {
    if !sess.state.is_live() {
        return None;
    }
    sess.current_tool = None;
    sess.subagents_active = 0;
    set_state(sess, SessionState::Ended { reason: "process exited".into() }, at);
    Some(FeedEvent {
        at,
        session_id: sess.id.clone(),
        project_root: sess.project_root.clone(),
        kind: FeedKind::End,
        text: "claude process exited".into(),
    })
}

// ---------------------------------------------------------------------------
// Payload trimming, done in the hook process before anything is sent.

const KEEP_INPUT: &[&str] = &[
    "file_path",
    "notebook_path",
    "path",
    "command",
    "description",
    "pattern",
    "glob",
    "url",
    "query",
    "subagent_type",
    "prompt",
    "plan",
    "questions",
];

fn clip(v: &mut Value, max: usize) {
    if let Value::String(s) = v
        && s.chars().count() > max
    {
        *s = truncate(s, max);
    }
}

fn count_lines(v: Option<&Value>) -> u64 {
    v.and_then(Value::as_str).map(|s| s.lines().count() as u64).unwrap_or(0)
}

/// Remove large fields (file contents, tool output) and compute edit sizes.
pub fn trim_payload(mut v: Value) -> Value {
    let Some(obj) = v.as_object_mut() else { return v };
    obj.remove("tool_response");
    obj.remove("permission_suggestions");
    obj.remove("session_crons");
    if let Some(tasks) = obj.remove("background_tasks") {
        let n = tasks.as_array().map(|a| a.len()).unwrap_or(0);
        obj.insert("_background_tasks".into(), Value::from(n));
    }
    for (k, max) in [("prompt", 4000), ("last_assistant_message", 6000), ("message", 1000), ("error", 1000)] {
        if let Some(f) = obj.get_mut(k) {
            clip(f, max);
        }
    }
    if let Some(input) = obj.get_mut("tool_input")
        && let Some(inp) = input.as_object()
    {
        let mut out = Map::new();
        // Edit sizes, before the strings are dropped.
        let (added, removed) = if let Some(edits) = inp.get("edits").and_then(Value::as_array) {
            edits
                .iter()
                .fold((0, 0), |(a, r), e| (a + count_lines(e.get("new_string")), r + count_lines(e.get("old_string"))))
        } else if inp.contains_key("content") {
            (count_lines(inp.get("content")), 0)
        } else {
            (count_lines(inp.get("new_string").or(inp.get("new_source"))), count_lines(inp.get("old_string")))
        };
        if added + removed > 0 {
            out.insert("_added".into(), Value::from(added));
            out.insert("_removed".into(), Value::from(removed));
        }
        for k in KEEP_INPUT {
            if let Some(val) = inp.get(*k) {
                let mut val = val.clone();
                if *k == "questions" {
                    if let Some(arr) = val.as_array() {
                        val = Value::Array(
                            arr.iter()
                                .take(4)
                                .map(|q| {
                                    let mut m = Map::new();
                                    for f in ["question", "header"] {
                                        if let Some(x) = q.get(f) {
                                            let mut x = x.clone();
                                            clip(&mut x, 300);
                                            m.insert(f.into(), x);
                                        }
                                    }
                                    if let Some(x) = q.get("multiSelect") {
                                        m.insert("multiSelect".into(), x.clone());
                                    }
                                    // The options, so the question can be answered from aulora.
                                    let options: Vec<Value> = q
                                        .get("options")
                                        .and_then(Value::as_array)
                                        .into_iter()
                                        .flatten()
                                        .take(9)
                                        .map(|o| {
                                            let mut om = Map::new();
                                            for f in ["label", "description"] {
                                                if let Some(x) = o.get(f) {
                                                    let mut x = x.clone();
                                                    clip(&mut x, 200);
                                                    om.insert(f.into(), x);
                                                }
                                            }
                                            Value::Object(om)
                                        })
                                        .collect();
                                    m.insert("options".into(), Value::Array(options));
                                    Value::Object(m)
                                })
                                .collect(),
                        );
                    }
                } else {
                    clip(&mut val, if *k == "command" { 600 } else { 400 });
                }
                out.insert((*k).into(), val);
            }
        }
        *input = Value::Object(out);
    }
    v
}

impl HookEnvelope {
    pub fn claude_pid(&self) -> Option<u32> {
        self.env.claude_pid
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::HookEnv;
    use serde_json::json;

    fn env(at: Ms, payload: Value) -> HookEnvelope {
        HookEnvelope { v: 1, at, payload: trim_payload(payload), env: HookEnv::default() }
    }

    fn session() -> Session {
        Session::new("abc12345-def", "/p", "/p", 0)
    }

    #[test]
    fn full_turn() {
        let mut s = session();
        apply_hook(
            &mut s,
            &env(1, json!({"hook_event_name":"SessionStart","source":"startup","model":"claude-opus-5-5"})),
        );
        assert_eq!(s.state, SessionState::Idle);
        assert_eq!(s.model.as_deref(), Some("claude-opus-5-5"));

        apply_hook(&mut s, &env(10, json!({"hook_event_name":"UserPromptSubmit","prompt":"fix the bug\nplease"})));
        assert_eq!(s.state, SessionState::Working);
        assert_eq!(s.turns, 1);

        apply_hook(
            &mut s,
            &env(
                20,
                json!({"hook_event_name":"PreToolUse","tool_name":"Edit","tool_use_id":"t1",
            "tool_input":{"file_path":"/p/src/main.rs","old_string":"a\nb","new_string":"a\nb\nc"}}),
            ),
        );
        assert_eq!(s.current_tool.as_ref().unwrap().target, "src/main.rs");

        apply_hook(
            &mut s,
            &env(
                25,
                json!({"hook_event_name":"PermissionRequest","tool_name":"Bash",
            "tool_input":{"command":"rm -rf node_modules"}}),
            ),
        );
        assert!(matches!(s.state, SessionState::NeedsYou { reason: Attention::Permission, .. }));

        apply_hook(
            &mut s,
            &env(
                30,
                json!({"hook_event_name":"PostToolUse","tool_name":"Edit","tool_use_id":"t1",
            "tool_input":{"file_path":"/p/src/main.rs","old_string":"a\nb","new_string":"a\nb\nc"},
            "tool_response":{"huge":"x"}, "duration_ms": 9}),
            ),
        );
        assert_eq!(s.state, SessionState::Working);
        assert_eq!(s.files_touched[0].path, "src/main.rs");
        assert_eq!((s.files_touched[0].added, s.files_touched[0].removed), (3, 2));
        assert_eq!(s.recent_tools[0].duration_ms, Some(9));
        assert!(s.current_tool.is_none());

        let feed =
            apply_hook(&mut s, &env(5_010, json!({"hook_event_name":"Stop","last_assistant_message":"Fixed it."})));
        assert_eq!(s.state, SessionState::YourTurn);
        assert_eq!(s.last_turn_ms, Some(5_000));
        assert_eq!(feed[0].kind, FeedKind::Done);

        apply_hook(&mut s, &env(6_000, json!({"hook_event_name":"SessionEnd","reason":"prompt_input_exit"})));
        assert!(!s.state.is_live());
    }

    #[test]
    fn question_and_plan_need_you() {
        let mut s = session();
        apply_hook(
            &mut s,
            &env(
                1,
                json!({"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion",
            "tool_input":{"questions":[{"question":"Which DB?","header":"DB","options":[{"label":"a"}]}]}}),
            ),
        );
        assert_eq!(s.state, SessionState::NeedsYou { reason: Attention::Question, detail: "Which DB?".into() });
        let ask = s.ask.clone().expect("the question is kept");
        assert_eq!(
            (ask.questions[0].question.as_str(), ask.questions[0].options[0].label.as_str()),
            ("Which DB?", "a")
        );
        // Claude Code 2.1.304 then asks permission for the tool; the
        // question is still what it shows, so the question stays.
        let feed = apply_hook(
            &mut s,
            &env(
                2,
                json!({"hook_event_name":"PermissionRequest","tool_name":"AskUserQuestion",
            "tool_input":{"questions":[{"question":"Which DB?","header":"DB","options":[{"label":"a"}]}]}}),
            ),
        );
        assert!(feed.is_empty(), "asked once");
        assert_eq!(s.state, SessionState::NeedsYou { reason: Attention::Question, detail: "Which DB?".into() });
        assert_eq!(s.ask, Some(ask.clone()));
        // And its "Claude needs your permission" notification after that.
        apply_hook(
            &mut s,
            &env(
                3,
                json!({"hook_event_name":"Notification","notification_type":"permission_prompt",
            "message":"Claude needs your permission"}),
            ),
        );
        assert_eq!(s.state, SessionState::NeedsYou { reason: Attention::Question, detail: "Which DB?".into() });
        assert_eq!(s.ask, Some(ask));
        apply_hook(&mut s, &env(3, json!({"hook_event_name":"PostToolUse","tool_name":"AskUserQuestion"})));
        assert_eq!(s.state, SessionState::Working);
        assert!(s.ask.is_none(), "answered: gone");
    }

    #[test]
    fn trims_large_fields() {
        let big = "x".repeat(50_000);
        let v = trim_payload(json!({"hook_event_name":"PostToolUse","tool_name":"Write",
            "tool_input":{"file_path":"/a","content": format!("{big}\n{big}")},
            "tool_response": {"content": big}}));
        assert!(v.get("tool_response").is_none());
        assert!(v["tool_input"].get("content").is_none());
        assert_eq!(v["tool_input"]["_added"], 2);
        assert!(serde_json::to_string(&v).unwrap().len() < 300);
    }

    #[test]
    fn targets() {
        assert_eq!(tool_target("Bash", &json!({"command":"npm   test\nmore"}), "/"), "$ npm test");
        assert_eq!(tool_target("mcp__linear__get_issue", &json!({}), "/"), "linear · get_issue");
        assert_eq!(tool_target("Grep", &json!({"pattern":"TODO","path":"/p/src"}), "/p"), "/TODO/ in src");
    }
}
