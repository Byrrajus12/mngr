use chrono::{DateTime, Utc};
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
const STATUS_LINE_SCRIPT_NAME: &str = "claude-statusline.ps1";
const STATUS_LINE_ORIGINAL_COMMAND_NAME: &str = "claude-statusline-original.txt";
const MNGR_ORIGINAL_STATUS_LINE_KEY: &str = "_mngrOriginalStatusLine";
const STATUS_LINE_REFRESH_INTERVAL_MS: u64 = 5000;
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
    pub wt_session: Option<String>,
    #[serde(default)]
    pub hook_pid: Option<u32>,
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
    pub wt_session: Option<String>,
    pub hook_pid: Option<u32>,
    pub terminal_window_hwnd: Option<isize>,
    pub started_at: u64,
    pub last_event_at: u64,
    pub current_tool: Option<String>,
    pub permission_mode: Option<String>,
    pub pending_approval: Option<PendingRequest>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ClaudeUsageWindow {
    pub used_percentage: f64,
    pub resets_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ClaudeUsageState {
    pub five_hour: Option<ClaudeUsageWindow>,
    pub seven_day: Option<ClaudeUsageWindow>,
    pub last_updated: Option<u64>,
}

impl ClaudeUsageState {
    fn empty() -> Self {
        Self {
            five_hour: None,
            seven_day: None,
            last_updated: None,
        }
    }
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
        if payload.wt_session.is_some() && session.wt_session != payload.wt_session {
            session.wt_session = payload.wt_session.clone();
            session.terminal_window_hwnd = None;
        }
        if payload.hook_pid.is_some() {
            session.hook_pid = payload.hook_pid;
        }
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
            wt_session: payload.wt_session.clone(),
            hook_pid: payload.hook_pid,
            terminal_window_hwnd: None,
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

fn choose_terminal_hwnd(hwnds: &[isize]) -> Result<isize, String> {
    match hwnds {
        [] => Err("No Windows Terminal window found -- this session may be running in a different terminal".to_string()),
        [hwnd] => Ok(*hwnd),
        [hwnd, ..] => Ok(*hwnd),
    }
}

#[cfg(windows)]
fn resolve_terminal_hwnd() -> Result<isize, String> {
    choose_terminal_hwnd(&windows_terminal_hwnds()?)
}

#[cfg(not(windows))]
fn resolve_terminal_hwnd() -> Result<isize, String> {
    Err("jump to terminal is only available on Windows".to_string())
}

#[cfg(windows)]
fn windows_terminal_process_ids() -> Result<Vec<u32>, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err("could not enumerate processes".to_string());
        }

        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..std::mem::zeroed()
        };
        let mut pids = Vec::new();
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|ch| *ch == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
                if name.eq_ignore_ascii_case("WindowsTerminal.exe") {
                    pids.push(entry.th32ProcessID);
                }
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
        Ok(pids)
    }
}

#[cfg(windows)]
fn windows_terminal_hwnds() -> Result<Vec<isize>, String> {
    use std::collections::HashSet;
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsWindowVisible,
    };

    struct Search {
        pids: HashSet<u32>,
        hwnds: Vec<isize>,
    }

    unsafe extern "system" fn enum_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = &mut *(lparam as *mut Search);
        let mut window_pid = 0;
        GetWindowThreadProcessId(hwnd, &mut window_pid);
        if search.pids.contains(&window_pid) && IsWindowVisible(hwnd) != 0 {
            search.hwnds.push(hwnd as isize);
        }
        1
    }

    let mut search = Search {
        pids: windows_terminal_process_ids()?.into_iter().collect(),
        hwnds: Vec::new(),
    };
    unsafe {
        EnumWindows(Some(enum_window), &mut search as *mut Search as LPARAM);
    }
    Ok(search.hwnds)
}
#[cfg(windows)]
fn terminal_window_exists(hwnd: isize) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsWindow;
    hwnd != 0 && unsafe { IsWindow(hwnd as _) != 0 }
}

#[cfg(not(windows))]
fn terminal_window_exists(_hwnd: isize) -> bool {
    false
}

