use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{Emitter, Manager, PhysicalPosition, PhysicalSize, State};
use tokio::time::{sleep, Duration};

const PIPE_NAME: &str = r"\\.\pipe\mngr";
const DONE_AFTER_MS: u64 = 5 * 60 * 1000;
const REMOVE_AFTER_MS: u64 = 30 * 60 * 1000;
const RESPONSE_CLEANUP_AFTER_MS: u64 = 10 * 60 * 1000;
const CLAUDE_EVENTS: &[&str] = &[
    "PreToolUse",
    "PostToolUse",
    "UserPromptSubmit",
    "Stop",
    "Notification",
    "PermissionRequest",
];

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ClaudeHookPayload {
    pub session_id: String,
    pub hook_event_name: String,
    pub cwd: String,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_input: Option<Value>,
    #[serde(default)]
    pub tool_response: Option<Value>,
    #[serde(default)]
    pub transcript_path: Option<String>,
    #[serde(default)]
    pub permission_suggestions: Vec<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApprovalRequest {
    pub request_id: String,
    pub tool_name: String,
    pub tool_input: Value,
    pub permission_mode: Option<String>,
    pub permission_suggestions: Vec<Value>,
    pub transcript_path: Option<String>,
    pub transcript_offset: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserQuestion {
    pub question: String,
    pub header: Option<String>,
    #[serde(rename = "multiSelect")]
    pub multi_select: bool,
    pub options: Vec<QuestionOption>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuestionRequest {
    pub request_id: String,
    pub questions: Vec<UserQuestion>,
    #[serde(skip_serializing)]
    pub raw_questions: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PendingRequest {
    Permission(ApprovalRequest),
    Question(QuestionRequest),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub enum SessionStatus {
    Working,
    WaitingForApproval,
    WaitingForInput,
    Idle,
    Done,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub session_id: String,
    pub agent_type: String,
    pub status: SessionStatus,
    pub project_path: String,
    pub project_name: String,
    pub started_at: u64,
    pub last_event_at: u64,
    pub current_tool: Option<String>,
    pub permission_mode: Option<String>,
    pub pending_approval: Option<PendingRequest>,
}

#[derive(Default)]
pub struct SessionManager {
    sessions: HashMap<String, Session>,
}

impl SessionManager {
    fn apply_event(&mut self, payload: ClaudeHookPayload) -> Vec<Session> {
        eprintln!(
            "mngr apply_event: event={} session_id={}",
            payload.hook_event_name, payload.session_id
        );

        let now = now_ms();
        self.apply_timeouts(now);

        let session_existed = self.sessions.contains_key(&payload.session_id);
        if payload.hook_event_name == "UserPromptSubmit" {
            eprintln!(
                "mngr apply_event: UserPromptSubmit session_id={} existing_session={}",
                payload.session_id, session_existed
            );
        }

        let session = self
            .sessions
            .entry(payload.session_id.clone())
            .or_insert_with(|| Session::new(&payload, now));

        session.project_path = payload.cwd.clone();
        session.project_name = project_name(&payload.cwd);
        session.last_event_at = now;

        match payload.hook_event_name.as_str() {
            "SessionStart" | "UserPromptSubmit" => {
                eprintln!(
                    "mngr apply_event: clearing pending_approval event={} session_id={} target_found={} had_pending={}",
                    payload.hook_event_name,
                    payload.session_id,
                    session_existed,
                    session.pending_approval.is_some()
                );
                session.status = SessionStatus::Working;
                session.pending_approval = None;
            }
            "PreToolUse" => {
                eprintln!(
                    "mngr apply_event: clearing pending_approval event={} session_id={} target_found={} had_pending={}",
                    payload.hook_event_name,
                    payload.session_id,
                    session_existed,
                    session.pending_approval.is_some()
                );
                session.status = SessionStatus::Working;
                session.current_tool = payload.tool_name.clone();
                session.pending_approval = None;
            }
            "PostToolUse" => {
                eprintln!(
                    "mngr apply_event: clearing pending_approval event={} session_id={} target_found={} had_pending={}",
                    payload.hook_event_name,
                    payload.session_id,
                    session_existed,
                    session.pending_approval.is_some()
                );
                session.status = SessionStatus::Working;
                session.current_tool = None;
                session.pending_approval = None;
            }
            "PermissionRequest" => {
                let tool_name = payload
                    .tool_name
                    .clone()
                    .unwrap_or_else(|| "command".to_string());
                let tool_input = payload.tool_input.clone().unwrap_or(Value::Null);
                session.permission_mode = payload.permission_mode.clone();
                session.current_tool = Some(tool_name.clone());
                let transcript_offset = payload
                    .transcript_path
                    .as_deref()
                    .and_then(transcript_end_offset)
                    .unwrap_or(0);
                let request_id = payload.request_id.clone().unwrap_or_default();

                if tool_name == "AskUserQuestion" {
                    if let Some(question) = parse_question_request(&payload, &request_id) {
                        session.status = SessionStatus::WaitingForApproval;
                        session.pending_approval = Some(PendingRequest::Question(question));
                        return self.snapshot();
                    }
                }

                session.status = SessionStatus::WaitingForApproval;
                session.pending_approval = Some(PendingRequest::Permission(ApprovalRequest {
                    request_id,
                    tool_name,
                    tool_input,
                    permission_mode: payload.permission_mode.clone(),
                    permission_suggestions: payload.permission_suggestions.clone(),
                    transcript_path: payload.transcript_path.clone(),
                    transcript_offset,
                }));
            }
            "Notification" => {
                if notification_is_completion(&payload) {
                    session.status = SessionStatus::Done;
                    session.current_tool = None;
                    session.pending_approval = None;
                } else if notification_is_question(&payload) {
                    session.status = SessionStatus::WaitingForInput;
                    session.current_tool = None;
                    session.pending_approval = None;
                }
            }
            "Stop" => {
                eprintln!(
                    "mngr apply_event: clearing pending_approval event={} session_id={} target_found={} had_pending={}",
                    payload.hook_event_name,
                    payload.session_id,
                    session_existed,
                    session.pending_approval.is_some()
                );
                session.status = SessionStatus::Idle;
                session.current_tool = None;
                session.pending_approval = None;
            }
            _ => {}
        }

        self.snapshot()
    }

    fn resolve_approval(&mut self, request_id: &str) -> Vec<Session> {
        for session in self.sessions.values_mut() {
            let Some(pending) = &session.pending_approval else {
                continue;
            };
            let PendingRequest::Permission(pending) = pending else {
                continue;
            };
            if pending.request_id != request_id {
                continue;
            }

            session.status = SessionStatus::Working;
            session.pending_approval = None;
            break;
        }

        self.snapshot()
    }

    fn question_request(&self, request_id: &str) -> Option<QuestionRequest> {
        self.sessions.values().find_map(|session| {
            let Some(PendingRequest::Question(pending)) = &session.pending_approval else {
                return None;
            };
            (pending.request_id == request_id).then(|| pending.clone())
        })
    }

    fn resolve_question(&mut self, request_id: &str) -> Vec<Session> {
        for session in self.sessions.values_mut() {
            let Some(pending) = &session.pending_approval else {
                continue;
            };
            let PendingRequest::Question(pending) = pending else {
                continue;
            };
            if pending.request_id != request_id {
                continue;
            }

            session.status = SessionStatus::Working;
            session.pending_approval = None;
            break;
        }

        self.snapshot()
    }

    fn poll_transcript_denials(&mut self) -> bool {
        let mut changed = false;

        for session in self.sessions.values_mut() {
            if session.status != SessionStatus::WaitingForApproval {
                continue;
            }

            let Some(PendingRequest::Permission(pending)) = session.pending_approval.as_mut()
            else {
                continue;
            };
            let Some(transcript_path) = pending.transcript_path.as_deref() else {
                continue;
            };

            let Ok(poll_result) =
                poll_transcript_for_denial(Path::new(transcript_path), pending.transcript_offset)
            else {
                continue;
            };

            pending.transcript_offset = poll_result.offset;
            if poll_result.denied {
                session.status = if poll_result.stop_after_denial {
                    SessionStatus::Idle
                } else {
                    SessionStatus::Working
                };
                session.pending_approval = None;
                changed = true;
            }
        }

        changed
    }

    fn snapshot(&mut self) -> Vec<Session> {
        self.apply_timeouts(now_ms());
        let mut sessions = self.sessions.values().cloned().collect::<Vec<_>>();
        sessions.sort_by(|a, b| b.last_event_at.cmp(&a.last_event_at));
        sessions
    }

    fn apply_timeouts(&mut self, now: u64) -> bool {
        let before = self.sessions.len();
        self.sessions
            .retain(|_, session| now.saturating_sub(session.last_event_at) < REMOVE_AFTER_MS);

        let mut changed = before != self.sessions.len();
        for session in self.sessions.values_mut() {
            let age = now.saturating_sub(session.last_event_at);
            if age >= DONE_AFTER_MS && session.status != SessionStatus::Done {
                session.status = SessionStatus::Done;
                session.current_tool = None;
                session.pending_approval = None;
                changed = true;
            }
        }

        changed
    }
}

impl Session {
    fn new(payload: &ClaudeHookPayload, now: u64) -> Self {
        Self {
            session_id: payload.session_id.clone(),
            agent_type: "claude-code".to_string(),
            status: SessionStatus::Working,
            project_path: payload.cwd.clone(),
            project_name: project_name(&payload.cwd),
            started_at: now,
            last_event_at: now,
            current_tool: None,
            permission_mode: payload.permission_mode.clone(),
            pending_approval: None,
        }
    }
}

pub struct SessionStore(Mutex<SessionManager>);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn project_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
        .to_string()
}

fn notification_text(payload: &ClaudeHookPayload) -> String {
    payload
        .extra
        .values()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn notification_is_completion(payload: &ClaudeHookPayload) -> bool {
    let content = notification_text(payload);
    content.contains("complete")
        || content.contains("completed")
        || content.contains("finished")
        || content.contains("done")
}

fn notification_is_question(payload: &ClaudeHookPayload) -> bool {
    let content = notification_text(payload);
    content.contains("?") || content.contains("question") || content.contains("needs input")
}

fn parse_question_request(
    payload: &ClaudeHookPayload,
    request_id: &str,
) -> Option<QuestionRequest> {
    let raw_questions = payload.tool_input.as_ref()?.get("questions")?.clone();
    let question_values = raw_questions.as_array()?;
    let mut questions = Vec::new();

    for value in question_values {
        let question = value.get("question")?.as_str()?.to_string();
        let header = value
            .get("header")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let multi_select = value
            .get("multiSelect")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if multi_select {
            return None;
        }

        let option_values = value.get("options")?.as_array()?;
        let mut options = Vec::new();
        for option in option_values {
            options.push(QuestionOption {
                label: option.get("label")?.as_str()?.to_string(),
                description: option
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            });
        }

        questions.push(UserQuestion {
            question,
            header,
            multi_select,
            options,
        });
    }

    if questions.is_empty() {
        return None;
    }

    Some(QuestionRequest {
        request_id: request_id.to_string(),
        questions,
        raw_questions,
    })
}

fn emit_sessions(app: &tauri::AppHandle, sessions: Vec<Session>) {
    if let Err(error) = app.emit("sessions-updated", sessions) {
        eprintln!("failed to emit sessions-updated: {error}");
    }
}

fn update_sessions(app: &tauri::AppHandle, payload: ClaudeHookPayload) {
    let sessions = {
        let store = app.state::<SessionStore>();
        let mut manager = store.0.lock().expect("session manager mutex poisoned");
        manager.apply_event(payload)
    };
    emit_sessions(app, sessions);
}

#[tauri::command]
fn get_sessions(store: State<'_, SessionStore>) -> Vec<Session> {
    let mut manager = store.0.lock().expect("session manager mutex poisoned");
    manager.snapshot()
}

#[tauri::command]
fn install_claude_hooks() -> Result<String, String> {
    install_claude_hooks_inner().map_err(|error| error.to_string())
}

#[tauri::command]
fn resolve_approval(
    app: tauri::AppHandle,
    store: State<'_, SessionStore>,
    request_id: String,
    decision: String,
    reason: Option<String>,
    updated_permissions: Option<Vec<Value>>,
) -> Result<Vec<Session>, String> {
    if !matches!(decision.as_str(), "allow" | "deny") {
        return Err("decision must be allow or deny".to_string());
    }

    write_approval_response(
        &responses_dir(),
        &request_id,
        &decision,
        reason.as_deref(),
        updated_permissions.as_ref(),
    )
    .map_err(|error| error.to_string())?;

    let sessions = {
        let mut manager = store.0.lock().expect("session manager mutex poisoned");
        manager.resolve_approval(&request_id)
    };
    emit_sessions(&app, sessions.clone());
    Ok(sessions)
}

#[tauri::command]
fn resolve_question(
    app: tauri::AppHandle,
    store: State<'_, SessionStore>,
    request_id: String,
    question: String,
    answer: String,
) -> Result<Vec<Session>, String> {
    let pending_question = {
        let manager = store.0.lock().expect("session manager mutex poisoned");
        manager
            .question_request(&request_id)
            .ok_or_else(|| "question request not found".to_string())?
    };

    write_question_response(
        &responses_dir(),
        &request_id,
        &pending_question.raw_questions,
        &question,
        &answer,
    )
    .map_err(|error| error.to_string())?;

    let sessions = {
        let mut manager = store.0.lock().expect("session manager mutex poisoned");
        manager.resolve_question(&request_id)
    };
    emit_sessions(&app, sessions.clone());
    Ok(sessions)
}

#[derive(Debug, thiserror::Error)]
enum InstallError {
    #[error("could not find the current user's home directory")]
    MissingHomeDir,
    #[error("settings file is not a JSON object: {0}")]
    InvalidSettingsShape(PathBuf),
    #[error("settings hooks field is not a JSON object")]
    InvalidHooksShape,
    #[error("failed to read or write settings: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse or serialize settings JSON: {0}")]
    Json(#[from] serde_json::Error),
}

fn install_claude_hooks_inner() -> Result<String, InstallError> {
    let home_dir = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .ok_or(InstallError::MissingHomeDir)?;

    let settings_dir = home_dir.join(".claude");
    let settings_path = settings_dir.join("settings.json");
    let mngr_dir = settings_dir.join("mngr");
    let hook_script = mngr_dir.join("claude-hook.ps1");
    fs::create_dir_all(&mngr_dir)?;
    fs::write(&hook_script, include_str!("../../scripts/claude-hook.ps1"))?;

    let mut settings = if settings_path.exists() {
        let contents = fs::read_to_string(&settings_path)?;
        if contents.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&contents)?
        }
    } else {
        json!({})
    };

    let settings_object = settings
        .as_object_mut()
        .ok_or_else(|| InstallError::InvalidSettingsShape(settings_path.clone()))?;

    let hooks_value = settings_object.entry("hooks").or_insert_with(|| json!({}));
    let hooks_object = hooks_value
        .as_object_mut()
        .ok_or(InstallError::InvalidHooksShape)?;

    let command = format!(
        "powershell.exe -NoProfile -ExecutionPolicy Bypass -File \"{}\"",
        hook_script.display()
    );

    for event_name in CLAUDE_EVENTS {
        append_hook_group(hooks_object, event_name, &command);
    }

    fs::write(
        &settings_path,
        serde_json::to_string_pretty(&settings)? + "\n",
    )?;

    Ok(settings_path.display().to_string())
}

fn append_hook_group(
    hooks_object: &mut serde_json::Map<String, Value>,
    event_name: &str,
    command: &str,
) {
    let group = hook_group_for(event_name, command);
    let event_hooks = hooks_object
        .entry(event_name.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));

    if let Some(groups) = event_hooks.as_array_mut() {
        if !groups
            .iter_mut()
            .any(|existing| ensure_hook_group_command_timeout(existing, command))
        {
            groups.push(group);
        }
    } else {
        hooks_object.insert(event_name.to_string(), Value::Array(vec![group]));
    }
}

fn ensure_hook_group_command_timeout(group: &mut Value, command: &str) -> bool {
    group
        .get_mut("hooks")
        .and_then(Value::as_array_mut)
        .map(|hooks| {
            let mut matched = false;
            for hook in hooks {
                let is_command = hook.get("type").and_then(Value::as_str) == Some("command")
                    && hook.get("command").and_then(Value::as_str) == Some(command);
                if is_command {
                    if let Some(object) = hook.as_object_mut() {
                        object.insert("timeout".to_string(), json!(3600));
                    }
                    matched = true;
                }
            }
            matched
        })
        .unwrap_or(false)
}

fn hook_group_for(event_name: &str, command: &str) -> Value {
    let mut group = serde_json::Map::new();

    if matches!(
        event_name,
        "PreToolUse" | "PostToolUse" | "PermissionRequest"
    ) {
        group.insert("matcher".to_string(), json!("*"));
    }

    group.insert(
        "hooks".to_string(),
        json!([
            {
                "type": "command",
                "command": command,
                "timeout": 3600
            }
        ]),
    );

    Value::Object(group)
}

fn responses_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("mngr")
        .join("responses")
}

fn write_approval_response(
    responses_dir: &Path,
    request_id: &str,
    decision: &str,
    reason: Option<&str>,
    updated_permissions: Option<&Vec<Value>>,
) -> std::io::Result<()> {
    fs::create_dir_all(responses_dir)?;
    let final_path = responses_dir.join(format!("{request_id}.json"));
    let tmp_path = responses_dir.join(format!("{request_id}.json.tmp"));
    let mut body = json!({
        "decision": decision,
        "reason": reason.unwrap_or("")
    });

    if decision == "allow" {
        if let Some(updated_permissions) = updated_permissions {
            body["updatedPermissions"] = Value::Array(updated_permissions.clone());
        }
    }

    fs::write(&tmp_path, body.to_string().as_bytes())?;
    fs::rename(tmp_path, final_path)
}

fn write_question_response(
    responses_dir: &Path,
    request_id: &str,
    raw_questions: &Value,
    question: &str,
    answer: &str,
) -> std::io::Result<()> {
    fs::create_dir_all(responses_dir)?;
    let final_path = responses_dir.join(format!("{request_id}.json"));
    let tmp_path = responses_dir.join(format!("{request_id}.json.tmp"));
    let mut answers = serde_json::Map::new();
    answers.insert(question.to_string(), Value::String(answer.to_string()));
    let body = json!({
        "decision": "allow",
        "updatedInput": {
            "questions": raw_questions,
            "answers": Value::Object(answers),
            "answer": answer
        }
    });

    fs::write(&tmp_path, body.to_string().as_bytes())?;
    fs::rename(tmp_path, final_path)
}

fn cleanup_responses_dir(responses_dir: &Path, now: SystemTime) -> std::io::Result<()> {
    fs::create_dir_all(responses_dir)?;
    for entry in fs::read_dir(responses_dir)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };
        if age.as_millis() as u64 > RESPONSE_CLEANUP_AFTER_MS {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .manage(SessionStore(Mutex::new(SessionManager::default())))
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Focused(false) = event {
                let _ = window.emit("window-blurred", ());
            }
        })
        .setup(|app| {
            if let Err(error) = cleanup_responses_dir(&responses_dir(), SystemTime::now()) {
                eprintln!("failed to prepare approval response dir: {error}");
            }
            position_overlay_window(app.handle());
            start_pipe_listener(app.handle().clone());
            start_session_cleanup(app.handle().clone());
            start_transcript_watcher(app.handle().clone());
            start_cursor_watcher(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_sessions,
            install_claude_hooks,
            resolve_approval,
            resolve_question,
            expand_panel
        ])
        .run(tauri::generate_context!())
        .expect("error while running mngr");
}

fn position_overlay_window(app: &tauri::AppHandle) {
    if let Err(error) = size_overlay_window_to_monitor(app) {
        eprintln!("failed to position overlay window: {error}");
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_ignore_cursor_events(true);
    }
}

#[tauri::command]
fn expand_panel(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_focus();
    }
    Ok(())
}

