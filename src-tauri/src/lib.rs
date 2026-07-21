use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{Emitter, Manager, PhysicalPosition, PhysicalSize, State};
use tokio::time::{sleep, Duration};

const PIPE_NAME: &str = r"\\.\pipe\mngr";
const COLLAPSED_WIDTH: f64 = 18.0;
const PEEK_WIDTH: f64 = 280.0;
const EXPANDED_WIDTH: f64 = 460.0;
const DONE_AFTER_MS: u64 = 5 * 60 * 1000;
const REMOVE_AFTER_MS: u64 = 30 * 60 * 1000;
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
}

impl SessionManager {
    fn apply_event(&mut self, payload: ClaudeHookPayload) -> Vec<Session> {
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

    fs::write(&settings_path, serde_json::to_string_pretty(&settings)? + "\n")?;

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
        if !groups.iter().any(|existing| hook_group_has_command(existing, command)) {
            groups.push(group);
        }
    } else {
        hooks_object.insert(event_name.to_string(), Value::Array(vec![group]));
    }
}

fn hook_group_has_command(group: &Value, command: &str) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .map(|hooks| {
            hooks.iter().any(|hook| {
                hook.get("type").and_then(Value::as_str) == Some("command")
                    && hook.get("command").and_then(Value::as_str) == Some(command)
            })
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
                "command": command
            }
        ]),
    );

    Value::Object(group)
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
            position_overlay_window(app.handle());
            start_pipe_listener(app.handle().clone());
            start_session_cleanup(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_sessions,
            install_claude_hooks,
            expand_panel,
            peek_panel,
            collapse_panel
        ])
        .run(tauri::generate_context!())
        .expect("error while running mngr");
}

fn position_overlay_window(app: &tauri::AppHandle) {
    if let Err(error) = resize_overlay_window(app, COLLAPSED_WIDTH) {
        eprintln!("failed to position overlay window: {error}");
    }
}

#[tauri::command]
fn expand_panel(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_focus();
    }
    resize_overlay_window(&app, EXPANDED_WIDTH).map_err(|error| error.to_string())
}

#[tauri::command]
fn peek_panel(app: tauri::AppHandle) -> Result<(), String> {
    resize_overlay_window(&app, PEEK_WIDTH).map_err(|error| error.to_string())
}

#[tauri::command]
fn collapse_panel(app: tauri::AppHandle) -> Result<(), String> {
    resize_overlay_window(&app, COLLAPSED_WIDTH).map_err(|error| error.to_string())
}

fn resize_overlay_window(app: &tauri::AppHandle, logical_width: f64) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window("main") else {
        return Ok(());
    };

    let Some(monitor) = window.primary_monitor()? else {
        return Ok(());
    };

    let scale = monitor.scale_factor();
    let size = monitor.size();
    let pos = monitor.position();

    let physical_width = (logical_width * scale).round().max(1.0) as i32;
    let height = size.height;
    let x = pos.x + size.width as i32 - physical_width;
    let y = pos.y;

    window.set_size(PhysicalSize::new(physical_width as u32, height))?;
    window.set_position(PhysicalPosition::new(x, y))?;
    Ok(())
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