#[cfg(windows)]
fn focus_terminal_window(hwnd: isize) -> Result<(), String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IsIconic, SetForegroundWindow, ShowWindowAsync, SW_RESTORE,
    };

    if !terminal_window_exists(hwnd) {
        return Err("terminal window no longer exists".to_string());
    }

    unsafe {
        if IsIconic(hwnd as _) != 0 {
            ShowWindowAsync(hwnd as _, SW_RESTORE);
        }
        if SetForegroundWindow(hwnd as _) == 0 {
            return Err("failed to focus terminal window".to_string());
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn focus_terminal_window(_hwnd: isize) -> Result<(), String> {
    Err("jump to terminal is only available on Windows".to_string())
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
fn jump_to_terminal(store: State<'_, SessionStore>, session_id: String) -> Result<(), String> {
    let hwnd = {
        let mut manager = store.0.lock().expect("session manager mutex poisoned");
        let session = manager
            .sessions
            .get_mut(&session_id)
            .ok_or_else(|| "session not found".to_string())?;

        if let Some(hwnd) = session
            .terminal_window_hwnd
            .filter(|hwnd| terminal_window_exists(*hwnd))
        {
            hwnd
        } else {
            let hwnd = resolve_terminal_hwnd()?;
            session.terminal_window_hwnd = Some(hwnd);
            hwnd
        }
    };

    focus_terminal_window(hwnd)
}

#[tauri::command]
fn install_claude_hooks() -> Result<String, String> {
    install_claude_hooks_inner().map_err(|error| error.to_string())
}

#[tauri::command]
fn get_claude_usage() -> Result<ClaudeUsageState, String> {
    read_claude_usage_cache(&claude_usage_cache_path()).map_err(|error| error.to_string())
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
    install_claude_hooks_at(&settings_path, &mngr_dir)?;
    Ok(settings_path.display().to_string())
}

fn install_claude_hooks_at(settings_path: &Path, mngr_dir: &Path) -> Result<(), InstallError> {
    let hook_script = mngr_dir.join("claude-hook.ps1");
    let status_line_script = mngr_dir.join(STATUS_LINE_SCRIPT_NAME);
    fs::create_dir_all(mngr_dir)?;
    write_if_changed(
        &hook_script,
        include_str!("../../scripts/claude-hook.ps1").as_bytes(),
    )?;
    write_if_changed(
        &status_line_script,
        include_str!("../../scripts/claude-statusline.ps1").as_bytes(),
    )?;

    let mut settings = load_json_object(settings_path)?;
    let original_settings = settings.clone();
    let settings_object = settings
        .as_object_mut()
        .ok_or_else(|| InstallError::InvalidSettingsShape(settings_path.to_path_buf()))?;

    let hooks_value = settings_object.entry("hooks").or_insert_with(|| json!({}));
    let hooks_object = hooks_value
        .as_object_mut()
        .ok_or(InstallError::InvalidHooksShape)?;

    let hook_command = powershell_file_command(&hook_script);
    for event_name in CLAUDE_EVENTS {
        append_hook_group(hooks_object, event_name, &hook_command);
    }

    configure_status_line(settings_object, &status_line_script, mngr_dir)?;

    if settings != original_settings {
        write_if_changed(
            settings_path,
            (serde_json::to_string_pretty(&settings)? + "\n").as_bytes(),
        )?;
    }

    Ok(())
}

fn write_if_changed(path: &Path, contents: &[u8]) -> Result<(), std::io::Error> {
    if fs::read(path).is_ok_and(|current| current == contents) {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}

fn load_json_object(path: &Path) -> Result<Value, InstallError> {
    if path.exists() {
        let contents = fs::read_to_string(path)?;
        if contents.trim().is_empty() {
            Ok(json!({}))
        } else {
            Ok(serde_json::from_str(&contents)?)
        }
    } else {
        Ok(json!({}))
    }
}

fn powershell_file_command(script: &Path) -> String {
    format!(
        "powershell.exe -NoProfile -ExecutionPolicy Bypass -File \"{}\"",
        script.display()
    )
}

fn configure_status_line(
    settings_object: &mut serde_json::Map<String, Value>,
    status_line_script: &Path,
    mngr_dir: &Path,
) -> Result<(), InstallError> {
    let managed_command = powershell_file_command(status_line_script);
    let original_command_path = mngr_dir.join(STATUS_LINE_ORIGINAL_COMMAND_NAME);
    let current = settings_object.get("statusLine").cloned();

    if let Some(current_status_line) = current.as_ref() {
        if !is_mngr_status_line(current_status_line, &managed_command) {
            if let Some(command) = current_status_line.get("command").and_then(Value::as_str) {
                if !command.trim().is_empty() {
                    write_if_changed(&original_command_path, command.as_bytes())?;
                }
            }
            settings_object.insert(
                MNGR_ORIGINAL_STATUS_LINE_KEY.to_string(),
                current_status_line.clone(),
            );
        }
    }

    settings_object.insert(
        "statusLine".to_string(),
        json!({
            "type": "command",
            "command": managed_command,
            "refreshInterval": STATUS_LINE_REFRESH_INTERVAL_MS
        }),
    );

    Ok(())
}

fn is_mngr_status_line(status_line: &Value, managed_command: &str) -> bool {
    status_line
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| {
            command == managed_command || command.contains(STATUS_LINE_SCRIPT_NAME)
        })
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

fn local_app_data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

fn responses_dir() -> PathBuf {
    local_app_data_dir().join("mngr").join("responses")
}

fn claude_usage_cache_path() -> PathBuf {
    local_app_data_dir().join("mngr").join("claude-usage.json")
}

fn read_claude_usage_cache(path: &Path) -> Result<ClaudeUsageState, std::io::Error> {
    if !path.exists() {
        return Ok(ClaudeUsageState::empty());
    }

    let contents = fs::read_to_string(path)?;
    let payload = serde_json::from_str::<Value>(&contents).unwrap_or(Value::Null);
    let last_updated = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(system_time_ms);

    Ok(ClaudeUsageState {
        five_hour: parse_usage_window(payload.get("five_hour")),
        seven_day: parse_usage_window(payload.get("seven_day")),
        last_updated,
    })
}

fn parse_usage_window(value: Option<&Value>) -> Option<ClaudeUsageWindow> {
    let object = value?.as_object()?;
    let used_percentage = number_value(object.get("used_percentage"))
        .or_else(|| number_value(object.get("utilization")))?;
    Some(ClaudeUsageWindow {
        used_percentage,
        resets_at: object.get("resets_at").and_then(reset_time_ms),
    })
}

fn number_value(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.parse::<f64>().ok(),
        _ => None,
    }
}

fn reset_time_ms(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_f64().and_then(seconds_to_ms),
        Value::String(text) => text
            .parse::<f64>()
            .ok()
            .and_then(seconds_to_ms)
            .or_else(|| {
                DateTime::parse_from_rfc3339(text)
                    .ok()
                    .map(|date| date.with_timezone(&Utc).timestamp_millis())
                    .and_then(|ms| u64::try_from(ms).ok())
            }),
        _ => None,
    }
}

fn seconds_to_ms(seconds: f64) -> Option<u64> {
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    Some((seconds * 1000.0).round() as u64)
}

fn system_time_ms(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as u64)
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
            if let Err(error) = install_claude_hooks_inner() {
                eprintln!("failed to install Claude Code integration: {error}");
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
            get_claude_usage,
            jump_to_terminal,
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
            wt_session: Some("wt-session-1".to_string()),
            hook_pid: Some(4242),
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
            wt_session: Some("wt-session-1".to_string()),
            hook_pid: Some(4242),
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
            wt_session: Some("wt-session-1".to_string()),
            hook_pid: Some(4242),
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
    fn terminal_window_selection_uses_single_window() {
        let hwnd = choose_terminal_hwnd(&[111]);

        assert_eq!(hwnd, Ok(111));
    }

    #[test]
    fn terminal_window_selection_uses_first_window_when_multiple_exist() {
        let hwnd = choose_terminal_hwnd(&[222, 111]);

        assert_eq!(hwnd, Ok(222));
    }

    #[test]
    fn terminal_window_selection_fails_when_no_windows_exist() {
        let error = choose_terminal_hwnd(&[]).expect_err("expected no window failure");

        assert_eq!(
            error,
            "No Windows Terminal window found -- this session may be running in a different terminal"
        );
    }

    #[test]
    fn session_stores_terminal_identity_from_hook_payload() {
        let mut manager = SessionManager::default();

        let sessions = manager.apply_event(session_event_payload("UserPromptSubmit"));

        assert_eq!(sessions[0].wt_session.as_deref(), Some("wt-session-1"));
        assert_eq!(sessions[0].hook_pid, Some(4242));
        assert_eq!(sessions[0].terminal_window_hwnd, None);
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
    fn claude_usage_cache_parses_percentage_shapes_and_reset_formats() {
        let dir = test_dir("claude-usage-cache");
        let path = dir.join("claude-usage.json");
        fs::write(
            &path,
            json!({
                "five_hour": {
                    "used_percentage": 59,
                    "resets_at": 1785102600
                },
                "seven_day": {
                    "utilization": "14.5",
                    "resets_at": "2026-02-09T12:00:00.462679+00:00"
                }
            })
            .to_string(),
        )
        .expect("write usage cache");

        let usage = read_claude_usage_cache(&path).expect("read usage cache");

        assert_eq!(usage.five_hour.as_ref().unwrap().used_percentage, 59.0);
        assert_eq!(
            usage.five_hour.as_ref().unwrap().resets_at,
            Some(1_785_102_600_000)
        );
        assert_eq!(usage.seven_day.as_ref().unwrap().used_percentage, 14.5);
        let expected_iso = DateTime::parse_from_rfc3339("2026-02-09T12:00:00.462679+00:00")
            .unwrap()
            .timestamp_millis() as u64;
        assert_eq!(
            usage.seven_day.as_ref().unwrap().resets_at,
            Some(expected_iso)
        );
        assert!(usage.last_updated.is_some());
    }

    #[test]
    fn missing_claude_usage_cache_returns_empty_state() {
        let dir = test_dir("claude-usage-missing");

        let usage = read_claude_usage_cache(&dir.join("missing.json")).expect("read missing usage");

        assert_eq!(usage, ClaudeUsageState::empty());
    }

    #[test]
    fn installer_preserves_settings_and_wraps_existing_status_line() {
        let dir = test_dir("settings-wrap");
        let settings_path = dir.join("settings.json");
        let mngr_dir = dir.join("mngr");
        fs::write(
            &settings_path,
            serde_json::to_string_pretty(&json!({
                "hooks": {
                    "Stop": [
                        {
                            "hooks": [
                                { "type": "command", "command": "Write-Host done" }
                            ]
                        }
                    ]
                },
                "enabledPlugins": {
                    "claude-mem@thedotmack": true
                },
                "statusLine": {
                    "type": "command",
                    "command": "powershell.exe -NoProfile -Command \"Write-Output custom\"",
                    "padding": 2
                }
            }))
            .unwrap(),
        )
        .expect("write settings");

        install_claude_hooks_at(&settings_path, &mngr_dir).expect("install hooks");

        let settings: Value =
            serde_json::from_str(&fs::read_to_string(&settings_path).expect("read settings"))
                .expect("parse settings");
        assert_eq!(settings["enabledPlugins"]["claude-mem@thedotmack"], true);
        assert_eq!(
            settings["hooks"]["Stop"][0]["hooks"][0]["command"],
            "Write-Host done"
        );
        assert!(settings["hooks"]["PreToolUse"].is_array());
        let status_command = settings["statusLine"]["command"].as_str().unwrap();
        assert!(status_command.contains(STATUS_LINE_SCRIPT_NAME));
        assert_eq!(
            settings["statusLine"]["refreshInterval"],
            STATUS_LINE_REFRESH_INTERVAL_MS
        );
        assert_eq!(
            settings[MNGR_ORIGINAL_STATUS_LINE_KEY]["command"],
            "powershell.exe -NoProfile -Command \"Write-Output custom\""
        );
        assert_eq!(
            fs::read_to_string(mngr_dir.join(STATUS_LINE_ORIGINAL_COMMAND_NAME))
                .expect("read original command"),
            "powershell.exe -NoProfile -Command \"Write-Output custom\""
        );
    }

    #[test]
    fn install_if_needed_is_noop_when_current() {
        use std::thread::sleep;
        use std::time::Duration;

        let dir = test_dir("settings-noop");
        let settings_path = dir.join("settings.json");
        let mngr_dir = dir.join("mngr");

        install_claude_hooks_at(&settings_path, &mngr_dir).expect("first install");
        let settings_before = fs::read_to_string(&settings_path).expect("read settings before");
        let settings_modified_before = fs::metadata(&settings_path)
            .expect("settings metadata before")
            .modified()
            .expect("settings modified before");
        let hook_modified_before = fs::metadata(mngr_dir.join("claude-hook.ps1"))
            .expect("hook metadata before")
            .modified()
            .expect("hook modified before");
        let status_modified_before = fs::metadata(mngr_dir.join(STATUS_LINE_SCRIPT_NAME))
            .expect("status metadata before")
            .modified()
            .expect("status modified before");

        sleep(Duration::from_millis(1200));
        install_claude_hooks_at(&settings_path, &mngr_dir).expect("second install");

        assert_eq!(
            fs::read_to_string(&settings_path).expect("read settings after"),
            settings_before
        );
        assert_eq!(
            fs::metadata(&settings_path)
                .expect("settings metadata after")
                .modified()
                .expect("settings modified after"),
            settings_modified_before
        );
        assert_eq!(
            fs::metadata(mngr_dir.join("claude-hook.ps1"))
                .expect("hook metadata after")
                .modified()
                .expect("hook modified after"),
            hook_modified_before
        );
        assert_eq!(
            fs::metadata(mngr_dir.join(STATUS_LINE_SCRIPT_NAME))
                .expect("status metadata after")
                .modified()
                .expect("status modified after"),
            status_modified_before
        );
    }

    #[cfg(windows)]
    #[test]
    fn statusline_script_writes_cache_only_when_rate_limits_change() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        use std::thread::sleep;
        use std::time::Duration;

        let dir = test_dir("statusline-script");
        let local_app_data = dir.join("local-app-data");
        fs::create_dir_all(&local_app_data).expect("create local app data");
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("scripts")
            .join("claude-statusline.ps1");
        let payload = json!({
            "model": { "display_name": "Opus" },
            "context_window": { "used_percentage": 31 },
            "rate_limits": {
                "five_hour": { "used_percentage": 59, "resets_at": 1785102600 },
                "seven_day": { "used_percentage": 14, "resets_at": 1785351600 }
            }
        })
        .to_string();

        let run_script = |input: &str| {
            let mut child = Command::new("powershell.exe")
                .arg("-NoProfile")
                .arg("-ExecutionPolicy")
                .arg("Bypass")
                .arg("-File")
                .arg(&script)
                .env("LOCALAPPDATA", &local_app_data)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .expect("spawn statusline script");
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(input.as_bytes())
                .expect("write stdin");
            child.wait_with_output().expect("wait statusline script")
        };

        let first = run_script(&payload);
        assert!(first.status.success());
        assert!(String::from_utf8_lossy(&first.stdout).contains("[Opus] 31% context"));
        let cache_path = local_app_data.join("mngr").join("claude-usage.json");
        let first_cache = fs::read_to_string(&cache_path).expect("read first cache");
        assert!(first_cache.contains("five_hour"));
        let first_modified = fs::metadata(&cache_path)
            .expect("first metadata")
            .modified()
            .expect("first modified");

        sleep(Duration::from_millis(1200));
        let second = run_script(&payload);
        assert!(second.status.success());
        let second_modified = fs::metadata(&cache_path)
            .expect("second metadata")
            .modified()
            .expect("second modified");
        assert_eq!(first_modified, second_modified);

        let changed_payload = payload.replace("59", "60");
        sleep(Duration::from_millis(1200));
        let third = run_script(&changed_payload);
        assert!(third.status.success());
        let third_modified = fs::metadata(&cache_path)
            .expect("third metadata")
            .modified()
            .expect("third modified");
        assert!(third_modified > second_modified);
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
