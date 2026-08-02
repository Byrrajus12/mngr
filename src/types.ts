export type SessionStatus =
  | "Working"
  | "WaitingForApproval"
  | "WaitingForInput"
  | "Idle"
  | "Done"
  | "Error";

export type PermissionSuggestion = Record<string, unknown>;

export type ApprovalRequest = {
  kind: "permission";
  request_id: string;
  tool_name: string;
  tool_input: unknown;
  computed_diff?: string | null;
  permission_mode?: string | null;
  permission_suggestions: PermissionSuggestion[];
};

export type QuestionOption = {
  label: string;
  description: string;
};

export type UserQuestion = {
  question: string;
  header?: string | null;
  multiSelect: boolean;
  options: QuestionOption[];
};

export type QuestionRequest = {
  kind: "question";
  request_id: string;
  questions: UserQuestion[];
};

export type PendingRequest = ApprovalRequest | QuestionRequest;

export type Session = {
  session_id: string;
  agent_type: "claude-code" | "codex" | string;
  status: SessionStatus;
  project_path: string;
  project_name: string;
  wt_session?: string | null;
  hook_pid?: number | null;
  shell_pid?: number | null;
  started_at: number;
  last_event_at: number;
  current_tool?: string | null;
  permission_mode?: string | null;
  pending_approval?: PendingRequest | null;
};

export type ClaudeUsageWindow = {
  used_percentage: number;
  resets_at?: number | null;
};

export type ClaudeUsageState = {
  five_hour?: ClaudeUsageWindow | null;
  seven_day?: ClaudeUsageWindow | null;
  last_updated?: number | null;
};
