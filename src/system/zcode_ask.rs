//! Answers ZCode `AskUserQuestion` prompts by counting head nods.
//!
//! ZCode Desktop renders its question panel as a mouse-driven widget with no
//! keyboard handling, so injecting keystrokes cannot select an option. What
//! ZCode *does* offer is a `PreToolUse` hook that receives the full question
//! payload and may block the tool while it runs. This module is both ends of
//! that channel:
//!
//! * `rustpods --zcode-ask-hook` is the hook command. It reads the hook JSON
//!   from stdin, forwards the question over a Unix socket to the running
//!   RustPods instance, and translates the reply into hook output. Every
//!   failure path falls back to "ask", which leaves the normal question UI in
//!   place - a broken gesture channel must never wedge ZCode.
//! * [`run_socket_server`] runs inside RustPods (GUI or `--no-gui` daemon). It
//!   registers the question in the [`ASK_HUB`], shows a notification, and
//!   waits for [`run_question_answers`] - the tokio task feeding head poses to
//!   the [`NodCounter`](crate::core::nod_counter::NodCounter) - to resolve it.
//!
//! The answer travels back as a `deny` decision whose reason carries the
//! choice, which the agent reads as the user's answer. Shaking the head,
//! nodding past the option count, or simply doing nothing for a couple of
//! minutes passes through, and ZCode shows its normal question UI.

use crate::core::nod_counter::{NodCounter, NodEvent};
use crate::core::head_tracking::HeadPose;
use crate::system::agent_hooks::HookInstallStatus;
use serde::{Deserialize, Serialize};
use serde_json::json;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookRequest {
    pub questions: Vec<HookQuestion>,
}

/// RustPods → hook reply.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AskReply {
    /// The wearer nodded `index + 1` times; the label is the selected option.
    Select { index: usize, label: String },
    /// Cancelled, timed out, or unanswerable: let ZCode ask normally.
    Pass,
}

/// Extracts the question list from a hook payload. Accepts both the full
/// `PreToolUse` envelope (`tool_input.questions`) and a bare question list,
/// and tolerates option objects or plain strings.
pub fn parse_questions(value: &serde_json::Value) -> Option<HookRequest> {
    let questions = value
        .get("tool_input")
        .and_then(|t| t.get("questions"))
        .or_else(|| value.get("questions"))?
        .as_array()?;

    let mut out = Vec::new();
    for q in questions {
        let options = match q.get("options") {
            Some(serde_json::Value::Array(opts)) => opts
                .iter()
                .filter_map(|o| match o {
                    serde_json::Value::String(s) => Some(s.clone()),
                    serde_json::Value::Object(_) => {
                        o.get("label").and_then(|l| l.as_str()).map(String::from)
                    }
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        out.push(HookQuestion {
            question: q
                .get("question")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            header: q
                .get("header")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            options,
            multi_select: q
                .get("multiSelect")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        });
    }
    Some(HookRequest { questions: out })
}

/// Maps a committed nod count onto a reply. Nods within the option range pick
/// that option; one nod past the end means the wearer wants "Other" or would
/// rather answer by hand; anything more is treated as a mistake.
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
/// A selection is a `deny` whose reason states the answer; the agent reading
/// the denial treats it as the user's choice. A pass-through is `ask`, which
/// restores the normal ZCode question UI.
fn hook_output_for(reply: &AskReply) -> String {
    match reply {
        AskReply::Select { index, label } => json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": format!(
                    "【头部手势应答】用户点头 {} 次,选择第 {} 项「{}」。(RustPods 通过点头手势代答;\
                     请直接采用该选项并继续,不要重复提问。)",
                    index + 1,
                    index + 1,
                    label
                ),
            }
        })
        .to_string(),
        AskReply::Pass => json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "ask",
            }
        })
        .to_string(),
    }
}

// ---------------------------------------------------------------------------
// Hub: shared state between the socket server and the pose-driven task
// ---------------------------------------------------------------------------

static NEXT_ASK_ID: AtomicU64 = AtomicU64::new(1);

/// What the UI card shows while a question is pending.
#[derive(Debug, Clone, PartialEq)]
pub struct AskProgress {
    pub id: u64,
    pub question: String,
    pub header: String,
    pub options: Vec<String>,
    pub count: usize,
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
    resolved: Mutex<Option<(String, Instant)>>,
    last_pose: Mutex<Option<Instant>>,
}

