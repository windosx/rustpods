//! Approves sensitive agent tool permissions hands-free via head gestures.
//!
//! Desktop AI agents (like ZCode, Codex) frequently prompt for user permission
//! before executing sensitive tools (such as terminal commands via `Bash`,
//! file edits via `Write` or `edit_file`). RustPods provides a `PreToolUse` hook
//! that receives permission prompts and lets the wearer approve or deny them
//! using natural head gestures:
//!
//! * **Nod** (点头) -> **Allow** (允许执行)
//! * **Shake head** (摇头) -> **Deny** (拒绝执行)
//! * **Timeout or Close** -> **Pass** (回退至终端原生提示)
//!
//! This module implements both ends of that channel:
//!
//! * `rustpods --zcode-ask-hook` / `rustpods --codex-ask-hook` is the hook command.
//!   It reads the hook JSON from stdin, forwards the request over a Unix socket
//!   to the running RustPods daemon, and translates the reply into hook output.
//!   Every failure path falls back to "ask" / "pass" without blocking.
//! * [`run_socket_server`] runs inside RustPods (GUI or `--no-gui` daemon). It
//!   registers the permission request in the [`ASK_HUB`], which surfaces the
//!   floating permission card in the UI, and waits for [`run_question_answers`]
//!   to resolve it.

use crate::core::nod_counter::{NodCounter, NodEvent};
use crate::core::head_tracking::HeadPose;
use crate::system::agent_hooks::HookInstallStatus;
use serde::{Deserialize, Serialize};
use serde_json::json;
#[cfg(target_os = "linux")]
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{oneshot, watch};
use tracing::{debug, error, info, warn};

/// How long a question may sit pending inside RustPods before the connection
/// gives up and passes through. The hook waits slightly longer, and its own
/// hook `timeoutMs` longer still, so RustPods always decides first.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(172);
/// A hook connection must deliver its payload within this window; longer is
/// indistinguishable from a wedged client.
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the hook process waits for a reply from RustPods.
const HOOK_REPLY_TIMEOUT: Duration = Duration::from_secs(175);
/// Connect timeout for the hook process reaching the RustPods socket. If the
/// daemon is not running, ZCode must not wait around.
const HOOK_CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
/// A head pose stream stops counting as "worn" after this much silence; while
/// it is stale, incoming questions pass through immediately.
const TRACKING_FRESH_AFTER: Duration = Duration::from_secs(3);
/// A question payload larger than this is treated as garbage.
const MAX_PAYLOAD_BYTES: u64 = 4 * 1024 * 1024;
/// How often the pending-question heartbeat logs what the counter sees.
const TELEMETRY_EVERY: Duration = Duration::from_secs(1);

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// One question, normalised out of the hook payload. `options` are labels;
/// the tool schema sends option objects, but plain strings are accepted too.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HookQuestion {
    #[serde(default)]
    pub question: String,
    #[serde(default)]
    pub header: String,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default, rename = "multiSelect")]
    pub multi_select: bool,
}

fn default_agent_name() -> String {
    "Agent".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PermissionPrompt {
    pub tool_name: String,
    #[serde(default)]
    pub command_or_arg: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HookRequest {
    #[serde(default)]
    pub questions: Vec<HookQuestion>,
    #[serde(default)]
    pub permission: Option<PermissionPrompt>,
    #[serde(default = "default_agent_name")]
    pub agent: String,
}

/// One question selection from a multi-question batch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SingleSelection {
    pub index: usize,
    pub label: String,
}

/// RustPods → hook reply.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AskReply {
    /// The wearer nodded `index + 1` times; the label is the selected option.
    Select { index: usize, label: String },
    /// Selected multiple questions in sequence.
    BatchSelect { selections: Vec<SingleSelection> },
    /// User nodded to allow tool execution.
    Allow,
    /// User shook head to deny tool execution.
    Deny,
    /// Cancelled, timed out, or unanswerable: let the agent ask normally.
    Pass,
}

/// Extracts the question list from a hook payload. Accepts both the full
/// `PreToolUse` envelope (`tool_input.questions`) and a bare question list,
/// and tolerates option objects or plain strings. An option object without a
/// `label` makes the whole payload unusable: silently dropping it would
/// shift every later nod index onto the wrong answer.
/// Normalises text from a hook payload for display and matching.
///
/// Payloads carry the caller's formatting: some arrive with stray `\r` or
/// runs of blank lines (seen live - the question card then renders a stack of
/// empty lines). Each `\r` and `\n` becomes a single line break, so blank
/// runs collapse and edges trim.
fn sanitize_payload_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_nl = 0usize;
    let mut any = false;
    for ch in s.chars() {
        match ch {
            '\r' | '\n' => pending_nl += 1,
            ' ' | '\t' if pending_nl > 0 => {}
            _ => {
                if pending_nl > 0 && any {
                    out.push('\n');
                }
                pending_nl = 0;
                any = true;
                out.push(ch);
            }
        }
    }
    out
}

fn parse_options_from(obj: &serde_json::Value) -> Option<Vec<String>> {
    match obj.get("options") {
        Some(serde_json::Value::Array(opts)) => {
            let mut labels = Vec::new();
            for o in opts {
                match o {
                    serde_json::Value::String(s) => labels.push(sanitize_payload_text(s)),
                    serde_json::Value::Object(_) => {
                        labels.push(sanitize_payload_text(o.get("label")?.as_str()?));
                    }
                    _ => return None,
                }
            }
            Some(labels)
        }
        _ => Some(Vec::new()),
    }
}

