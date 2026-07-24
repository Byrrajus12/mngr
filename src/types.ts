export type SessionStatus =
  | "Working"
  | "WaitingForApproval"
  | "WaitingForInput"
  | "Idle"
  | "Done"
  | "Error";

export type PermissionSuggestion = Record<string, unknown>;

export type ApprovalRequest = {
  request_id: string;
  tool_name: string;
  tool_input: unknown;
  permission_mode?: string | null;
  permission_suggestions: PermissionSuggestion[];
};

export type Session = {
  session_id: string;
  agent_type: "claude-code" | "codex" | string;
  status: SessionStatus;
  project_path: string;
  project_name: string;
  started_at: number;
  last_event_at: number;
  current_tool?: string | null;
  permission_mode?: string | null;
  pending_approval?: ApprovalRequest | null;
};