pub static ASK_HUB: LazyLock<AskHub> = LazyLock::new(AskHub::new);

impl AskHub {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(None),
            progress: Mutex::new(None),
            resolved: Mutex::new(None),
            last_pose: Mutex::new(None),
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
    }

    /// Records a nod count for the UI.
    fn set_count(&self, id: u64, count: usize) {
        let mut progress = self.progress.lock().unwrap();
        if let Some(p) = progress.as_mut() {
            if p.id == id {
                p.count = count;
            }
        }
    }

    fn clear_progress(&self, id: u64) {
        let mut progress = self.progress.lock().unwrap();
        if progress.as_ref().is_some_and(|p| p.id == id) {
            *progress = None;
        }
    }

    /// Snapshot for the UI card.
    pub fn progress_snapshot(&self) -> Option<AskProgress> {
        self.progress.lock().unwrap().clone()
    }

    /// Marks a choice as made, so the UI can confirm it briefly.
    pub fn mark_resolved(&self, text: String) {
        *self.resolved.lock().unwrap() = Some((text, Instant::now()));
    }

    /// The confirmation text, if one was made recently enough to still show.
    pub fn resolved_note(&self) -> Option<String> {
        let resolved = self.resolved.lock().unwrap();
        resolved
            .as_ref()
            .filter(|(_, at)| at.elapsed() < Duration::from_secs(3))
            .map(|(text, _)| text.clone())
    }

    /// Records that a pose sample just arrived.
    pub fn note_pose(&self) {
        *self.last_pose.lock().unwrap() = Some(Instant::now());
    }

    /// Whether the pose stream is alive right now.
    pub fn tracking_fresh(&self) -> bool {
        self.last_pose
            .lock()
            .unwrap()
            .is_some_and(|at| at.elapsed() < TRACKING_FRESH_AFTER)
    }
}

// ---------------------------------------------------------------------------
// Socket server (runs inside RustPods)
// ---------------------------------------------------------------------------

/// Where the hook process finds the running RustPods instance.
pub fn socket_path() -> PathBuf {
    let dir = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    dir.join("rustpods").join("zcode-ask.sock")
}

/// Serves hook connections until the process exits.
///
/// Failing to bind is not an error: another RustPods instance owns the socket
/// and will answer instead. A stale socket left behind by a crash is removed
/// and rebound once.
pub async fn run_socket_server() {
    let path = socket_path();
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

/// Handles one hook connection: read the request, register the question, wait
/// for the gesture task, reply. Every early exit is a pass-through.
async fn handle_hook_connection(mut stream: tokio::net::UnixStream) {
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
async fn ask_from_stream(stream: &mut tokio::net::UnixStream) -> Option<AskReply> {
    // One line, capped. `take` guarantees a hostile writer cannot balloon
    // memory; anything past the cap means the payload is garbage anyway.
    let mut buf = Vec::new();
    let mut reader = BufReader::new(stream.take(MAX_PAYLOAD_BYTES));
    reader.read_until(b'\n', &mut buf).await.ok()?;
    if buf.is_empty() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&buf).ok()?;
    let request = parse_questions(&value)?;
    Some(ask_core(request).await)
}

/// The shared question pipeline: validate, gate, register, wait for the
/// gesture task. Both the socket handler and the settings page's test button
/// come through here, so a test exercises exactly what a live question does.
async fn ask_core(request: HookRequest) -> AskReply {
    // Only single-select questions with options can be nodded at; everything
    // else (multi-select, multi-question calls) goes straight back.
    if request.questions.len() != 1 {
        return AskReply::Pass;
    }
    let Some(q) = request.questions.into_iter().next() else {
        return AskReply::Pass;
    };
    if q.multi_select || q.options.is_empty() {
        return AskReply::Pass;
    }

    // The wearer is not streaming (buds off, not connected): do not pretend a
    // gesture is coming, hand the question to the normal UI immediately.
    if !ASK_HUB.tracking_fresh() {
        info!("Question passed through: head tracking is not active");
        return AskReply::Pass;
    }

    // Feature toggle lives in the config file; re-reading it here means the
    // switch takes effect without a restart.
    if !crate::system::config::load_config().head_gesture_answers_enabled {
        return AskReply::Pass;
    }

    let id = NEXT_ASK_ID.fetch_add(1, Ordering::SeqCst);
    let progress = AskProgress {
        id,
        question: q.question.clone(),
        header: q.header.clone(),
        options: q.options.clone(),
        count: 0,
    };
    let (tx, rx) = oneshot::channel();
    if !ASK_HUB.register(progress, tx) {
        return AskReply::Pass;
    }
    info!(
        "Question pending: {:?} with {} options (id {})",
        q.question,
        q.options.len(),
        id
    );
    notify_question(&q.question, &q.header, &q.options);

    match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
        Ok(Ok(reply)) => {
            info!("Question {} resolved: {:?}", id, reply);
            ASK_HUB.clear_progress(id);
            reply
        }
        // Timeout, or the pose task vanished without answering. `abandon`
        // clears the registration itself so the next question can register.
        _ => {
            warn!("Question {} timed out; passing through", id);
            ASK_HUB.abandon(id);
            AskReply::Pass
        }
    }
}