pub fn parse_questions(value: &serde_json::Value) -> Option<HookRequest> {
    let mut tool_input_val: Option<serde_json::Value> = None;
    if let Some(ti) = value.get("tool_input") {
        if let Some(s) = ti.as_str() {
            if let Ok(parsed) = serde_json::from_str(s) {
                tool_input_val = Some(parsed);
            }
        } else if ti.is_object() {
            tool_input_val = Some(ti.clone());
        }
    } else if let Some(args) = value.get("arguments") {
        if let Some(s) = args.as_str() {
            if let Ok(parsed) = serde_json::from_str(s) {
                tool_input_val = Some(parsed);
            }
        } else if args.is_object() {
            tool_input_val = Some(args.clone());
        }
    }
    let tool_input = tool_input_val.as_ref().or_else(|| value.get("tool_input"));

    let questions_array = tool_input
        .and_then(|t| t.get("questions"))
        .or_else(|| value.get("questions"))
        .and_then(|q| q.as_array());

    let tool_name = value
        .get("tool_name")
        .and_then(|t| t.as_str())
        .unwrap_or_default();
    let agent = if let Some(a) = value
        .get("agent")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
    {
        a.to_string()
    } else if tool_name.contains("request_user_input") {
        "Codex".to_string()
    } else if tool_name.contains("AskUserQuestion") || tool_name.eq_ignore_ascii_case("askuserquestion") {
        "ZCode".to_string()
    } else {
        "Agent".to_string()
    };

    if let Some(questions) = questions_array {
        let mut out = Vec::new();
        for q in questions {
            let options = parse_options_from(q)?;
            let q_str = q
                .get("question")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .or_else(|| q.get("title").and_then(|v| v.as_str()))
                .or_else(|| q.get("header").and_then(|v| v.as_str()))
                .unwrap_or_default();
            let header_str = q
                .get("header")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .or_else(|| q.get("title").and_then(|v| v.as_str()))
                .unwrap_or_default();
            out.push(HookQuestion {
                question: sanitize_payload_text(q_str),
                header: sanitize_payload_text(header_str),
                options,
                multi_select: q
                    .get("multiSelect")
                    .or_else(|| q.get("multi_select"))
                    .or_else(|| q.get("is_multi_select"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            });
        }
        return Some(HookRequest {
            questions: out,
            permission: None,
            agent,
        });
    }

    if let Some(ti) = tool_input {
        let q_str_opt = ti
            .get("question")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| ti.get("title").and_then(|v| v.as_str()));
        if let Some(q_str) = q_str_opt {
            let options = parse_options_from(ti)?;
            let header_str = ti
                .get("header")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .or_else(|| ti.get("title").and_then(|v| v.as_str()))
                .unwrap_or_default();
            return Some(HookRequest {
                questions: vec![HookQuestion {
                    question: sanitize_payload_text(q_str),
                    header: sanitize_payload_text(header_str),
                    options,
                    multi_select: ti
                        .get("multiSelect")
                        .or_else(|| ti.get("multi_select"))
                        .or_else(|| ti.get("is_multi_select"))
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                }],
                permission: None,
                agent,
            });
        }
    }

    let tool_name = value
        .get("tool_name")
        .or_else(|| value.get("tool"))
        .and_then(|t| t.as_str())
        .unwrap_or_default();

    if !tool_name.is_empty()
        && !tool_name.eq_ignore_ascii_case("askuserquestion")
        && !tool_name.contains("request_user_input")
        && !tool_name.contains("ask_question")
    {
        let command_or_arg = if let Some(cmd) = tool_input
            .and_then(|t| t.get("command").or_else(|| t.get("cmd")))
            .and_then(|v| v.as_str())
        {
            cmd.to_string()
        } else if let Some(path) = tool_input
            .and_then(|t| t.get("file_path").or_else(|| t.get("path")))
            .and_then(|v| v.as_str())
        {
            path.to_string()
        } else if let Some(q) = tool_input
            .and_then(|t| t.get("query").or_else(|| t.get("prompt")))
            .and_then(|v| v.as_str())
        {
            q.to_string()
        } else if let Some(ti) = tool_input {
            let s = serde_json::to_string(ti).unwrap_or_default();
            if s.len() > 140 {
                format!("{}…", &s[..140])
            } else {
                s
            }
        } else {
            String::new()
        };

        let description = format!("请求执行「{}」", tool_name);
        return Some(HookRequest {
            questions: Vec::new(),
            permission: Some(PermissionPrompt {
                tool_name: tool_name.to_string(),
                command_or_arg: sanitize_payload_text(&command_or_arg),
                description,
            }),
            agent,
        });
    }

    None
}

/// Maps a committed nod count onto a reply. Nods within the option range pick
/// that option; one nod past the end means the wearer wants "Other" or would
/// rather answer by hand; anything more is treated as a mistake.
#[allow(dead_code)]
fn decision_for_commit(count: usize, options: &[String]) -> AskReply {
    match count {
        n if n >= 1 && n <= options.len() => AskReply::Select {
            index: n - 1,
            label: options[n - 1].clone(),
        },
        _ => AskReply::Pass,
    }
}

/// Renders the hook's stdout JSON for a reply.
///
/// The selection is an `allow` whose `updatedInput` injects the choice as the
/// tool's `answers` field while preserving the original `tool_input` (`questions`,
/// etc.): ZCode replaces `tool_input` wholesale with `updatedInput` and
/// re-validates it against AskUserQuestion's strict `inputSchema` (which requires
/// `questions`). Sending only `{ "answers": ... }` fails `inputSchema`
/// validation (live, 2026-11-12). Allowing with the full input + `answers` is
/// what the native widget produces on an accept, so the question renders as
/// answered and the reason tells the agent which choice was made. A pass-through
/// is `ask`, which restores the normal ZCode question UI.
fn hook_output_for(
    reply: &AskReply,
    request: &HookRequest,
    tool_input: Option<&serde_json::Value>,
    is_codex: bool,
) -> String {
    match reply {
        AskReply::Select { index, label } => {
            let note = format!(
                "【头部手势应答】用户点头 {} 次,选择第 {} 项「{}」。(RustPods 通过点头手势代答;\
                 答案已提供,直接继续即可;\
                 并在下一条回复的第一句向用户复述这个选择,例如「已按你的点头选择:第 {} 项「{}」。」)",
                index + 1,
                index + 1,
                label,
                index + 1,
                label
            );
            if is_codex {
                json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "deny",
                        "permissionDecisionReason": note,
                        "additionalContext": note,
                    }
                })
                .to_string()
            } else {
                let raw_q = tool_input
                    .and_then(|t| t.get("questions"))
                    .and_then(|qs| qs.get(0));
                let answer_key = raw_q
                    .and_then(|q| q.get("question").or_else(|| q.get("title")))
                    .and_then(|v| v.as_str())
                    .or_else(|| tool_input.and_then(|t| t.get("question")).and_then(|v| v.as_str()))
                    .unwrap_or(request.questions.first().map(|q| q.question.as_str()).unwrap_or(""));
                let answer_val = raw_q
                    .and_then(|q| q.get("options"))
                    .or_else(|| tool_input.and_then(|t| t.get("options")))
                    .and_then(|opts| opts.get(*index))
                    .and_then(|opt| {
                        opt.as_str()
                            .or_else(|| opt.get("label").and_then(|l| l.as_str()))
                    })
                    .unwrap_or(label.as_str());
                let answers = json!({ answer_key: answer_val });
                let updated_input = match tool_input.and_then(|v| v.as_object()) {
                    Some(obj) => {
                        let mut merged = obj.clone();
                        merged.insert("answers".into(), answers);
                        merged.insert("answer".into(), json!(answer_val));
                        serde_json::Value::Object(merged)
                    }
                    None => json!({ "answers": answers, "answer": answer_val }),
                };
                json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "allow",
                        "permissionDecisionReason": note,
                        "additionalContext": note,
                        "updatedInput": updated_input,
                    }
                })
                .to_string()
            }
        }
        AskReply::BatchSelect { selections } => {
            let mut note_items = Vec::new();
            let mut answers_map = serde_json::Map::new();
            let raw_qs = tool_input
                .and_then(|t| t.get("questions"))
                .and_then(|qs| qs.as_array());

            for (i, sel) in selections.iter().enumerate() {
                let q_info = request.questions.get(i);
                let q_title = q_info
                    .map(|q| {
                        if !q.header.is_empty() {
                            q.header.as_str()
                        } else {
                            q.question.as_str()
                        }
                    })
                    .unwrap_or("问题");
                note_items.push(format!("{}.「{}」选择第 {} 项「{}」", i + 1, q_title, sel.index + 1, sel.label));

                let answer_key = raw_qs
                    .and_then(|qs| qs.get(i))
                    .and_then(|raw_q| {
                        raw_q.get("question")
                            .or_else(|| raw_q.get("title"))
                            .and_then(|v| v.as_str())
                    })
                    .unwrap_or(q_info.map(|q| q.question.as_str()).unwrap_or(""));
                answers_map.insert(answer_key.to_string(), json!(sel.label));
            }

            let note = format!(
                "【头部手势应答】用户点头完成了 {} 道选择题：{}。(RustPods 通过点头手势代答;\
                 答案已提供,直接继续即可;\
                 并在下一条回复的第一句向用户复述这些选择。)",
                selections.len(),
                note_items.join("；")
            );

            if is_codex {
                json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "deny",
                        "permissionDecisionReason": note,
                        "additionalContext": note,
                    }
                })
                .to_string()
            } else {
                let answers = serde_json::Value::Object(answers_map);
                let updated_input = match tool_input.and_then(|v| v.as_object()) {
                    Some(obj) => {
                        let mut merged = obj.clone();
                        merged.insert("answers".into(), answers);
                        if let Some(first) = selections.first() {
                            merged.insert("answer".into(), json!(first.label));
                        }
                        serde_json::Value::Object(merged)
                    }
                    None => json!({ "answers": answers }),
                };
                json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "allow",
                        "permissionDecisionReason": note,
                        "additionalContext": note,
                        "updatedInput": updated_input,
                    }
                })
                .to_string()
            }
        }
        AskReply::Allow => {
            let tool_name = request
                .permission
                .as_ref()
                .map(|p| p.tool_name.as_str())
                .unwrap_or("工具");
            let note = format!(
                "【头部手势应答】用户点头确认，允许执行「{}」。(RustPods 通过点头手势授权)",
                tool_name
            );
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "allow",
                    "permissionDecisionReason": note,
                    "additionalContext": note,
                }
            })
            .to_string()
        }
        AskReply::Deny => {
            let tool_name = request
                .permission
                .as_ref()
                .map(|p| p.tool_name.as_str())
                .unwrap_or("工具");
            let note = format!(
                "【头部手势应答】用户摇头拒绝执行「{}」。(RustPods 通过摇头手势拒绝; 请向用户汇报操作已被取消并询问下一步建议)",
                tool_name
            );
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": note,
                    "additionalContext": note,
                }
            })
            .to_string()
        }
        AskReply::Pass => {
            if request.permission.is_some() {
                json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "ask",
                    }
                })
                .to_string()
            } else if is_codex {
                json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "allow",
                    }
                })
                .to_string()
            } else {
                json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "ask",
                    }
                })
                .to_string()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Hub: shared state between the socket server and the pose-driven task
// ---------------------------------------------------------------------------

static NEXT_ASK_ID: AtomicU64 = AtomicU64::new(1);

/// What the UI card shows while a question or permission is pending.
#[derive(Debug, Clone, PartialEq)]
pub struct AskProgress {
    pub id: u64,
    pub agent: String,
    pub question: String,
    pub header: String,
    pub options: Vec<String>,
    pub count: usize,
    pub permission: Option<PermissionPrompt>,
    pub decision_preview: Option<bool>,
}

/// What the UI shows briefly once a question has been answered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedNote {
    pub text: String,
    pub agent: String,
}

struct PendingAsk {
    id: u64,
    reply: oneshot::Sender<AskReply>,
}

/// Process-wide coordination point for pending questions.
///
/// The socket server registers questions, the pose task resolves them, and
/// the GTK card polls snapshots. All locks are short and uncontended, so a
/// plain `std` mutex is fine even on the async side.
pub struct AskHub {
    pending: Mutex<Option<PendingAsk>>,
    progress: Mutex<Option<AskProgress>>,
    resolved: Mutex<Option<(ResolvedNote, Instant)>>,
    last_pose: Mutex<Option<Instant>>,
    notify: tokio::sync::Notify,
}