fn size_overlay_window_to_monitor(app: &tauri::AppHandle) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window("main") else {
        return Ok(());
    };

    let Some(monitor) = window.primary_monitor()? else {
        return Ok(());
    };

    let size = monitor.size();
    let pos = monitor.position();

    window.set_size(PhysicalSize::new(size.width, size.height))?;
    window.set_position(PhysicalPosition::new(pos.x, pos.y))?;
    Ok(())
}

fn start_cursor_watcher(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let mut last: Option<(i32, i32)> = None;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(16));
            let Some(window) = app.get_webview_window("main") else {
                continue;
            };
            let Ok(cursor) = window.cursor_position() else {
                continue;
            };
            let Ok(win_pos) = window.outer_position() else {
                continue;
            };
            let scale = window.scale_factor().unwrap_or(1.0);
            let x = ((cursor.x - win_pos.x as f64) / scale).round() as i32;
            let y = ((cursor.y - win_pos.y as f64) / scale).round() as i32;
            if last == Some((x, y)) {
                continue;
            }
            last = Some((x, y));
            let _ = window.emit("cursor-pos", serde_json::json!({ "x": x, "y": y }));
        }
    });
}

#[derive(Debug, Clone, Copy)]
struct TranscriptPollResult {
    offset: u64,
    denied: bool,
    stop_after_denial: bool,
}