/// The sample question the test button and the CLI simulator use.
fn sample_question_request() -> HookRequest {
    HookRequest {
        questions: vec![HookQuestion {
            question: "模拟提问:这是一个用于测试点头应答的示例问题".into(),
            header: "测试".into(),
            options: vec!["第一个选项".into(), "第二个选项".into(), "第三个选项".into()],
            multi_select: false,
        }],
    }
}

/// Pushes the sample question through the real answering pipeline from inside
/// the process. Returns the decision once the wearer nods, cancels, or the
/// question passes through (not worn, not installed, timed out).
pub async fn run_test_question() -> Result<AskReply, String> {
    Ok(ask_core(sample_question_request()).await)
}

/// Shows a desktop notification describing the question and the gestures.
///
/// This is what makes the feature usable in daemon mode, where there is no
/// window to look at. Failures are logged and ignored: a missing notification
/// daemon must not break answering.
fn notify_question(question: &str, header: &str, options: &[String]) {
    let mut body = String::new();
    if !question.is_empty() {
        body.push_str(question);
        body.push('\n');
    }
    for (i, label) in options.iter().enumerate() {
        body.push_str(&format!("{}. {}\n", i + 1, label));
    }
    body.push_str("— 点头 N 次选择第 N 项 · 摇头取消");

    let summary = if header.is_empty() {
        "RustPods · ZCode 提问".to_string()
    } else {
        format!("RustPods · ZCode 提问:{}", header)
    };

    tokio::task::spawn_blocking(move || {
        if let Err(e) = notify_rust::Notification::new()
            .summary(&summary)
            .body(&body)
            .timeout(notify_rust::Timeout::Milliseconds(12000))
            .show()
        {
            debug!("Could not show question notification: {}", e);
        }
    });
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
    let mut counter = NodCounter::new();
    let mut last_sample: Option<Instant> = None;

    loop {
        if pose_rx.changed().await.is_err() {
            return;
        }
        if ASK_TASK_GEN.load(Ordering::SeqCst) != task_gen {
            debug!("A newer head-gesture task took over; retiring this one");
            return;
        }
        ASK_HUB.note_pose();

        let now = Instant::now();
        let dt = last_sample.replace(now).map_or(0.0, |t| now.duration_since(t).as_secs_f32());
        let Some(pose) = *pose_rx.borrow_and_update() else {
            counter.reset();
            continue;
        };

        let Some(progress) = ASK_HUB.progress_snapshot() else {
            counter.reset();
            continue;
        };

        let Some(event) = counter.update(pose.pitch, pose.yaw, dt) else {
            continue;
        };

        let reply = match event {
            NodEvent::Counted(n) => {
                ASK_HUB.set_count(progress.id, n);
                debug!("Nod count {} for question {}", n, progress.id);
                continue;
            }
            NodEvent::Cancelled => AskReply::Pass,
            NodEvent::Commit(n) => decision_for_commit(n, &progress.options),
        };

        resolve_question(progress.id, reply);
        counter.reset();
    }
}