pub static ASK_HUB: LazyLock<AskHub> = LazyLock::new(AskHub::new);

impl AskHub {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(None),
            progress: Mutex::new(None),
            resolved: Mutex::new(None),
            last_pose: Mutex::new(None),
            notify: tokio::sync::Notify::new(),
        }
    }

    /// Registers a question for gesture answering. Returns `false` if another
    /// question is already mid-flight - rare, and pass-through is the safe
    /// reply for the loser.
    fn register(&self, progress: AskProgress, reply: oneshot::Sender<AskReply>) -> bool {
        let mut pending = self.pending.lock().unwrap();
        if pending.is_some() {
            return false;
        }
        let id = progress.id;
        *self.progress.lock().unwrap() = Some(progress);
        *pending = Some(PendingAsk { id, reply });
        self.notify.notify_waiters();
        true
    }

    /// Takes the pending question with the given id, if it is still the
    /// current one.
    fn take_pending(&self, id: u64) -> Option<PendingAsk> {
        let mut pending = self.pending.lock().unwrap();
        match pending.as_ref() {
            Some(p) if p.id == id => pending.take(),
            _ => None,
        }
    }

    /// Drops a registration the gesture task never resolved, so the next
    /// question can register again.
    fn abandon(&self, id: u64) {
        drop(self.take_pending(id));
        self.clear_progress(id);
        self.notify.notify_waiters();
    }

    /// Records a nod count for the UI.
    #[allow(dead_code)]
    fn set_count(&self, id: u64, count: usize) {
        let mut progress = self.progress.lock().unwrap();
        if let Some(p) = progress.as_mut()
            && p.id == id {
                p.count = count;
                self.notify.notify_waiters();
            }
    }

    fn clear_progress(&self, id: u64) {
        let mut progress = self.progress.lock().unwrap();
        if progress.as_ref().is_some_and(|p| p.id == id) {
            *progress = None;
            self.notify.notify_waiters();
        }
    }

    /// Waits for an update event on the hub (question arrival, count change, resolution).
    pub async fn notified(&self) {
        self.notify.notified().await;
    }

    /// Programmatically select an option by index (e.g. from popup click).
    #[allow(dead_code)]
    pub fn resolve_by_index(&self, index: usize) {
        let (id, label, agent) = {
            let progress = self.progress.lock().unwrap();
            let Some(p) = progress.as_ref() else { return };
            if index >= p.options.len() { return };
            (p.id, p.options[index].clone(), p.agent.clone())
        };
        if let Some(pending) = self.take_pending(id) {
            let note = format!("已选择第 {} 项「{}」", index + 1, label);
            let _ = pending.reply.send(AskReply::Select { index, label });
            self.mark_resolved(note, agent);
            self.clear_progress(id);
        }
    }

    /// Programmatically resolve a permission inquiry (e.g. from popup button click or gesture).
    pub fn resolve_permission(&self, allow: bool) {
        let (id, tool_name, agent) = {
            let progress = self.progress.lock().unwrap();
            let Some(p) = progress.as_ref() else { return };
            let tool = p
                .permission
                .as_ref()
                .map(|perm| perm.tool_name.clone())
                .unwrap_or_else(|| "工具".into());
            (p.id, tool, p.agent.clone())
        };
        if let Some(pending) = self.take_pending(id) {
            let (reply, note) = if allow {
                (AskReply::Allow, format!("已允许执行「{}」", tool_name))
            } else {
                (AskReply::Deny, format!("已拒绝执行「{}」", tool_name))
            };
            let _ = pending.reply.send(reply);
            self.mark_resolved(note, agent);
            self.clear_progress(id);
        }
    }

    /// Records a decision preview (allow/deny) for immediate UI feedback.
    pub fn set_decision_preview(&self, id: u64, allow: bool) {
        let mut progress = self.progress.lock().unwrap();
        if let Some(p) = progress.as_mut()
            && p.id == id
        {
            p.decision_preview = Some(allow);
            self.notify.notify_waiters();
        }
    }

    /// Cancels the current question.
    pub fn cancel(&self) {
        let id = {
            let progress = self.progress.lock().unwrap();
            progress.as_ref().map(|p| p.id)
        };
        if let Some(id) = id
            && let Some(pending) = self.take_pending(id) {
                let _ = pending.reply.send(AskReply::Pass);
                self.clear_progress(id);
            }
    }

    /// Snapshot for the UI card.
    pub fn progress_snapshot(&self) -> Option<AskProgress> {
        self.progress.lock().unwrap().clone()
    }

    /// Marks a choice as made, so the UI can confirm it briefly.
    pub fn mark_resolved(&self, text: String, agent: String) {
        *self.resolved.lock().unwrap() = Some((ResolvedNote { text, agent }, Instant::now()));
        self.notify.notify_waiters();
    }

    /// The confirmation text, if one was made recently enough to still show.
    pub fn resolved_note(&self) -> Option<ResolvedNote> {
        let resolved = self.resolved.lock().unwrap();
        resolved
            .as_ref()
            .filter(|(_, at)| at.elapsed() < Duration::from_secs(3))
            .map(|(note, _)| note.clone())
    }

    /// Records that a pose sample just arrived.
    pub fn note_pose(&self) {
        *self.last_pose.lock().unwrap() = Some(Instant::now());
    }

    /// Marks the pose stream as gone (disconnect sentinel), so freshness
    /// checks fail immediately instead of up to one window later.
    pub fn retire_pose(&self) {
        *self.last_pose.lock().unwrap() = None;
    }

    /// Whether the pose stream is alive right now.
    pub fn tracking_fresh(&self) -> bool {
        self.last_pose
            .lock()
            .unwrap()
            .is_some_and(|at| at.elapsed() < TRACKING_FRESH_AFTER)
    }
}

