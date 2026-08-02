import { invoke } from "@tauri-apps/api/core";
import { useState } from "react";
import claudeCodeLogo from "../assets/providers/claudecode.svg";
import codexLogo from "../assets/providers/codex.svg";
import type { PermissionSuggestion, QuestionRequest, Session } from "../types";

type SessionCardProps = {
  session: Session;
  index: number;
  now: number;
  onDismiss: () => void;
};

function elapsed(startedAt: number, now: number, lastEventAt?: number) {
  const base = lastEventAt && lastEventAt > startedAt ? lastEventAt : now;
  let seconds = Math.max(0, Math.floor((base - startedAt) / 1000));
  const minutes = Math.floor(seconds / 60);
  seconds %= 60;
  if (minutes >= 60) {
    const hours = Math.floor(minutes / 60);
    return `${hours}h ${minutes % 60}m`;
  }
  return `${minutes}m ${String(seconds).padStart(2, "0")}s`;
}

function glyph(session: Session) {
  if (session.agent_type === "codex") {
    return <img src={codexLogo} alt="" width="14" height="14" style={{ display: "block" }} />;
  }

  return <img src={claudeCodeLogo} alt="" width="14" height="14" style={{ display: "block" }} />;
}

function statusClass(session: Session) {
  if (session.status === "WaitingForApproval" && session.pending_approval?.kind === "question") return "q";
  if (session.status === "WaitingForApproval") return "perm";
  if (session.status === "WaitingForInput") return "q";
  if (session.status === "Error") return "err";
  if (session.status === "Idle") return "idle";
  if (session.status === "Done") return "done";
  return "working";
}

function shouldFreezeElapsed(session: Session) {
  return session.status === "Idle" || session.status === "Done" || session.status === "Error";
}

function statusText(session: Session, now: number) {
  if (session.status === "WaitingForApproval" && session.pending_approval?.kind === "question") return "Waiting for your answer";
  if (session.status === "WaitingForApproval") return "Waiting for permission";
  if (session.status === "WaitingForInput") return "Waiting for your answer";
  if (session.status === "Error") return "Session stopped responding";
  if (session.status === "Done") return `Finished - ${elapsed(session.started_at, now, session.last_event_at)}`;
  if (session.status === "Idle") return `Idle - ${elapsed(session.started_at, now, session.last_event_at)}`;
  if (session.current_tool) return `Using ${session.current_tool}`;
  return "Working";
}

function commandText(session: Session) {
  const pending = session.pending_approval;
  if (!pending || pending.kind !== "permission") return "command";
  const name = pending.tool_name || "command";
  const input = pending.tool_input;

  if (input && typeof input === "object") {
    const obj = input as Record<string, unknown>;
    if (typeof obj.command === "string") return obj.command;
    if (typeof obj.file_path === "string") return `${name} ${obj.file_path}`;
    const compact = JSON.stringify(obj);
    return `${name} ${compact.length > 64 ? `${compact.slice(0, 63)}...` : compact}`;
  }
  if (input == null) return name;
  return `${name} ${String(input)}`;
}

function providerName(session: Session) {
  if (session.agent_type === "claude-code") return "Claude Code";
  if (session.agent_type === "codex") return "Codex";
  return session.agent_type;
}

function toolName(session: Session) {
  if (session.pending_approval?.kind === "permission") return session.pending_approval.tool_name || "command";
  return session.current_tool || "Working";
}

function targetText(input: unknown) {
  if (!input || typeof input !== "object") return null;
  const obj = input as Record<string, unknown>;
  for (const field of ["file_path", "path", "target", "cwd"]) {
    if (typeof obj[field] === "string") return obj[field] as string;
  }
  if (typeof obj.command === "string") return obj.command;
  return null;
}

type DiffLine = {
  kind: "add" | "del" | "ctx";
  line: string;
  number: number | null;
};

function diffText(input: unknown, computedDiff?: string | null) {
  if (computedDiff?.trim()) return computedDiff;
  if (!input || typeof input !== "object") return null;
  const obj = input as Record<string, unknown>;
  for (const field of ["diff", "patch", "edit_diff"]) {
    if (typeof obj[field] === "string" && (obj[field] as string).trim()) return obj[field] as string;
  }
  return null;
}