/// Delivers a decision for the pending question, if it is still ours to
/// answer, and tidies the shared state.
fn resolve_question(id: u64, reply: AskReply) {
    if let Some(pending) = ASK_HUB.take_pending(id) {
        let note = match &reply {
            AskReply::Select { index, label } => {
                format!("✓ 已选择第 {} 项「{}」", index + 1, label)
            }
            AskReply::Pass => "已取消,请在 ZCode 中手动作答".to_string(),
        };
        ASK_HUB.mark_resolved(note);
        let _ = pending.reply.send(reply);
    } else {
        // The server timed out first; drop the card so it does not linger.
        ASK_HUB.clear_progress(id);
    }
}

// ---------------------------------------------------------------------------
// Hook process (`rustpods --zcode-ask-hook`)
// ---------------------------------------------------------------------------

/// Entry point of the ZCode hook command.
///
/// Reads the hook payload from stdin, asks RustPods for a decision, and prints
/// the hook output JSON. Any failure exits 0 with no output, which ZCode
/// treats as "hook passed, carry on normally".
pub async fn run_hook_mode() -> i32 {
    let payload = match read_stdin_with_timeout().await {
        Some(payload) => payload,
        None => return 0,
    };
    let value: serde_json::Value = match serde_json::from_str(&payload) {
        Ok(value) => value,
        Err(e) => {
            debug!("zcode-ask-hook: unreadable payload: {}", e);
            return 0;
        }
    };
    let Some(request) = parse_questions(&value) else {
        return 0;
    };
    if request.questions.len() != 1 {
        // Nothing gesturable here; the normal UI handles it.
        return 0;
    }

    debug!(
        "zcode-ask-hook: forwarding a question with {} options",
        request.questions[0].options.len()
    );

    let mut stream = match tokio::time::timeout(HOOK_CONNECT_TIMEOUT, tokio::net::UnixStream::connect(socket_path())).await {
        Ok(Ok(stream)) => stream,
        _ => {
            debug!("zcode-ask-hook: RustPods socket not reachable; passing through");
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
            warn!("zcode-ask-hook: no reply from RustPods; passing through");
            return 0;
        }
        Ok(Ok(_)) => {}
    }

    let reply: AskReply = match serde_json::from_slice(&buf) {
        Ok(reply) => reply,
        Err(e) => {
            warn!("zcode-ask-hook: undecipherable reply: {}", e);
            return 0;
        }
    };

    println!("{}", hook_output_for(&reply));
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

/// Merges our PreToolUse hook entry into a ZCode config document.
///
/// Returns whether anything changed, so repeated installs are no-ops. An
/// entry installed by an older binary path is repointed to `command` rather
/// than duplicated - the marker in `args`, not the command path, is the
/// identity of our hook.
pub fn install_hook_into(config: &mut serde_json::Value, command: &str) -> bool {
    let Some(root) = config.as_object_mut() else {
        return false;
    };
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .expect("hooks must be an object");
    hooks.insert("enabled".into(), json!(true));

    let events = hooks
        .entry("events")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .expect("events must be an object");
    let pre = events
        .entry("PreToolUse")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .expect("PreToolUse must be an array");

    if let Some(existing) = pre.iter_mut().find(|e| entry_is_ours(e)) {
        let mut changed = false;
        if let Some(hooks) = existing.get_mut("hooks").and_then(|h| h.as_array_mut()) {
            for h in hooks.iter_mut() {
                let ours = h
                    .get("args")
                    .and_then(|a| a.as_array())
                    .is_some_and(|args| args.iter().any(|a| a.as_str() == Some(HOOK_ARG_MARKER)));
                if ours {
                    if h.get("command").and_then(|c| c.as_str()) != Some(command) {
                        h["command"] = json!(command);
                        changed = true;
                    }
                    break;
                }
            }
        }
        return changed;
    }

    pre.push(json!({
        "matcher": "AskUserQuestion",
        "hooks": [{
            "type": "command",
            "command": command,
            "args": ["--zcode-ask-hook"],
            "timeoutMs": 180000,
            "onError": "allow",
        }],
    }));
    true
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

/// Whether a PreToolUse entry is ours (AskUserQuestion matcher + our marker).
fn entry_is_ours(entry: &serde_json::Value) -> bool {
    entry.get("matcher").and_then(|m| m.as_str()) == Some("AskUserQuestion")
        && entry
            .get("hooks")
            .and_then(|h| h.as_array())
            .is_some_and(|hooks| {
                hooks.iter().any(|h| {
                    h.get("args")
                        .and_then(|a| a.as_array())
                        .is_some_and(|args| {
                            args.iter().any(|a| a.as_str() == Some(HOOK_ARG_MARKER))
                        })
                })
            })
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

/// Writes `~/.zcode/cli/config.json` with our hook merged in, backing the
/// previous file up first.
pub fn install_hook() -> Result<(), String> {
    let path = zcode_cli_config_path();
    let existing = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".to_string());
    let mut root: serde_json::Value = serde_json::from_str(&existing)
        .map_err(|e| format!("{} is not valid JSON ({}); fix it by hand first", path.display(), e))?;

    let command = hook_command_path()?;
    if !install_hook_into(&mut root, &command) {
        info!("ZCode hook already installed");
        return Ok(());
    }

    if path.exists() {
        let backup = path.with_extension("json.rustpods-bak");
        std::fs::copy(&path, &backup)
            .map_err(|e| format!("could not back up {} : {}", path.display(), e))?;
        info!("Backed up previous config to {}", backup.display());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let pretty = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    std::fs::write(&path, pretty).map_err(|e| format!("could not write {}: {}", path.display(), e))?;
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
        std::fs::write(&path, pretty)
            .map_err(|e| format!("could not write {}: {}", path.display(), e))?;
        info!("Removed ZCode hook");
    } else {
        info!("No ZCode hook to remove");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Simulation (`rustpods --zcode-ask-simulate`)
// ---------------------------------------------------------------------------

/// Pushes a sample question through the socket, for testing the gesture path
/// without ZCode or Bluetooth.
pub async fn run_simulate() -> i32 {
    let request = sample_question_request();

    let mut stream = match tokio::net::UnixStream::connect(socket_path()).await {
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
        error!("Could not send the sample question: {}", e);
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
                info!("Simulated question resolved: {:?}", reply);
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
    fn hook_output_carries_the_choice_in_a_deny_reason() {
        let out = hook_output_for(&AskReply::Select { index: 2, label: "第三个".into() });
        assert!(out.contains("\"permissionDecision\":\"deny\""), "{out}");
        assert!(out.contains("点头 3 次"), "{out}");
        assert!(out.contains("第三个"), "{out}");

        let out = hook_output_for(&AskReply::Pass);
        assert!(out.contains("\"permissionDecision\":\"ask\""), "{out}");
    }

    #[test]
    fn wire_reply_roundtrips() {
        let select = AskReply::Select { index: 1, label: "选项".into() };
        let text = serde_json::to_string(&select).unwrap();
        assert!(text.contains("\"event\":\"select\""), "{text}");
        assert_eq!(serde_json::from_str::<AskReply>(&text).unwrap(), select);

        let pass_text = serde_json::to_string(&AskReply::Pass).unwrap();
        assert_eq!(pass_text, "{\"event\":\"pass\"}");
    }

    #[test]
    fn install_is_idempotent_and_preserves_siblings() {
        let mut config: Value = serde_json::from_str(
            r#"{"mcp": {"servers": {"gitlab": {"url": "http://x"}}}}"#,
        )
        .unwrap();

        assert!(install_hook_into(&mut config, "/opt/rustpods"));
        let pre = config["hooks"]["events"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 1);
        assert_eq!(pre[0]["matcher"], "AskUserQuestion");
        assert_eq!(pre[0]["hooks"][0]["timeoutMs"], 180000);
        assert_eq!(config["mcp"]["servers"]["gitlab"]["url"], "http://x");
        assert_eq!(config["hooks"]["enabled"], true);

        // A second run changes nothing.
        assert!(!install_hook_into(&mut config, "/opt/rustpods"));
        assert_eq!(
            config["hooks"]["events"]["PreToolUse"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn reinstall_from_a_moved_binary_repoints_the_command() {
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

        // The marker in `args`, not the old command path, is the identity.
        assert!(install_hook_into(&mut config, "/new/path/rustpods"));
        let pre = config["hooks"]["events"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 1, "moving the binary must not duplicate the hook");
        assert_eq!(pre[0]["hooks"][0]["command"], "/new/path/rustpods");
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
}