fn transcript_end_offset(path: &str) -> Option<u64> {
    fs::metadata(path).ok().map(|metadata| metadata.len())
}

fn poll_transcript_for_denial(path: &Path, offset: u64) -> std::io::Result<TranscriptPollResult> {
    let mut file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = offset.min(len);
    file.seek(SeekFrom::Start(start))?;

    let mut appended = String::new();
    file.read_to_string(&mut appended)?;

    let Some(last_newline) = appended.rfind('\n') else {
        return Ok(TranscriptPollResult {
            offset: start,
            denied: false,
            stop_after_denial: false,
        });
    };

    let complete = &appended[..=last_newline];
    let next_offset = start + complete.len() as u64;
    let mut denied = false;
    let mut stop_after_denial = false;

    for line in complete.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };

        if denied && transcript_line_is_stop(&value) {
            stop_after_denial = true;
        }

        if transcript_line_is_terminal_denial(&value) {
            denied = true;
        }
    }

    Ok(TranscriptPollResult {
        offset: next_offset,
        denied,
        stop_after_denial,
    })
}

fn transcript_line_is_terminal_denial(value: &Value) -> bool {
    json_contains_string_field(value, "toolDenialKind", "user-rejected")
        || (json_contains_string_field(value, "role", "user")
            && json_contains_tool_result_error(value))
}