impl Default for AskHub {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Socket server (runs inside RustPods)
// ---------------------------------------------------------------------------

/// Where the hook process finds the running RustPods instance.
///
pub const WINDOWS_PIPE_NAME: &str = r"\\.\pipe\rustpods-zcode-ask";

/// Falls back to the temp directory when `XDG_RUNTIME_DIR` is unset or
/// empty; the server side refuses to serve there (see `run_socket_server`),
/// so the fallback only ever produces a connect-fail on the client, which
/// fails open.
#[cfg(target_os = "linux")]
pub fn socket_path() -> PathBuf {
    let dir = std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    dir.join("rustpods").join("zcode-ask.sock")
}

#[cfg(target_os = "windows")]
pub fn socket_path() -> PathBuf {
    PathBuf::from(WINDOWS_PIPE_NAME)
}

#[cfg(target_os = "linux")]
async fn connect_to_daemon() -> Result<tokio::net::UnixStream, std::io::Error> {
    tokio::net::UnixStream::connect(socket_path()).await
}

#[cfg(target_os = "windows")]
async fn connect_to_daemon() -> Result<tokio::net::windows::named_pipe::NamedPipeClient, std::io::Error> {
    tokio::net::windows::named_pipe::ClientOptions::new().open(WINDOWS_PIPE_NAME)
}

/// The per-user runtime directory, if this session has a proper one.
#[cfg(target_os = "linux")]
fn runtime_dir() -> Option<PathBuf> {
    std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

/// Serves hook connections until the process exits.
///
/// Failing to bind is not an error: another RustPods instance owns the socket
/// and will answer instead. A stale socket left behind by a crash is removed
/// and rebound once.
#[cfg(target_os = "linux")]
pub async fn run_socket_server() {
    // A predictable world-readable path under /tmp would let any local user
    // pre-bind it (this instance would then silently never serve) or post
    // questions into it. Without a per-user runtime dir, fail open: hooks
    // pass through and the question UI stays as it was.
    let Some(base) = runtime_dir() else {
        warn!(
            "XDG_RUNTIME_DIR is not set; not serving the zcode-ask socket, \
             agent questions will pass through"
        );
        return;
    };
    let path = base.join("rustpods").join("zcode-ask.sock");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let listener = match tokio::net::UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(e) if e.kind() == ErrorKind::AddrInUse => {
            // Alive owner or stale file? Connecting tells the two apart.
            if tokio::net::UnixStream::connect(&path).await.is_ok() {
                info!(
                    "zcode-ask socket is owned by another RustPods instance; \
                     not starting a second server"
                );
                return;
            }
            let _ = std::fs::remove_file(&path);
            match tokio::net::UnixListener::bind(&path) {
                Ok(listener) => {
                    info!("Rebound stale zcode-ask socket at {}", path.display());
                    listener
                }
                Err(e) => {
                    warn!("Could not bind zcode-ask socket {}: {}", path.display(), e);
                    return;
                }
            }
        }
        Err(e) => {
            warn!("Could not bind zcode-ask socket {}: {}", path.display(), e);
            return;
        }
    };

    info!("Listening for ZCode questions on {}", path.display());
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(handle_hook_connection(stream));
            }
            Err(e) => {
                warn!("zcode-ask accept failed: {}", e);
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub async fn run_socket_server() {
    use tokio::net::windows::named_pipe::ServerOptions;

    info!("Listening for ZCode questions on Windows named pipe {}", WINDOWS_PIPE_NAME);
    let mut server = match ServerOptions::new()
        .first_pipe_instance(true)
        .create(WINDOWS_PIPE_NAME)
    {
        Ok(s) => s,
        Err(e) => {
            warn!("Could not create named pipe {}: {}", WINDOWS_PIPE_NAME, e);
            return;
        }
    };

    loop {
        if server.connect().await.is_ok() {
            let connected_client = server;
            server = match ServerOptions::new().create(WINDOWS_PIPE_NAME) {
                Ok(s) => s,
                Err(e) => {
                    warn!("Could not create next named pipe instance: {}", e);
                    break;
                }
            };
            tokio::spawn(handle_hook_connection(connected_client));
        } else {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// Handles one hook connection: read the request, register the question, wait
/// for the gesture task, reply. Every early exit is a pass-through.
async fn handle_hook_connection<S>(mut stream: S)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let reply = ask_from_stream(&mut stream).await.unwrap_or(AskReply::Pass);
    let mut line = serde_json::to_string(&reply).unwrap_or_else(|_| "{\"event\":\"pass\"}".into());
    line.push('\n');
    if let Err(e) = stream.write_all(line.as_bytes()).await {
        debug!("zcode-ask reply write failed: {}", e);
    }
    let _ = stream.flush().await;
}

/// Reads a question from the hook, shepherds it through the hub, and returns
/// the decision. `None` means the payload was unusable.
async fn ask_from_stream<S>(stream: &mut S) -> Option<AskReply>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    // One line, capped and time-bounded. `take` guarantees a hostile writer
    // cannot balloon memory; a client that connects and never speaks cannot
    // park this task forever.
    let mut buf = Vec::new();
    let mut reader = BufReader::new(stream.take(MAX_PAYLOAD_BYTES));
    tokio::time::timeout(REQUEST_READ_TIMEOUT, reader.read_until(b'\n', &mut buf))
        .await
        .ok()?
        .ok()?;
    if buf.is_empty() {
        return None;
    }
    let request: HookRequest = if let Ok(req) = serde_json::from_slice::<HookRequest>(&buf) {
        if req.questions.is_empty() && req.permission.is_none() {
            let value: serde_json::Value = serde_json::from_slice(&buf).ok()?;
            parse_questions(&value)?
        } else {
            req
        }
    } else {
        let value: serde_json::Value = serde_json::from_slice(&buf).ok()?;
        parse_questions(&value)?
    };
    Some(ask_core(request).await)
}

/// The shared permission pipeline: validate, gate, register, wait for the
/// gesture task. Both the socket handler and the settings page's test button
/// come through here, so a test exercises exactly what a live permission does.
async fn ask_core(request: HookRequest) -> AskReply {
    // Only sensitive tool permissions are answered via head gestures;
    // general questions pass through directly to the terminal / native UI.
    let Some(perm) = request.permission else {
        return AskReply::Pass;
    };

    // The wearer is not streaming (buds off, not connected): do not pretend a
    // gesture is coming, hand the permission request to the normal UI immediately.
    if !ASK_HUB.tracking_fresh() {
        info!("Permission request passed through: head tracking is not active");
        return AskReply::Pass;
    }

    let cfg = crate::system::config::load_config();
    // Feature toggle lives in the config file; re-reading it here means the
    // switch takes effect without a restart.
    if !cfg.gesture_permission_answers_enabled || !cfg.head_gesture_answers_enabled {
        info!("Permission request passed through: gesture permission answers disabled in config");
        return AskReply::Pass;
    }

    let id = NEXT_ASK_ID.fetch_add(1, Ordering::SeqCst);
    let progress = AskProgress {
        id,
        agent: request.agent.clone(),
        question: perm.description.clone(),
        header: perm.tool_name.clone(),
        options: vec!["允许".into(), "拒绝".into()],
        count: 0,
        permission: Some(perm.clone()),
        decision_preview: None,
    };
    let (tx, rx) = oneshot::channel();
    if !ASK_HUB.register(progress, tx) {
        return AskReply::Pass;
    }
    info!(
        "Permission pending: tool {:?} (id {}, sensitivity {})",
        perm.tool_name,
        id,
        crate::core::nod_counter::sensitivity()
    );

    match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
        Ok(Ok(reply)) => {
            info!("Permission {} resolved: {:?}", id, reply);
            ASK_HUB.clear_progress(id);
            reply
        }
        _ => {
            warn!(
                "Permission {} timed out; passing through (tracking_fresh={})",
                id,
                ASK_HUB.tracking_fresh()
            );
            ASK_HUB.abandon(id);
            AskReply::Pass
        }
    }
}

/// The sample question the test button and the CLI simulator use.
pub fn sample_question_request(agent: &str) -> HookRequest {
    HookRequest {
        questions: vec![HookQuestion {
            question: "模拟提问:这是一个用于测试点头应答的示例问题".into(),
            header: "测试".into(),
            options: vec!["第一个选项".into(), "第二个选项".into(), "第三个选项".into()],
            multi_select: false,
        }],
        permission: None,
        agent: if agent.trim().is_empty() {
            "Agent".into()
        } else {
            agent.into()
        },
    }
}

/// The sample permission request the test button and CLI simulator use.
pub fn sample_permission_request(agent: &str) -> HookRequest {
    HookRequest {
        questions: Vec::new(),
        permission: Some(PermissionPrompt {
            tool_name: "Bash".into(),
            command_or_arg: "cargo test --workspace".into(),
            description: "请求执行命令「cargo test --workspace」".into(),
        }),
        agent: if agent.trim().is_empty() {
            "Agent".into()
        } else {
            agent.into()
        },
    }
}

/// Pushes the sample permission request through the real answering pipeline from inside
/// the process. Returns the decision once the wearer nods, shakes, cancels, or the
/// permission passes through (not worn, not installed, timed out).
pub async fn run_test_question(agent: &str) -> Result<AskReply, String> {
    run_test_permission(agent).await
}

/// Pushes the sample permission request through the real answering pipeline.
pub async fn run_test_permission(agent: &str) -> Result<AskReply, String> {
    Ok(ask_core(sample_permission_request(agent)).await)
}

// ---------------------------------------------------------------------------
// Pose-driven answering task (runs inside RustPods, one per AACP session)
// ---------------------------------------------------------------------------

/// Generation counter so a reconnected AACP session retires the previous
/// task instead of two counters racing on the same question.
static ASK_TASK_GEN: AtomicU64 = AtomicU64::new(0);

/// Feeds head poses into the nod counter and resolves pending questions.
///
/// Spawned alongside `run_audio_follow` per connection; exits when a newer
/// connection spawns its own task or the pose stream closes.
pub async fn run_question_answers(mut pose_rx: watch::Receiver<Option<HeadPose>>) {
    let task_gen = ASK_TASK_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let mut cur_sensitivity = crate::core::nod_counter::sensitivity();
    let mut counter = NodCounter::with_sensitivity(cur_sensitivity);
    let mut last_sample: Option<Instant> = None;
    let mut was_answering = false;
    let mut last_telemetry: Option<Instant> = None;

    loop {
        if pose_rx.changed().await.is_err() {
            return;
        }
        if ASK_TASK_GEN.load(Ordering::SeqCst) != task_gen {
            debug!("A newer head-gesture task took over; retiring this one");
            return;
        }
        // A sensitivity change takes effect at the next question, not the
        // next restart: rebuild the counter, resetting any partial count
        // with it (changing thresholds mid-sequence would strand it).
        let sensitivity = crate::core::nod_counter::sensitivity();
        if sensitivity != cur_sensitivity {
            cur_sensitivity = sensitivity;
            counter = NodCounter::with_sensitivity(sensitivity);
        }
        let now = Instant::now();
        let dt = last_sample.replace(now).map_or(0.0, |t| now.duration_since(t).as_secs_f32());
        let Some(pose) = *pose_rx.borrow_and_update() else {
            // Disconnect sentinel: expire the freshness stamp, so a question
            // arriving right after a drop passes through instead of
            // registering for gestures nothing can answer.
            ASK_HUB.retire_pose();
            counter.reset();
            was_answering = false;
            continue;
        };
        ASK_HUB.note_pose();

        let Some(progress) = ASK_HUB.progress_snapshot() else {
            // Nothing pending: keep the resting pose warm instead of
            // resetting. A question that registers while the wearer is
            // moving - or seconds after they stopped - then meets a
            // baseline already sitting on their rest pose, rather than one
            // seeded from the single sample that happened to be current at
            // registration.
            if was_answering {
                counter.reset_count();
                was_answering = false;
            }
            counter.observe_idle(pose.pitch, pose.yaw, dt);
            continue;
        };
        was_answering = true;

        // A pending question that never counts is undiagnosable from the
        // outside - live it has happened twice (2026-11-09, ids 7 and the
        // post-fix retest). The heartbeat shows the motion the counter
        // actually saw against the thresholds it actually uses: a nod must
        // dip below `baseline - depth` (6° at sensitivity 3) and return
        // above `baseline - depth/3`.
        if last_telemetry.is_none_or(|t| t.elapsed() >= TELEMETRY_EVERY) {
            last_telemetry = Some(now);
            let (min_dev, max_yaw) = counter.take_telemetry();
            let (baseline, pitch_swing, count, down) = counter.snapshot();
            info!(
                "Q{} telemetry: min(pitch-baseline) {:+.1}°, yaw swing {:.1}°, baseline {:+.1}°, pitch swing {:.1}°, count {}, down {}",
                progress.id,
                min_dev,
                max_yaw,
                baseline.unwrap_or(f32::NAN),
                pitch_swing,
                count,
                down
            );
        }

        let Some(event) = counter.update(pose.pitch, pose.yaw, dt) else {
            continue;
        };

        let reply = match event {
            NodEvent::Counted(_) | NodEvent::Commit(_) => {
                info!("Nod detected for permission request {}", progress.id);
                ASK_HUB.set_decision_preview(progress.id, true);
                AskReply::Allow
            }
            NodEvent::Shaken | NodEvent::Cancelled => {
                info!("Head shake/turn detected for permission request {}", progress.id);
                ASK_HUB.set_decision_preview(progress.id, false);
                AskReply::Deny
            }
        };

        resolve_question(progress.id, reply);
        // Drop the gesture state but keep the tracked resting pose: the
        // wearer is mid-motion right now, and a full reset would re-seed
        // the baseline from exactly that.
        counter.reset_count();
    }
}

/// Delivers a decision for the pending question, if it is still ours to
/// answer, and tidies the shared state.
fn resolve_question(id: u64, reply: AskReply) {
    let (agent_name, perm) = ASK_HUB
        .progress_snapshot()
        .map(|p| (p.agent, p.permission))
        .unwrap_or_else(|| ("Agent".to_string(), None));
    let note = match &reply {
        AskReply::Select { index, label } => format!("✓ 已选择第 {} 项「{}」", index + 1, label),
        AskReply::BatchSelect { selections } => {
            let last = selections.last().map(|s| s.label.as_str()).unwrap_or("");
            format!("✓ 全部完成「{}」", last)
        }
        AskReply::Allow => {
            let tool = perm.as_ref().map(|p| p.tool_name.as_str()).unwrap_or("工具");
            format!("✓ 已允许执行「{}」", tool)
        }
        AskReply::Deny => {
            let tool = perm.as_ref().map(|p| p.tool_name.as_str()).unwrap_or("工具");
            format!("✗ 已拒绝执行「{}」", tool)
        }
        AskReply::Pass => format!("已取消,请在 {} 中手动操作", agent_name),
    };
    if let Some(pending) = ASK_HUB.take_pending(id) {
        if pending.reply.send(reply).is_ok() {
            ASK_HUB.mark_resolved(note, agent_name);
        } else {
            // The server timed out first and already answered pass-through:
            // the choice is moot, so the card must not claim it was applied.
            ASK_HUB.clear_progress(id);
        }
    } else {
        // The server timed out first; drop the card so it does not linger.
        ASK_HUB.clear_progress(id);
    }
}

// ---------------------------------------------------------------------------
// Hook process (`rustpods --zcode-ask-hook` or `rustpods --codex-ask-hook`)
// ---------------------------------------------------------------------------

/// Entry point of the agent hook command.
///
/// Reads the hook payload from stdin, asks RustPods for a decision, and prints
/// the hook output JSON. Any failure exits 0 with no output, which the host agent
/// treats as "hook passed, carry on normally".
pub async fn run_hook_mode() -> i32 {
    let payload = match read_stdin_with_timeout().await {
        Some(payload) => payload,
        None => return 0,
    };
    let value: serde_json::Value = match serde_json::from_str(&payload) {
        Ok(value) => value,
        Err(e) => {
            debug!("ask-hook: unreadable payload: {}", e);
            return 0;
        }
    };
    let Some(request) = parse_questions(&value) else {
        return 0;
    };
    if request.permission.is_none() {
        // Only tool permissions are answered by gestures; questions pass through directly
        return 0;
    }

    let tool_name = value
        .get("tool_name")
        .and_then(|t| t.as_str())
        .unwrap_or_default();
    let is_codex = std::env::args().any(|a| a == crate::system::codex_ask::HOOK_ARG_MARKER)
        || tool_name.contains("request_user_input");
    let is_zcode = std::env::args().any(|a| a == crate::system::zcode_ask::HOOK_ARG_MARKER)
        || tool_name.contains("AskUserQuestion")
        || tool_name.eq_ignore_ascii_case("askuserquestion");

    let mut request = request;
    if is_codex {
        request.agent = "Codex".to_string();
    } else if is_zcode {
        request.agent = "ZCode".to_string();
    } else if request.agent.is_empty() || request.agent == "Agent" {
        request.agent = "Agent".to_string();
    }

    debug!(
        "ask-hook: forwarding request (questions={}, has_permission={}, is_codex={})",
        request.questions.len(),
        request.permission.is_some(),
        is_codex
    );

    let mut stream = match tokio::time::timeout(HOOK_CONNECT_TIMEOUT, connect_to_daemon()).await {
        Ok(Ok(stream)) => stream,
        _ => {
            debug!("ask-hook: RustPods socket not reachable; passing through");
            return 0;
        }
    };

    let mut line = match serde_json::to_string(&request) {
        Ok(line) => line,
        Err(_) => return 0,
    };
    line.push('\n');
    if stream.write_all(line.as_bytes()).await.is_err() || stream.flush().await.is_err() {
        return 0;
    }

    let mut buf = Vec::new();
    let read_reply = async {
        let mut reader = BufReader::new(&mut stream);
        reader.read_until(b'\n', &mut buf).await
    };
    match tokio::time::timeout(HOOK_REPLY_TIMEOUT, read_reply).await {
        Ok(Ok(0)) | Ok(Err(_)) | Err(_) => {
            warn!("ask-hook: no reply from RustPods; passing through");
            return 0;
        }
        Ok(Ok(_)) => {}
    }

    let reply: AskReply = match serde_json::from_slice(&buf) {
        Ok(reply) => reply,
        Err(e) => {
            warn!("ask-hook: undecipherable reply: {}", e);
            return 0;
        }
    };

    let tool_input = value
        .get("tool_input")
        .or_else(|| value.get("questions").map(|_| &value));
    println!(
        "{}",
        hook_output_for(&reply, &request, tool_input, is_codex)
    );
    0
}

/// Stdin arrives with the payload and should close right away; a timeout here
/// means something is wrong upstream and the hook must not block ZCode.
async fn read_stdin_with_timeout() -> Option<String> {
    let mut buf = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(10), async {
        // Framed through `take` so an unbounded payload cannot exhaust memory.
        tokio::io::stdin()
            .take(MAX_PAYLOAD_BYTES)
            .read_to_end(&mut buf)
            .await
    });
    match read.await {
        Ok(Ok(_)) => String::from_utf8(buf).ok(),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Hook installation
// ---------------------------------------------------------------------------

/// Path of the user-level ZCode CLI config.
fn zcode_cli_config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".zcode").join("cli").join("config.json")
}

/// The hook entry RustPods installs, as a reusable value: a `process` hook
/// (argv vector, no shell). ZCode **drops** a hook outright when `args` is
/// mixed into a `type: "command"` entry - the first shipped version did
/// exactly that and the feature silently never engaged - so the entry is
/// always written back in this canonical shape on reinstall.
fn canonical_hook_entry(command: &str) -> serde_json::Value {
    json!([{
        "type": "process",
        "command": command,
        "args": [HOOK_ARG_MARKER],
        "timeoutMs": 180000,
        "onError": "allow",
    }])
}

/// Matcher string targeting sensitive permission tools in ZCode.
pub const HOOK_MATCHER: &str =
    "Bash|Write|FileEdit|edit_file|execute_command";

/// Merges our PreToolUse hook entry into a ZCode config document.
///
/// Returns whether anything changed, so repeated installs are no-ops, and an
/// entry installed by an older binary path is repointed rather than
/// duplicated. Errors out instead of improvising when the document has an
/// unexpected shape: an install must never mangle a config another tool
/// wrote.
pub fn install_hook_into(config: &mut serde_json::Value, command: &str) -> Result<bool, String> {
    let Some(root) = config.as_object_mut() else {
        return Err("config 根节点不是 JSON 对象".to_string());
    };
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| "`hooks` 已存在且不是对象;请先手工修正 ~/.zcode/cli/config.json".to_string())?;