function parseDiff(input: unknown, computedDiff?: string | null) {
  const text = diffText(input, computedDiff);
  if (!text) return null;

  let nextLine = 1;
  let adds = 0;
  let dels = 0;
  const lines: DiffLine[] = [];

  for (const raw of text.split(/\r?\n/)) {
    if (!raw || raw.startsWith("+++") || raw.startsWith("---")) continue;
    const hunk = raw.match(/^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
    if (hunk) {
      nextLine = Number(hunk[1]);
      continue;
    }

    const marker = raw[0];
    if (marker === "+") {
      adds += 1;
      lines.push({ kind: "add", line: raw, number: nextLine });
      nextLine += 1;
    } else if (marker === "-") {
      dels += 1;
      lines.push({ kind: "del", line: raw, number: null });
    } else {
      lines.push({ kind: "ctx", line: raw.startsWith(" ") ? raw : ` ${raw}`, number: nextLine });
      nextLine += 1;
    }
  }

  return lines.length ? { lines, adds, dels } : null;
}

function demoQuestions(): QuestionRequest["questions"] {
  return [
    {
      question: "Claude Code has a question before continuing.",
      header: null,
      multiSelect: false,
      options: [
        { label: "Continue", description: "Resume the demo session." },
        { label: "Explain first", description: "Ask for more context before continuing." },
        { label: "Stop", description: "Stop this demo path." },
      ],
    },
  ];
}

function suggestionLabel(suggestion: PermissionSuggestion) {
  const type = typeof suggestion.type === "string" ? suggestion.type : "permission";
  const destination = typeof suggestion.destination === "string" ? suggestion.destination : "session";

  if (type === "addRules") {
    if (Array.isArray(suggestion.rules) && suggestion.rules.length > 0) {
      return `Add ${suggestion.rules.length} rules for this ${destination}`;
    }
    return `Add rules for this ${destination}`;
  }

  if (type === "addDirectories") {
    if (Array.isArray(suggestion.directories)) {
      const dirs = suggestion.directories.filter((dir): dir is string => typeof dir === "string");
      const objectDirCount = suggestion.directories.length - dirs.length;
      if (dirs.length === 1 && objectDirCount === 0) return `Allow access to ${dirs[0]} for this ${destination}`;
      if (dirs.length + objectDirCount > 0) return `Allow access to ${dirs.length + objectDirCount} directories for this ${destination}`;
    }
    return `Allow access to directories for this ${destination}`;
  }

  if (type === "setMode" && typeof suggestion.mode === "string") {
    return `Use ${suggestion.mode} mode for this ${destination}`;
  }

  function formatValue(value: unknown) {
    if (value && typeof value === "object") {
      const json = JSON.stringify(value);
      if (!json) return "";
      return json.length > 96 ? `${json.slice(0, 95)}...` : json;
    }
    return String(value);
  }

  const fields = Object.entries(suggestion)
    .filter(([key]) => key !== "type")
    .map(([key, value]) => `${key}: ${formatValue(value)}`)
    .join(", ");
  return fields ? `Apply ${type} (${fields})` : `Apply ${type}`;
}
function SessionCard({ session, index, now, onDismiss }: SessionCardProps) {
  const cls = statusClass(session);
  const isDone = cls === "done";
  const isDismissible = isDone || cls === "err";
  const isDemo = session.session_id.startsWith("demo-");
  const [resolved, setResolved] = useState<string | null>(null);
  const [resolving, setResolving] = useState(false);
  const [reason, setReason] = useState("");
  const [pendingAction, setPendingAction] = useState<string | null>(null);
  const [approvalError, setApprovalError] = useState<string | null>(null);
  const [terminalJumpError, setTerminalJumpError] = useState<string | null>(null);
  const [selectedAnswers, setSelectedAnswers] = useState<Record<string, string[]>>({});

  const pendingQuestion = session.pending_approval?.kind === "question" ? session.pending_approval : null;
  const pendingPermission = session.pending_approval?.kind === "permission" ? session.pending_approval : null;
  const showApproval = session.status === "WaitingForApproval" && session.pending_approval?.kind !== "question" && !resolved;
  const showQuestion = ((session.status === "WaitingForApproval" && !!pendingQuestion) || session.status === "WaitingForInput") && !resolved;
  const parsedDiff = pendingPermission ? parseDiff(pendingPermission.tool_input, pendingPermission.computed_diff) : null;

  function beginResolveAnimation() {
    setResolving(true);
  }

  function handleDismiss() {
    beginResolveAnimation();
    window.setTimeout(onDismiss, 450);
  }

  async function resolveRealApproval(decision: "allow" | "deny", updatedPermissions?: PermissionSuggestion[]) {
    const requestId = session.pending_approval?.kind === "permission" ? session.pending_approval.request_id : null;
    if (!requestId) {
      setApprovalError("Missing approval request id");
      return;
    }

    setPendingAction(updatedPermissions ? "suggestion" : decision);
    setApprovalError(null);
    try {
      await invoke("resolve_approval", {
        requestId,
        decision,
        reason: decision === "deny" ? reason.trim() || null : null,
        updatedPermissions: updatedPermissions ?? null,
      });
    } catch (error) {
      setApprovalError(error instanceof Error ? error.message : String(error));
    } finally {
      setPendingAction(null);
    }
  }


  async function handleJumpToTerminal() {
    if (isDemo) {
      console.log("jump", session.session_id);
      return;
    }

    setTerminalJumpError(null);
    try {
      await invoke("jump_to_terminal", { sessionId: session.session_id });
    } catch (error) {
      setTerminalJumpError(error instanceof Error ? error.message : String(error));
    }
  }
  function handleAllow(updatedPermissions?: PermissionSuggestion[]) {
    if (isDemo) {
      console.log("mngr allow clicked", session.session_id, updatedPermissions);
      setResolved("Allowed - resuming");
      return;
    }

    resolveRealApproval("allow", updatedPermissions);
  }

  function handleDeny() {
    if (isDemo) {
      console.log("mngr deny clicked", session.session_id);
      setResolved("Denied - resuming");
      return;
    }

    resolveRealApproval("deny");
  }

  async function resolveRealQuestion(question: string, answer: string) {
    const requestId = pendingQuestion?.request_id;
    if (!requestId) {
      setApprovalError("Missing question request id");
      return;
    }

    setPendingAction(answer);
    setApprovalError(null);
    try {
      await invoke("resolve_question", { requestId, question, answer });
    } catch (error) {
      setApprovalError(error instanceof Error ? error.message : String(error));
    } finally {
      setPendingAction(null);
    }
  }

  function handleQuestionAnswer(question: string, answer: string) {
    if (isDemo || !pendingQuestion) {
      console.log("question option", answer, session.session_id);
      setResolved(`-> ${answer}`);
      return;
    }

    resolveRealQuestion(question, answer);
  }

  function toggleMultiAnswer(question: string, answer: string) {
    setSelectedAnswers((current) => {
      const selected = current[question] ?? [];
      const next = selected.includes(answer) ? selected.filter((item) => item !== answer) : [...selected, answer];
      return { ...current, [question]: next };
    });
  }

  function handleMultiSubmit(question: string) {
    const answer = (selectedAnswers[question] ?? []).join(", ");
    handleQuestionAnswer(question, answer);
  }

  function rowLine() {
    if (session.status === "WaitingForApproval" && pendingPermission) {
      return <span className="statusword">PERMISSION</span>;
    }
    if (showQuestion) {
      const header = (pendingQuestion?.questions ?? demoQuestions()).find((question) => question.header)?.header;
      return <span className="statusword">QUESTION{header ? ` · ${header.toUpperCase()}` : ""}</span>;
    }
    if (session.status === "Error") return <span className="statusword">ERROR</span>;
    if (session.status === "Done") return <span className="dismiss">finished - click to dismiss</span>;
    if (session.status === "Idle") return "idle - waiting for next task";
    if (session.current_tool) {
      return (
        <>
          <span className="verb">{session.current_tool}</span> {session.project_path}
        </>
      );
    }
    return statusText(session, now);
  }

  return (
    <article
      className={`card row ${cls} ${resolving ? "resolving" : ""}`}
      style={{ animationDelay: `${index * 40 + 40}ms` }}
      onClick={isDismissible ? handleDismiss : undefined}
    >
      <div className="r1">
        <span className="led">
          {isDone ? <svg className="checkwrap" viewBox="0 0 14 14"><path className="check" d="M2.5 7.5l3 3 6-7" /></svg> : null}
        </span>
        <span className="glyph" aria-hidden="true">
          {glyph(session)}
        </span>
        <span className="proj">{session.project_name}</span>
        <span className="prov">{providerName(session)}</span>
        <span className="el">{elapsed(session.started_at, now, shouldFreezeElapsed(session) ? session.last_event_at : undefined)}</span>
        <button
          className="jump"
          type="button"
          aria-label="Jump to terminal"
          onClick={(event) => {
            event.stopPropagation();
            handleJumpToTerminal();
          }}
        >
          <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><path d="M3 9L9 3M4.5 3H9v4.5" stroke="currentColor" strokeWidth="1.3" fill="none" strokeLinecap="round" strokeLinejoin="round" /></svg>
        </button>
      </div>

      <div className="r2">{rowLine()}</div>

      <div className={`tray ${showApproval ? "" : "hidden"}`}>
        {showApproval ? (
          <>
          <div className="trayhead">
            <span>{toolName(session)}</span>
            {targetText(pendingPermission?.tool_input) ? <span className="path">{targetText(pendingPermission?.tool_input)}</span> : null}
          </div>
          {parsedDiff ? (
            <>
              <div className="diff">
                {parsedDiff.lines.map((line, lineIndex) => (
                  <div className={`dl ${line.kind}`} key={`${line.kind}-${lineIndex}`}>
                    <span className="ln">{line.number ?? ""}</span>
                    <span>{line.line}</span>
                  </div>
                ))}
              </div>
              <div className="diffstat"><b className="a">+{parsedDiff.adds}</b> <b className="d">-{parsedDiff.dels}</b></div>
            </>
          ) : (
            <div className="cmdchip">{commandText(session)}</div>
          )}
          {!isDemo ? (
            <input
              className="reasonInput"
              type="text"
              value={reason}
              onChange={(event) => setReason(event.target.value)}
              placeholder="optional deny reason"
              aria-label="Optional deny reason"
            />
          ) : null}
          <div className="actions">
            <button
              className="deny"
              type="button"
              disabled={pendingAction !== null}
              onClick={handleDeny}
            >
              {pendingAction === "deny" ? "Denying…" : "Deny"}
            </button>
            <button
              className="allow"
              type="button"
              disabled={pendingAction !== null}
              onClick={() => handleAllow()}
            >
              {pendingAction === "allow" ? "Allowing…" : "Allow"}
            </button>
            {(session.pending_approval?.kind === "permission" ? session.pending_approval.permission_suggestions : []).map((suggestion, suggestionIndex) => (
              <button
                className="sugg"
                type="button"
                disabled={pendingAction !== null}
                key={suggestionIndex}
                onClick={() => handleAllow([suggestion])}
              >
                {pendingAction === "suggestion" ? "Applying…" : suggestionLabel(suggestion)}
              </button>
            ))}
          </div>
          {approvalError ? <div className="cardError">{approvalError}</div> : null}
          </>
        ) : null}
      </div>

      <div className={`tray questionTray ${showQuestion ? "" : "hidden"}`}>
        {showQuestion ? (
          <>
          {(pendingQuestion?.questions ?? demoQuestions()).map((question) => (
            <div className="questionBlock" key={question.question}>
              <div className="qtext">{question.question}</div>
              <div className="opts">
                {question.options.map((option) => {
                  const selected = (selectedAnswers[question.question] ?? []).includes(option.label);
                  return (
                    <button
                      className={`opt ${selected ? "selected" : ""}`}
                      type="button"
                      disabled={pendingAction !== null}
                      key={option.label}
                      onClick={() =>
                        question.multiSelect
                          ? toggleMultiAnswer(question.question, option.label)
                          : handleQuestionAnswer(question.question, option.label)
                      }
                    >
                      <b>{question.multiSelect && selected ? `✓ ${option.label}` : pendingAction === option.label ? "Answering…" : option.label}</b>
                      <span className="pillDesc">{option.description}</span>
                    </button>
                  );
                })}
                {question.multiSelect ? (
                  <button
                    className="submitAnswer"
                    type="button"
                    disabled={pendingAction !== null || (selectedAnswers[question.question] ?? []).length === 0}
                    onClick={() => handleMultiSubmit(question.question)}
                  >
                    {pendingAction === (selectedAnswers[question.question] ?? []).join(", ") ? "Answering…" : "Submit"}
                  </button>
                ) : null}
              </div>
            </div>
          ))}
          {approvalError ? <div className="cardError">{approvalError}</div> : null}
        </>
      ) : null}
      </div>

      {terminalJumpError ? <div className="cardError terminalError">{terminalJumpError}</div> : null}

      {resolved ? <div className="cardResolved">{resolved}</div> : null}

      {cls === "err" ? <div className="errmsg">{statusText(session, now)}</div> : null}
    </article>
  );
}

export default SessionCard;
