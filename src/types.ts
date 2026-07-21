export type SessionStatus =
  | "Working"
  | "WaitingForApproval"
  | "WaitingForInput"
  | "Idle"
  | "Done"
  | "Error";

export type ApprovalRequest = {
  tool_name: string;
  tool_input: unknown;
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
  pending_approval?: ApprovalRequest | null;
};