    // Config-file hooks are inert unless enabled. Re-enabling is required for
    // the feature to run at all, but a deliberate `false` is the user's
    // global kill switch for every hook - refuse rather than flip it back on
    // behind their back.
    let mut changed = false;
    match hooks.get("enabled") {
        Some(serde_json::Value::Bool(false)) => {
            return Err(
                "hooks.enabled 为 false:配置文件 hooks 已被整体停用,请先在 ZCode 侧启用再安装"
                    .to_string(),
            );
        }
        Some(serde_json::Value::Bool(true)) => {}
        _ => {
            hooks.insert("enabled".into(), json!(true));
            changed = true;
        }
    }

    let events = hooks
        .entry("events")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| "`hooks.events` 不是对象".to_string())?;
    let pre = events
        .entry("PreToolUse")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| "`hooks.events.PreToolUse` 不是数组".to_string())?;

    let canonical = canonical_hook_entry(command);
    if let Some(existing) = pre.iter_mut().find(|e| entry_is_ours(e)) {
        if existing.get("matcher").and_then(|m| m.as_str()) != Some(HOOK_MATCHER) {
            existing["matcher"] = json!(HOOK_MATCHER);
            changed = true;
        }
        if existing.get("hooks") != Some(&canonical) {
            existing["hooks"] = canonical;
            changed = true;
        }
        return Ok(changed);
    }

    pre.push(json!({
        "matcher": HOOK_MATCHER,
        "hooks": canonical,
    }));
    Ok(true)
}