fn transcript_line_is_stop(value: &Value) -> bool {
    value.get("turn_duration").is_some()
        || json_contains_string_field(value, "hook_event_name", "Stop")
        || json_contains_string_field(value, "type", "stop")
}

fn json_contains_string_field(value: &Value, key: &str, expected: &str) -> bool {
    match value {
        Value::Object(object) => {
            object
                .get(key)
                .and_then(Value::as_str)
                .is_some_and(|actual| actual == expected)
                || object
                    .values()
                    .any(|child| json_contains_string_field(child, key, expected))
        }
        Value::Array(items) => items
            .iter()
            .any(|child| json_contains_string_field(child, key, expected)),
        _ => false,
    }
}

fn json_contains_tool_result_error(value: &Value) -> bool {
    match value {
        Value::Object(object) => {
            let is_tool_result = object
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind == "tool_result");
            let is_error = object
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);

            (is_tool_result && is_error) || object.values().any(json_contains_tool_result_error)
        }
        Value::Array(items) => items.iter().any(json_contains_tool_result_error),
        _ => false,
    }
}

fn start_transcript_watcher(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            sleep(Duration::from_millis(500)).await;
            let changed = {
                let store = app.state::<SessionStore>();
                let mut manager = store.0.lock().expect("session manager mutex poisoned");
                manager.poll_transcript_denials()
            };

            if changed {
                let sessions = {
                    let store = app.state::<SessionStore>();
                    let mut manager = store.0.lock().expect("session manager mutex poisoned");
                    manager.snapshot()
                };
                emit_sessions(&app, sessions);
            }
        }
    });
}

