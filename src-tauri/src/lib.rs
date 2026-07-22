use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
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
];

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ClaudeHookPayload {
    pub session_id: String,
    pub hook_event_name: String,
    pub cwd: String,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub gated: bool,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_input: Option<Value>,
    #[serde(default)]
    pub tool_response: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApprovalRequest {
    pub request_id: String,
    pub tool_name: String,
    pub tool_input: Value,
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
    pub pending_approval: Option<ApprovalRequest>,
}

#[derive(Default)]
pub struct SessionManager {
    sessions: HashMap<String, Session>,
    allowlist: HashSet<AllowlistEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AllowlistEntry {
    tool_name: String,
    bash_command: Option<String>,
}

impl SessionManager {
    fn apply_event(&mut self, payload: ClaudeHookPayload, responses_dir: &Path) -> Vec<Session> {
        let now = now_ms();
        self.apply_timeouts(now);

        let session = self
            .sessions
            .entry(payload.session_id.clone())
            .or_insert_with(|| Session::new(&payload, now));

        session.project_path = payload.cwd.clone();
        session.project_name = project_name(&payload.cwd);
        session.last_event_at = now;

        match payload.hook_event_name.as_str() {
            "SessionStart" | "UserPromptSubmit" => {
                session.status = SessionStatus::Working;
                session.pending_approval = None;
            }
            "PreToolUse" => {
                session.status = SessionStatus::Working;
                session.current_tool = payload.tool_name.clone();
                session.pending_approval = None;

                if payload.gated {
                    let request_id = payload.request_id.clone().unwrap_or_default();
                    let tool_name = payload
                        .tool_name
                        .clone()
                        .unwrap_or_else(|| "command".to_string());
                    let tool_input = payload.tool_input.clone().unwrap_or(Value::Null);

                    let allowlisted = self
                        .allowlist
                        .contains(&AllowlistEntry::from_tool(&tool_name, &tool_input));

                    if !request_id.is_empty() && allowlisted {
                        if let Err(error) =
                            write_approval_response(responses_dir, &request_id, "allow", None)
                        {
                            eprintln!("failed to write allowlisted approval response: {error}");
                        }
                    } else if !request_id.is_empty() {
                        session.status = SessionStatus::WaitingForApproval;
                        session.current_tool = Some(tool_name.clone());
                        session.pending_approval = Some(ApprovalRequest {
                            request_id,
                            tool_name,
                            tool_input,
                        });
                    }
                }
            }
            "PostToolUse" => {
                session.status = SessionStatus::Working;
                session.current_tool = None;
            }
            "PermissionRequest" => {
                let tool_name = payload
                    .tool_name
                    .clone()
                    .unwrap_or_else(|| "command".to_string());
                let tool_input = payload.tool_input.clone().unwrap_or(Value::Null);
                session.status = SessionStatus::WaitingForApproval;
                session.current_tool = Some(tool_name.clone());
                session.pending_approval = Some(ApprovalRequest {
                    request_id: payload.request_id.clone().unwrap_or_default(),
                    tool_name,
                    tool_input,
                });
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
                session.status = SessionStatus::Idle;
                session.current_tool = None;
                session.pending_approval = None;
            }
            _ => {}
        }

        self.snapshot()
    }

    fn resolve_approval(&mut self, request_id: &str, decision: &str, always: bool) -> Vec<Session> {
        let mut allowlist_entry = None;

        for session in self.sessions.values_mut() {
            let Some(pending) = &session.pending_approval else {
                continue;
            };
            if pending.request_id != request_id {
                continue;
            }

            if always && decision == "allow" {
                allowlist_entry = Some(AllowlistEntry::from_tool(
                    &pending.tool_name,
                    &pending.tool_input,
                ));
            }

            session.status = SessionStatus::Working;
            session.pending_approval = None;
            break;
        }

        if let Some(entry) = allowlist_entry {
            self.allowlist.insert(entry);
        }

        self.snapshot()
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

impl AllowlistEntry {
    fn from_tool(tool_name: &str, tool_input: &Value) -> Self {
        let bash_command = if tool_name == "Bash" {
            tool_input
                .get("command")
                .and_then(Value::as_str)
                .map(str::to_string)
        } else {
            None
        };

        Self {
            tool_name: tool_name.to_string(),
            bash_command,
        }
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

fn emit_sessions(app: &tauri::AppHandle, sessions: Vec<Session>) {
    if let Err(error) = app.emit("sessions-updated", sessions) {
        eprintln!("failed to emit sessions-updated: {error}");
    }
}

fn update_sessions(app: &tauri::AppHandle, payload: ClaudeHookPayload) {
    let responses_dir = responses_dir();
    let sessions = {
        let store = app.state::<SessionStore>();
        let mut manager = store.0.lock().expect("session manager mutex poisoned");
        manager.apply_event(payload, &responses_dir)
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
    always: bool,
) -> Result<Vec<Session>, String> {
    if !matches!(decision.as_str(), "allow" | "deny" | "ask") {
        return Err("decision must be allow, deny, or ask".to_string());
    }

    write_approval_response(&responses_dir(), &request_id, &decision, reason.as_deref())
        .map_err(|error| error.to_string())?;

    let sessions = {
        let mut manager = store.0.lock().expect("session manager mutex poisoned");
        manager.resolve_approval(&request_id, &decision, always)
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
                        object.insert("timeout".to_string(), json!(600));
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

    if matches!(event_name, "PreToolUse" | "PostToolUse") {
        group.insert("matcher".to_string(), json!("*"));
    }

    group.insert(
        "hooks".to_string(),
        json!([
            {
                "type": "command",
                "command": command,
                "timeout": 600
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
) -> std::io::Result<()> {
    fs::create_dir_all(responses_dir)?;
    let final_path = responses_dir.join(format!("{request_id}.json"));
    let tmp_path = responses_dir.join(format!("{request_id}.json.tmp"));
    let body = json!({
        "decision": decision,
        "reason": reason.unwrap_or("")
    });
    fs::write(&tmp_path, body.to_string())?;
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
            start_cursor_watcher(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_sessions,
            install_claude_hooks,
            resolve_approval,
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
            gated: true,
            tool_name: Some("Bash".to_string()),
            tool_input: Some(json!({ "command": command })),
            tool_response: None,
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn gated_pre_tool_use_surfaces_request_id() {
        let dir = test_dir("gated");
        let mut manager = SessionManager::default();

        let sessions = manager.apply_event(pre_tool_payload("req-1", "npm test"), &dir);

        assert_eq!(sessions[0].status, SessionStatus::WaitingForApproval);
        let pending = sessions[0]
            .pending_approval
            .as_ref()
            .expect("pending approval");
        assert_eq!(pending.request_id, "req-1");
        assert_eq!(pending.tool_name, "Bash");
    }

    #[test]
    fn response_file_write_uses_final_json_path() {
        let dir = test_dir("response-write");

        write_approval_response(&dir, "req-2", "deny", Some("not today")).expect("write response");

        let final_path = dir.join("req-2.json");
        let tmp_path = dir.join("req-2.json.tmp");
        assert!(final_path.exists());
        assert!(!tmp_path.exists());
        let value: Value =
            serde_json::from_str(&fs::read_to_string(final_path).expect("read response"))
                .expect("parse response");
        assert_eq!(value["decision"], "deny");
        assert_eq!(value["reason"], "not today");
    }

    #[test]
    fn always_allow_writes_immediate_response_for_matching_bash_command() {
        let dir = test_dir("allowlist");
        let mut manager = SessionManager::default();

        manager.apply_event(pre_tool_payload("req-3", "npm test"), &dir);
        manager.resolve_approval("req-3", "allow", true);
        manager.apply_event(pre_tool_payload("req-4", "npm test"), &dir);

        assert!(dir.join("req-4.json").exists());
        assert!(manager
            .snapshot()
            .iter()
            .all(|session| session.pending_approval.is_none()));
    }
}