/// Removes our hook entry; returns whether anything changed.
///
/// Matching is by the argv marker, so uninstalling from a different binary
/// path than the one that installed still works.
pub fn uninstall_hook_from(config: &mut serde_json::Value) -> bool {
    let Some(root) = config.as_object_mut() else {
        return false;
    };
    let Some(hooks) = root.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return false;
    };
    let Some(events) = hooks.get_mut("events").and_then(|e| e.as_object_mut()) else {
        return false;
    };
    let Some(pre) = events.get_mut("PreToolUse").and_then(|p| p.as_array_mut()) else {
        return false;
    };

    let before = pre.len();
    pre.retain(|entry| !entry_is_ours(entry));
    if pre.len() == before {
        return false;
    }
    if pre.is_empty() {
        events.remove("PreToolUse");
    }
    if events.is_empty() {
        hooks.remove("events");
    }
    if hooks.len() == 1 && hooks.contains_key("enabled") {
        root.remove("hooks");
    }
    true
}

fn hook_command_path() -> Result<String, String> {
    std::env::current_exe()
        .map_err(|e| format!("could not locate the running binary: {}", e))
        .map(|p| p.to_string_lossy().into_owned())
}

/// The argv marker that identifies our hook entry, independent of where the
/// binary happens to live: a rebuild or a moved checkout must update the
/// installed entry, not duplicate it.
const HOOK_ARG_MARKER: &str = "--zcode-ask-hook";

/// Whether a PreToolUse entry is ours (matching our matcher and marker).
fn entry_is_ours(entry: &serde_json::Value) -> bool {
    let is_matcher = entry
        .get("matcher")
        .and_then(|m| m.as_str())
        .is_some_and(|m| {
            m == HOOK_MATCHER
                || m == "AskUserQuestion"
                || m.contains("AskUserQuestion")
                || m.contains("Bash")
        });
    let has_marker = entry
        .get("hooks")
        .and_then(|h| h.as_array())
        .is_some_and(|hooks| {
            hooks.iter().any(|h| {
                h.get("args")
                    .and_then(|a| a.as_array())
                    .is_some_and(|args| {
                        args.iter().any(|a| a.as_str() == Some(HOOK_ARG_MARKER))
                    })
                    || h.get("command")
                        .and_then(|c| c.as_str())
                        .is_some_and(|c| c.contains(HOOK_ARG_MARKER))
            })
        });
    is_matcher && has_marker
}

/// The command of our hook inside an entry we own.
fn hook_command_of(entry: &serde_json::Value) -> Option<String> {
    entry
        .get("hooks")
        .and_then(|h| h.as_array())
        .and_then(|hooks| {
            hooks
                .iter()
                .filter(|h| {
                    h.get("args")
                        .and_then(|a| a.as_array())
                        .is_some_and(|args| {
                            args.iter().any(|a| a.as_str() == Some(HOOK_ARG_MARKER))
                        })
                })
                .find_map(|h| h.get("command").and_then(|c| c.as_str()))
        })
        .map(String::from)
}

/// Reads the install status out of a parsed ZCode config document.
pub fn hook_status_from(config: &serde_json::Value) -> HookInstallStatus {
    let pre = config
        .get("hooks")
        .and_then(|h| h.get("events"))
        .and_then(|e| e.get("PreToolUse"))
        .and_then(|p| p.as_array());
    match pre.and_then(|pre| pre.iter().find(|e| entry_is_ours(e)).and_then(hook_command_of)) {
        Some(cmd) => HookInstallStatus::Installed(cmd),
        None => HookInstallStatus::NotInstalled,
    }
}

/// Current install status, for the agent management UI.
pub fn hook_status() -> HookInstallStatus {
    match std::fs::read_to_string(zcode_cli_config_path()) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(value) => hook_status_from(&value),
            Err(e) => HookInstallStatus::ConfigUnreadable(e.to_string()),
        },
        // No config file means the agent was never configured at all.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => HookInstallStatus::NotInstalled,
        Err(e) => HookInstallStatus::ConfigUnreadable(e.to_string()),
    }
}

/// Writes ZCode's config through a temp file + rename, so a crash mid-write
/// cannot leave the agent's own config half-written.
fn write_zcode_config(path: &std::path::Path, pretty: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.rustpods-tmp");
    std::fs::write(&tmp, pretty).map_err(|e| format!("could not write {}: {}", tmp.display(), e))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("could not move {} into place: {}", tmp.display(), e)
    })?;
    Ok(())
}

/// Writes `~/.zcode/cli/config.json` with our hook merged in, backing the
/// previous file up first.
pub fn install_hook() -> Result<(), String> {
    let path = zcode_cli_config_path();
    let existing = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".to_string());
    let mut root: serde_json::Value = serde_json::from_str(&existing)
        .map_err(|e| format!("{} is not valid JSON ({}); fix it by hand first", path.display(), e))?;

    let command = hook_command_path()?;
    let changed = install_hook_into(&mut root, &command)?;
    if !changed {
        info!("ZCode hook already installed");
        return Ok(());
    }

    if path.exists() {
        let backup = path.with_extension("json.rustpods-bak");
        std::fs::copy(&path, &backup)
            .map_err(|e| format!("could not back up {} : {}", path.display(), e))?;
        info!("Backed up previous config to {}", backup.display());
    }
    let pretty = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    write_zcode_config(&path, &pretty)?;
    info!("Installed ZCode hook (command: {})", command);
    Ok(())
}