fn start_session_cleanup(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            sleep(Duration::from_secs(60)).await;
            let changed = {
                let store = app.state::<SessionStore>();
                let mut manager = store.0.lock().expect("session manager mutex poisoned");
                manager.apply_timeouts(now_ms())
            };

            if changed {
                let sessions = {
                    let store = app.state::<SessionStore>();
                    let mut manager = store.0.lock().expect("session manager mutex poisoned");
                    manager.snapshot()
                };
                emit_sessions(&app, sessions);
            }
        }
    });
}

#[cfg(windows)]
fn start_pipe_listener(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = pipe::listen(app).await {
            eprintln!("mngr named pipe listener stopped: {error}");
        }
    });
}

#[cfg(not(windows))]
fn start_pipe_listener(_app: tauri::AppHandle) {
    eprintln!("mngr named pipe listener is only available on Windows");
}

#[cfg(windows)]
mod pipe {
    use super::{update_sessions, ClaudeHookPayload, PIPE_NAME};
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

    pub async fn listen(app: tauri::AppHandle) -> std::io::Result<()> {
        loop {
            let server = ServerOptions::new().create(PIPE_NAME)?;
            server.connect().await?;

            let app_for_client = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = handle_client(server, app_for_client).await {
                    eprintln!("mngr named pipe client error: {error}");
                }
            });
        }
    }

    async fn handle_client(server: NamedPipeServer, app: tauri::AppHandle) -> std::io::Result<()> {
        let mut lines = BufReader::new(server).lines();

        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }

            match serde_json::from_str::<ClaudeHookPayload>(&line) {
                Ok(payload) => update_sessions(&app, payload),
                Err(error) => eprintln!("failed to parse Claude hook payload: {error}"),
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mngr-{name}-{}", now_ms()));
        fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    fn pre_tool_payload(request_id: &str, command: &str) -> ClaudeHookPayload {
        ClaudeHookPayload {
            session_id: "session-1".to_string(),
            hook_event_name: "PreToolUse".to_string(),
            cwd: "C:\\work\\project".to_string(),
            request_id: Some(request_id.to_string()),
            permission_mode: None,
            tool_name: Some("Bash".to_string()),
            tool_input: Some(json!({ "command": command })),
            tool_response: None,
            transcript_path: None,
            permission_suggestions: Vec::new(),
            extra: BTreeMap::new(),
        }
    }

    fn permission_request_payload(request_id: &str) -> ClaudeHookPayload {
        ClaudeHookPayload {
            session_id: "session-1".to_string(),
            hook_event_name: "PermissionRequest".to_string(),
            cwd: "C:\\work\\project".to_string(),
            request_id: Some(request_id.to_string()),
            permission_mode: Some("default".to_string()),
            tool_name: Some("Bash".to_string()),
            tool_input: Some(json!({ "command": "npm test" })),
            tool_response: None,
            transcript_path: None,
            permission_suggestions: vec![
                json!({
                    "type": "addDirectories",
                    "directories": ["C:\\work\\project"],
                    "destination": "session"
                }),
                json!({
                    "type": "setMode",
                    "mode": "acceptEdits",
                    "destination": "session"
                }),
            ],
            extra: BTreeMap::new(),
        }
    }

    fn session_event_payload(hook_event_name: &str) -> ClaudeHookPayload {
        ClaudeHookPayload {
            session_id: "session-1".to_string(),
            hook_event_name: hook_event_name.to_string(),
            cwd: "C:\\work\\project".to_string(),
            request_id: None,
            permission_mode: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            transcript_path: None,
            permission_suggestions: Vec::new(),
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn pre_tool_use_updates_current_tool_without_approval() {
        let mut manager = SessionManager::default();

        let sessions = manager.apply_event(pre_tool_payload("req-1", "npm test"));

        assert_eq!(sessions[0].status, SessionStatus::Working);
        assert_eq!(sessions[0].current_tool.as_deref(), Some("Bash"));
        assert!(sessions[0].pending_approval.is_none());
    }

    #[test]
    fn permission_request_surfaces_suggestions_and_mode() {
        let mut manager = SessionManager::default();

        let sessions = manager.apply_event(permission_request_payload("req-2"));

        assert_eq!(sessions[0].status, SessionStatus::WaitingForApproval);
        assert_eq!(sessions[0].permission_mode.as_deref(), Some("default"));
        let PendingRequest::Permission(pending) = sessions[0]
            .pending_approval
            .as_ref()
            .expect("pending approval")
        else {
            panic!("expected permission request");
        };
        assert_eq!(pending.request_id, "req-2");
        assert_eq!(pending.tool_name, "Bash");
        assert_eq!(pending.permission_mode.as_deref(), Some("default"));
        assert_eq!(pending.permission_suggestions.len(), 2);
        assert_eq!(pending.permission_suggestions[0]["type"], "addDirectories");
        assert_eq!(pending.permission_suggestions[1]["mode"], "acceptEdits");
    }

    #[test]
    fn stop_after_pending_approval_clears_card() {
        let mut manager = SessionManager::default();
        manager.apply_event(permission_request_payload("req-stop"));

        let sessions = manager.apply_event(session_event_payload("Stop"));

        assert_eq!(sessions[0].status, SessionStatus::Idle);
        assert!(sessions[0].pending_approval.is_none());
    }

    #[test]
    fn user_prompt_after_pending_approval_clears_card() {
        let mut manager = SessionManager::default();
        manager.apply_event(permission_request_payload("req-prompt"));

        let sessions = manager.apply_event(session_event_payload("UserPromptSubmit"));

        assert_eq!(sessions[0].status, SessionStatus::Working);
        assert!(sessions[0].pending_approval.is_none());
    }

    #[test]
    fn new_permission_request_replaces_pending_approval() {
        let mut manager = SessionManager::default();
        manager.apply_event(permission_request_payload("req-old"));

        let sessions = manager.apply_event(permission_request_payload("req-new"));

        assert_eq!(sessions[0].status, SessionStatus::WaitingForApproval);
        let PendingRequest::Permission(pending) = sessions[0]
            .pending_approval
            .as_ref()
            .expect("pending approval")
        else {
            panic!("expected permission request");
        };
        assert_eq!(pending.request_id, "req-new");
    }

    #[test]
    fn transcript_user_rejected_denial_clears_pending_approval() {
        let dir = test_dir("transcript-denial");
        let transcript_path = dir.join("session.jsonl");
        fs::write(&transcript_path, "{\"type\":\"prior\"}\n").expect("write transcript seed");

        let mut manager = SessionManager::default();
        let mut payload = permission_request_payload("req-transcript-denial");
        payload.transcript_path = Some(transcript_path.display().to_string());
        manager.apply_event(payload);

        let current = fs::read_to_string(&transcript_path).expect("read transcript seed");
        fs::write(
            &transcript_path,
            format!(
                "{current}{}\n",
                json!({
                    "toolDenialKind": "user-rejected",
                    "tool_result": {
                        "is_error": true
                    }
                })
            ),
        )
        .expect("append transcript denial");

        assert!(manager.poll_transcript_denials());
        let sessions = manager.snapshot();
        assert_eq!(sessions[0].status, SessionStatus::Working);
        assert!(sessions[0].pending_approval.is_none());
    }

    fn question_request_payload(request_id: &str) -> ClaudeHookPayload {
        let question = "Which path \u{2014} should I \u{201c}take\u{201d}?";
        let description = "Spend more time checking \u{2014} preserving \u{201c}quotes\u{201d}.";
        let payload = json!({
            "session_id": "session-1",
            "hook_event_name": "PermissionRequest",
            "cwd": "C:\\work\\project",
            "request_id": request_id,
            "permission_mode": "default",
            "tool_name": "AskUserQuestion",
            "tool_input": {
                "questions": [
                    {
                        "question": question,
                        "header": "Choose",
                        "multiSelect": false,
                        "options": [
                            { "label": "Fast", "description": "Do the quickest safe thing." },
                            { "label": "Careful", "description": description }
                        ]
                    }
                ]
            },
            "permission_suggestions": []
        });
        let payload_bytes = payload.to_string().into_bytes();
        assert!(payload_bytes
            .windows(question.as_bytes().len())
            .any(|window| window == question.as_bytes()));
        serde_json::from_slice(&payload_bytes).expect("parse payload bytes")
    }

    #[test]
    fn ask_user_question_surfaces_options_and_writes_answer_with_original_questions() {
        let dir = test_dir("question-answer");
        let mut manager = SessionManager::default();

        let sessions = manager.apply_event(question_request_payload("req-question"));

        assert_eq!(sessions[0].status, SessionStatus::WaitingForApproval);
        let PendingRequest::Question(pending) = sessions[0]
            .pending_approval
            .as_ref()
            .expect("pending question")
        else {
            panic!("expected question request");
        };
        assert_eq!(pending.request_id, "req-question");
        assert_eq!(pending.questions.len(), 1);
        assert_eq!(
            pending.questions[0].question,
            "Which path \u{2014} should I \u{201c}take\u{201d}?"
        );
        assert_eq!(
            pending.questions[0].question.as_bytes(),
            "Which path \u{2014} should I \u{201c}take\u{201d}?".as_bytes()
        );
        assert_eq!(pending.questions[0].header.as_deref(), Some("Choose"));
        assert!(!pending.questions[0].multi_select);
        assert_eq!(pending.questions[0].options[0].label, "Fast");
        assert_eq!(
            pending.questions[0].options[1].description,
            "Spend more time checking \u{2014} preserving \u{201c}quotes\u{201d}."
        );

        write_question_response(
            &dir,
            &pending.request_id,
            &pending.raw_questions,
            &pending.questions[0].question,
            "Careful",
        )
        .expect("write question response");

        let response_bytes = fs::read(dir.join("req-question.json")).expect("read response bytes");
        let unicode_bytes =
            "Spend more time checking \u{2014} preserving \u{201c}quotes\u{201d}.".as_bytes();
        assert!(response_bytes
            .windows(unicode_bytes.len())
            .any(|window| window == unicode_bytes));
        let value: Value = serde_json::from_slice(&response_bytes).expect("parse response");
        assert_eq!(value["decision"], "allow");
        assert_eq!(
            value["updatedInput"]["answers"]
                .get("Which path \u{2014} should I \u{201c}take\u{201d}?")
                .expect("answer by question text"),
            "Careful"
        );
        assert_eq!(value["updatedInput"]["answer"], "Careful");
        assert_eq!(
            value["updatedInput"]["questions"],
            json!([
                {
                    "question": "Which path \u{2014} should I \u{201c}take\u{201d}?",
                    "header": "Choose",
                    "multiSelect": false,
                    "options": [
                        { "label": "Fast", "description": "Do the quickest safe thing." },
                        { "label": "Careful", "description": "Spend more time checking \u{2014} preserving \u{201c}quotes\u{201d}." }
                    ]
                }
            ])
        );

        let sessions = manager.resolve_question("req-question");
        assert_eq!(sessions[0].status, SessionStatus::Working);
        assert!(sessions[0].pending_approval.is_none());
    }

    #[test]
    fn response_file_write_uses_final_json_path() {
        let dir = test_dir("response-write");

        write_approval_response(&dir, "req-3", "deny", Some("not today"), None)
            .expect("write response");

        let final_path = dir.join("req-3.json");
        let tmp_path = dir.join("req-3.json.tmp");
        assert!(final_path.exists());
        assert!(!tmp_path.exists());
        let value: Value =
            serde_json::from_str(&fs::read_to_string(final_path).expect("read response"))
                .expect("parse response");
        assert_eq!(value["decision"], "deny");
        assert_eq!(value["reason"], "not today");
        assert!(value.get("updatedPermissions").is_none());
    }

    #[test]
    fn chosen_suggestion_is_echoed_into_updated_permissions() {
        let dir = test_dir("updated-permissions");
        let suggestion = json!({
            "type": "addDirectories",
            "directories": ["C:\\work\\project"],
            "destination": "session"
        });
        let updated_permissions = vec![suggestion.clone()];

        write_approval_response(&dir, "req-4", "allow", None, Some(&updated_permissions))
            .expect("write response");

        let value: Value = serde_json::from_str(
            &fs::read_to_string(dir.join("req-4.json")).expect("read response"),
        )
        .expect("parse response");
        assert_eq!(value["decision"], "allow");
        assert_eq!(value["updatedPermissions"], json!([suggestion]));
    }
}