pub fn uninstall_hook() -> Result<(), String> {
    let path = zcode_cli_config_path();
    let existing = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".to_string());
    let mut root: serde_json::Value = serde_json::from_str(&existing)
        .map_err(|e| format!("{} is not valid JSON ({}); fix it by hand first", path.display(), e))?;
    if uninstall_hook_from(&mut root) {
        let pretty = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
        write_zcode_config(&path, &pretty)?;
        info!("Removed ZCode hook");
    } else {
        info!("No ZCode hook to remove");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Simulation (`rustpods --zcode-ask-simulate`)
// ---------------------------------------------------------------------------

/// Pushes a sample permission request through the socket, for testing the gesture path
/// without ZCode or Bluetooth.
pub async fn run_simulate() -> i32 {
    run_permission_simulate().await
}

/// Sends a sample permission request to the running RustPods daemon.
pub async fn run_permission_simulate() -> i32 {
    let request = sample_permission_request("Agent");

    let mut stream = match connect_to_daemon().await {
        Ok(stream) => stream,
        Err(e) => {
            error!(
                "Could not reach RustPods at {}: {} (is it running?)",
                socket_path().display(),
                e
            );
            return 1;
        }
    };
    let mut line = serde_json::to_string(&request).expect("serialise sample");
    line.push('\n');
    if let Err(e) = stream.write_all(line.as_bytes()).await {
        error!("Could not send the sample permission request: {}", e);
        return 1;
    }

    let mut buf = Vec::new();
    let read_reply = async {
        let mut reader = BufReader::new(&mut stream);
        reader.read_until(b'\n', &mut buf).await
    };
    match tokio::time::timeout(HOOK_REPLY_TIMEOUT, read_reply).await {
        Ok(Ok(_)) => match serde_json::from_slice::<AskReply>(&buf) {
            Ok(reply) => {
                info!("Simulated permission request resolved: {:?}", reply);
                0
            }
            Err(e) => {
                error!("Undecipherable reply: {}", e);
                1
            }
        },
        _ => {
            error!("No reply from RustPods");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn parse_questions_reads_the_pretooluse_envelope() {
        let payload: Value = serde_json::from_str(
            r#"{
                "hook_event_name": "PreToolUse",
                "tool_name": "AskUserQuestion",
                "tool_input": {
                    "questions": [{
                        "question": "选哪个方案?",
                        "header": "方案",
                        "options": [
                            {"label": "方案 A", "description": "快"},
                            {"label": "方案 B", "description": "稳"}
                        ],
                        "multiSelect": false
                    }]
                }
            }"#,
        )
        .unwrap();
        let request = parse_questions(&payload).unwrap();
        assert_eq!(request.questions.len(), 1);
        assert_eq!(request.questions[0].options, vec!["方案 A", "方案 B"]);
        assert!(!request.questions[0].multi_select);
    }

    #[test]
    fn parse_questions_accepts_bare_lists_and_plain_strings() {
        let payload: Value = serde_json::from_str(
            r#"{"questions": [
                {"question": "q", "options": ["一", "二"], "multiSelect": true}
            ]}"#,
        )
        .unwrap();
        let request = parse_questions(&payload).unwrap();
        assert_eq!(request.questions[0].options, vec!["一", "二"]);
        assert!(request.questions[0].multi_select);
    }

    #[test]
    fn parse_questions_rejects_payloads_without_questions() {
        let payload: Value = serde_json::json!({"tool_input": {"other": 1}});
        assert!(parse_questions(&payload).is_none());
    }

    #[test]
    fn commit_maps_onto_options_and_passes_beyond() {
        let options = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        assert_eq!(
            decision_for_commit(3, &options),
            AskReply::Select { index: 2, label: "C".into() }
        );
        // One past the end: the wearer wants "Other" or a manual answer.
        assert_eq!(decision_for_commit(4, &options), AskReply::Pass);
        assert_eq!(decision_for_commit(5, &options), AskReply::Pass);
        assert_eq!(decision_for_commit(0, &options), AskReply::Pass);
    }

    #[test]
    fn parse_questions_sanitizes_carriage_returns_and_blank_runs() {
        // Live: some payloads arrive with stray \r and runs of blank lines;
        // the question card rendered a stack of empty lines for them.
        let payload: Value = serde_json::from_str(
            r#"{"tool_input":{"questions":[{
                "question":"第一行\r\r\r第二行\n\n\n第三行",
                "options":["甲\r","乙\n\n"]
            }]}}"#,
        )
        .unwrap();
        let request = parse_questions(&payload).unwrap();
        assert_eq!(request.questions[0].question, "第一行\n第二行\n第三行");
        assert_eq!(request.questions[0].options, vec!["甲", "乙"]);
    }

    #[test]
    fn parse_questions_accepts_codex_single_question_shape() {
        let payload: Value = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "ask_question",
            "tool_input": {
                "question": "Which language?",
                "options": ["Rust", "Python", "Go"],
                "is_multi_select": false
            }
        });
        let req = parse_questions(&payload).unwrap();
        assert_eq!(req.questions.len(), 1);
        assert_eq!(req.questions[0].question, "Which language?");
        assert_eq!(req.questions[0].options, vec!["Rust", "Python", "Go"]);
        assert!(!req.questions[0].multi_select);
    }

    #[test]
    fn parse_questions_accepts_codex_request_user_input_async_with_title() {
        let payload: Value = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "request_user_input_async",
            "tool_input": {
                "questions": [{
                    "title": "如果现在有一小时空闲，你最想怎么放松？",
                    "options": ["听音乐", "看电影或剧", "出去走走", "和朋友聊天"]
                }]
            }
        });
        let req = parse_questions(&payload).unwrap();
        assert_eq!(req.questions.len(), 1);
        assert_eq!(req.agent, "Codex");
        assert_eq!(req.questions[0].question, "如果现在有一小时空闲，你最想怎么放松？");
        assert_eq!(req.questions[0].header, "如果现在有一小时空闲，你最想怎么放松？");
        assert_eq!(req.questions[0].options.len(), 4);
    }

    #[test]
    fn parse_questions_accepts_multi_questions() {
        let payload: Value = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "AskUserQuestion",
            "tool_input": {
                "questions": [
                    {
                        "question": "Q1",
                        "header": "H1",
                        "options": ["A1", "A2"]
                    },
                    {
                        "question": "Q2",
                        "header": "H2",
                        "options": ["B1", "B2"]
                    }
                ]
            }
        });
        let req = parse_questions(&payload).unwrap();
        assert_eq!(req.questions.len(), 2);
        assert_eq!(req.agent, "ZCode");
        assert_eq!(req.questions[0].question, "Q1");
        assert_eq!(req.questions[1].question, "Q2");
    }

    #[test]
    fn parse_questions_preserves_agent_field() {
        let payload: Value = serde_json::json!({
            "questions": [{
                "question": "Q?",
                "header": "H",
                "options": ["1", "2"]
            }],
            "agent": "Codex"
        });
        let req = parse_questions(&payload).unwrap();
        assert_eq!(req.agent, "Codex");

        let zcode_payload: Value = serde_json::json!({
            "questions": [{
                "question": "Q?",
                "header": "H",
                "options": ["1", "2"]
            }],
            "agent": "ZCode"
        });
        let zreq = parse_questions(&zcode_payload).unwrap();
        assert_eq!(zreq.agent, "ZCode");
    }

    #[test]
    fn hook_output_carries_the_choice_in_updated_answers() {
        let tool_input: Value = serde_json::json!({
            "questions": [{
                "question": "选哪个?",
                "header": "选择",
                "options": [
                    {"label": "第一个", "description": "a"},
                    {"label": "第二个", "description": "b"},
                    {"label": "第三个", "description": "c"}
                ],
                "multiSelect": false
            }]
        });
        let req = parse_questions(&tool_input).unwrap();
        let out = hook_output_for(
            &AskReply::Select { index: 2, label: "第三个".into() },
            &req,
            Some(&tool_input),
            false,
        );
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let hook_out = &parsed["hookSpecificOutput"];
        assert_eq!(hook_out["permissionDecision"], "allow");
        assert!(hook_out["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("点头 3 次"));
        assert!(hook_out["additionalContext"]
            .as_str()
            .unwrap()
            .contains("点头 3 次"));
        // updatedInput replaces the tool input wholesale in ZCode, so both
        // `questions` and `answers` must be present to pass inputSchema
        // validation.
        assert_eq!(hook_out["updatedInput"]["questions"], tool_input["questions"]);
        assert_eq!(hook_out["updatedInput"]["answers"]["选哪个?"], "第三个");

        let out = hook_output_for(&AskReply::Pass, &req, Some(&tool_input), false);
        assert!(out.contains("\"permissionDecision\":\"ask\""), "{out}");
    }

    #[test]
    fn hook_output_for_codex_denies_tool_with_reason() {
        let tool_input: Value = serde_json::json!({
            "questions": [{
                "title": "Which music?",
                "options": ["Rock", "Jazz", "Classical"]
            }]
        });
        let req = parse_questions(&tool_input).unwrap();
        let out = hook_output_for(
            &AskReply::Select { index: 0, label: "Rock".into() },
            &req,
            Some(&tool_input),
            true, // is_codex
        );
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let hook_out = &parsed["hookSpecificOutput"];
        assert_eq!(hook_out["permissionDecision"], "deny");
        assert!(hook_out["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("选择第 1 项「Rock」"));

        let out_pass = hook_output_for(&AskReply::Pass, &req, Some(&tool_input), true);
        let parsed_pass: Value = serde_json::from_str(&out_pass).unwrap();
        assert_eq!(parsed_pass["hookSpecificOutput"]["permissionDecision"], "allow");
    }

    #[test]
    fn hook_output_for_multi_questions_zcode() {
        let tool_input: Value = serde_json::json!({
            "questions": [
                {
                    "question": "Q1?",
                    "header": "H1",
                    "options": [{"label": "A"}, {"label": "B"}]
                },
                {
                    "question": "Q2?",
                    "header": "H2",
                    "options": [{"label": "C"}, {"label": "D"}]
                }
            ]
        });
        let req = parse_questions(&tool_input).unwrap();
        let out = hook_output_for(
            &AskReply::BatchSelect {
                selections: vec![
                    SingleSelection { index: 0, label: "A".into() },
                    SingleSelection { index: 1, label: "D".into() },
                ],
            },
            &req,
            Some(&tool_input),
            false,
        );
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let hook_out = &parsed["hookSpecificOutput"];
        assert_eq!(hook_out["permissionDecision"], "allow");
        assert_eq!(hook_out["updatedInput"]["answers"]["Q1?"], "A");
        assert_eq!(hook_out["updatedInput"]["answers"]["Q2?"], "D");
    }

    #[test]
    fn wire_reply_roundtrips() {
        let select = AskReply::Select { index: 1, label: "选项".into() };
        let text = serde_json::to_string(&select).unwrap();
        assert!(text.contains("\"event\":\"select\""), "{text}");
        assert_eq!(serde_json::from_str::<AskReply>(&text).unwrap(), select);

        let batch = AskReply::BatchSelect {
            selections: vec![
                SingleSelection { index: 0, label: "A".into() },
                SingleSelection { index: 1, label: "B".into() },
            ],
        };
        let batch_text = serde_json::to_string(&batch).unwrap();
        assert_eq!(serde_json::from_str::<AskReply>(&batch_text).unwrap(), batch);

        let pass_text = serde_json::to_string(&AskReply::Pass).unwrap();
        assert_eq!(pass_text, "{\"event\":\"pass\"}");
    }

    #[test]
    fn install_is_idempotent_and_preserves_siblings() {
        let mut config: Value = serde_json::from_str(
            r#"{"mcp": {"servers": {"gitlab": {"url": "http://x"}}}}"#,
        )
        .unwrap();

        assert!(install_hook_into(&mut config, "/opt/rustpods").unwrap());
        let pre = config["hooks"]["events"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 1);
        assert_eq!(pre[0]["matcher"], HOOK_MATCHER);
        // A `command` hook carrying `args` is dropped outright by ZCode, so
        // the installed shape must be a `process` hook.
        assert_eq!(pre[0]["hooks"][0]["type"], "process");
        assert_eq!(pre[0]["hooks"][0]["args"][0], "--zcode-ask-hook");
        assert_eq!(pre[0]["hooks"][0]["timeoutMs"], 180000);
        assert_eq!(config["mcp"]["servers"]["gitlab"]["url"], "http://x");
        assert_eq!(config["hooks"]["enabled"], true);

        // A second run changes nothing.
        assert!(!install_hook_into(&mut config, "/opt/rustpods").unwrap());
        assert_eq!(
            config["hooks"]["events"]["PreToolUse"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn install_migrates_the_dropped_command_shape() {
        // The shape the first version installed: `args` on a `command` hook,
        // which ZCode discards wholesale. Reinstalling must replace it with
        // the canonical process entry rather than only repointing command.
        let mut config: Value = serde_json::from_str(
            r#"{
                "hooks": {
                    "enabled": true,
                    "events": {
                        "PreToolUse": [
                            {"matcher": "AskUserQuestion", "hooks": [
                                {"type": "command", "command": "/old/path/rustpods", "args": ["--zcode-ask-hook"]}
                            ]}
                        ]
                    }
                }
            }"#,
        )
        .unwrap();

        assert!(install_hook_into(&mut config, "/new/path/rustpods").unwrap());
        let pre = config["hooks"]["events"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 1, "migration must not duplicate the hook");
        assert_eq!(pre[0]["hooks"][0]["type"], "process");
        assert_eq!(pre[0]["hooks"][0]["command"], "/new/path/rustpods");
    }

    #[test]
    fn install_refuses_wrong_shaped_blocks() {
        // `hooks` present but not an object: refuse instead of clobbering.
        let mut config: Value = serde_json::json!({"hooks": []});
        assert!(install_hook_into(&mut config, "/x").is_err());
        assert_eq!(config["hooks"], serde_json::json!([]), "must not rewrite");

        let mut config: Value = serde_json::json!({"hooks": {"events": {"PreToolUse": {}}}});
        assert!(install_hook_into(&mut config, "/x").is_err());
    }

    #[test]
    fn install_respects_a_deliberate_hooks_kill_switch() {
        let mut config: Value = serde_json::json!({
            "hooks": {"enabled": false, "events": {"PostToolUse": []}}
        });
        let err = install_hook_into(&mut config, "/x").unwrap_err();
        assert!(err.contains("false"), "{err}");
        // The user's kill switch and their other hooks survive untouched.
        assert_eq!(config["hooks"]["enabled"], false);
        assert!(config["hooks"]["events"].get("PostToolUse").is_some());
    }

    #[test]
    fn parse_questions_rejects_option_objects_without_labels() {
        // Dropping a label-less option would shift every later nod index
        // onto the wrong answer; the payload must be unusable instead.
        let payload: Value = serde_json::json!({
            "tool_input": {"questions": [{
                "question": "q",
                "options": [{"label": "A"}, {"description": "no label"}]
            }]}
        });
        assert!(parse_questions(&payload).is_none());
    }

    #[test]
    fn status_reports_the_installed_command() {
        let with_hook: Value = serde_json::from_str(
            r#"{
                "hooks": {"enabled": true, "events": {"PreToolUse": [
                    {"matcher": "AskUserQuestion", "hooks": [{"command": "/x/rustpods", "args": ["--zcode-ask-hook"]}]}
                ]}}
            }"#,
        )
        .unwrap();
        assert_eq!(
            hook_status_from(&with_hook),
            HookInstallStatus::Installed("/x/rustpods".into())
        );

        let without_hook: Value = serde_json::json!({"mcp": {}});
        assert_eq!(hook_status_from(&without_hook), HookInstallStatus::NotInstalled);

        // Another agent's AskUserQuestion hook is not ours.
        let foreign: Value = serde_json::from_str(
            r#"{
                "hooks": {"enabled": true, "events": {"PreToolUse": [
                    {"matcher": "AskUserQuestion", "hooks": [{"command": "/other", "args": ["--else"]}]}
                ]}}
            }"#,
        )
        .unwrap();
        assert_eq!(hook_status_from(&foreign), HookInstallStatus::NotInstalled);
    }

    #[test]
    fn uninstall_removes_only_our_entry() {
        let mut config: Value = serde_json::from_str(
            r#"{
                "hooks": {
                    "enabled": true,
                    "events": {
                        "PreToolUse": [
                            {"matcher": "AskUserQuestion", "hooks": [
                                {"type": "command", "command": "/opt/rustpods", "args": ["--zcode-ask-hook"]}
                            ]},
                            {"matcher": "Write", "hooks": [{"type": "command", "command": "/other/tool"}]}
                        ]
                    }
                }
            }"#,
        )
        .unwrap();

        assert!(uninstall_hook_from(&mut config));
        let pre = config["hooks"]["events"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 1);
        assert_eq!(pre[0]["matcher"], "Write");
        // An AskUserQuestion hook without our marker belongs to someone else
        // and must survive.
        let mut foreign_ask: Value = serde_json::from_str(
            r#"{
                "hooks": {"enabled": true, "events": {"PreToolUse": [
                    {"matcher": "AskUserQuestion", "hooks": [{"command": "/other", "args": ["--else"]}]}
                ]}}
            }"#,
        )
        .unwrap();
        assert!(!uninstall_hook_from(&mut foreign_ask));
    }

    #[test]
    fn uninstall_cleans_up_an_empty_hooks_block() {
        let mut config: Value = serde_json::from_str(
            r#"{
                "hooks": {
                    "enabled": true,
                    "events": {"PreToolUse": [{"matcher": "AskUserQuestion", "hooks": [
                        {"command": "/opt/rustpods", "args": ["--zcode-ask-hook"]}
                    ]}]}
                }
            }"#,
        )
        .unwrap();

        assert!(uninstall_hook_from(&mut config));
        assert!(config.get("hooks").is_none());
    }

    #[test]
    fn parse_questions_extracts_bash_permission_prompt() {
        let payload: Value = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": {
                "command": "cargo test --all"
            }
        });
        let req = parse_questions(&payload).expect("should parse bash permission prompt");
        assert!(req.questions.is_empty());
        let perm = req.permission.expect("permission prompt should be extracted");
        assert_eq!(perm.tool_name, "Bash");
        assert_eq!(perm.command_or_arg, "cargo test --all");
    }

    #[test]
    fn parse_questions_extracts_file_write_permission_prompt() {
        let payload: Value = serde_json::json!({
            "tool_name": "write_file",
            "tool_input": {
                "path": "/etc/hosts",
                "content": "127.0.0.1 localhost"
            }
        });
        let req = parse_questions(&payload).expect("should parse write_file permission prompt");
        let perm = req.permission.expect("permission prompt should be extracted");
        assert_eq!(perm.tool_name, "write_file");
        assert_eq!(perm.command_or_arg, "/etc/hosts");
    }

    #[test]
    fn hook_output_for_permission_allow_and_deny() {
        let req = sample_permission_request("ZCode");

        let out_allow = hook_output_for(&AskReply::Allow, &req, None, false);
        let val_allow: Value = serde_json::from_str(&out_allow).unwrap();
        assert_eq!(val_allow["hookSpecificOutput"]["permissionDecision"], "allow");

        let out_deny = hook_output_for(&AskReply::Deny, &req, None, false);
        let val_deny: Value = serde_json::from_str(&out_deny).unwrap();
        assert_eq!(val_deny["hookSpecificOutput"]["permissionDecision"], "deny");

        let out_pass = hook_output_for(&AskReply::Pass, &req, None, false);
        let val_pass: Value = serde_json::from_str(&out_pass).unwrap();
        assert_eq!(val_pass["hookSpecificOutput"]["permissionDecision"], "ask");
    }

    #[test]
    fn sample_permission_request_roundtrip() {
        let req = sample_permission_request("Codex");
        let json = serde_json::to_string(&req).unwrap();
        let decoded: HookRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.agent, "Codex");
        let perm = decoded.permission.unwrap();
        assert_eq!(perm.tool_name, "Bash");
        assert_eq!(perm.command_or_arg, "cargo test --workspace");
    }
}
