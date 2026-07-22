import { invoke } from "@tauri-apps/api/core";
import { useState } from "react";
import type { Session } from "../types";

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
    return (
      <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="#7fd0c0" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        <path d="M6 4 L2.5 8 L6 12" />
        <path d="M10 4 L13.5 8 L10 12" />
      </svg>
    );
  }

  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="#d9a4f5" strokeWidth="1.5" strokeLinecap="round" aria-hidden="true">
      <path d="M8 2 V14 M2 8 H14 M3.8 3.8 L12.2 12.2 M12.2 3.8 L3.8 12.2" />
    </svg>
  );
}

function statusClass(session: Session) {
  if (session.status === "WaitingForApproval") return "attention-req";
  if (session.status === "WaitingForInput") return "attention-q";
  if (session.status === "Done" || session.status === "Idle") return "done";
  return "working";
}

function shouldFreezeElapsed(session: Session) {
  return session.status === "Idle" || session.status === "Done";
}

function statusText(session: Session, now: number) {
  if (session.status === "WaitingForApproval") return "Waiting for permission";
  if (session.status === "WaitingForInput") return "Waiting for your answer";
  if (session.status === "Done") return `Finished - ${elapsed(session.started_at, now, session.last_event_at)}`;
  if (session.status === "Idle") return `Idle - ${elapsed(session.started_at, now, session.last_event_at)}`;
  if (session.current_tool) return `Using ${session.current_tool}`;
  return "Working";
}

function commandText(session: Session) {
  const pending = session.pending_approval;
  if (!pending) return "command";
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

function SessionCard({ session, index, now, onDismiss }: SessionCardProps) {
  const cls = statusClass(session);
  const isDone = cls === "done";
  const isDemo = session.session_id.startsWith("demo-");
  const [resolved, setResolved] = useState<string | null>(null);
  const [reason, setReason] = useState("");
  const [pendingAction, setPendingAction] = useState<string | null>(null);
  const [approvalError, setApprovalError] = useState<string | null>(null);

  const showApproval = session.status === "WaitingForApproval" && !resolved;
  const showQuestion = session.status === "WaitingForInput" && !resolved;

  async function resolveRealApproval(decision: "allow" | "deny", always: boolean) {
    const requestId = session.pending_approval?.request_id;
    if (!requestId) {
      setApprovalError("Missing approval request id");
      return;
    }

    setPendingAction(always ? "always" : decision);
    setApprovalError(null);
    try {
      await invoke("resolve_approval", {
        requestId,
        decision,
        reason: decision === "deny" ? reason.trim() || null : null,
        always,
      });
    } catch (error) {
      setApprovalError(error instanceof Error ? error.message : String(error));
    } finally {
      setPendingAction(null);
    }
  }

  function handleAllow(always: boolean) {
    if (isDemo) {
      console.log("mngr allow clicked", session.session_id);
      setResolved("Allowed - resuming");
      return;
    }

    resolveRealApproval("allow", always);
  }

  function handleDeny() {
    if (isDemo) {
      console.log("mngr deny clicked", session.session_id);
      setResolved("Denied - resuming");
      return;
    }

    resolveRealApproval("deny", false);
  }

  return (
    <article
      className={`card ${cls}`}
      style={{ animationDelay: `${index * 40 + 40}ms` }}
      onClick={isDone ? onDismiss : undefined}
    >
      <div className="top">
        <span className="glyph">
          {isDone ? (
            <svg className="checkwrap" viewBox="0 0 22 22" fill="none" stroke="var(--emerald)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M5 11.5 L9.5 16 L17 6.5" />
            </svg>
          ) : (
            glyph(session)
          )}
        </span>
        <div>
          <div className="proj">{session.project_name}</div>
          <div className="act">{statusText(session, now)}</div>
        </div>
        <div className="meta">
          <span className={`sdot ${cls}`} />
          <br />
          <span className="el">{elapsed(session.started_at, now, shouldFreezeElapsed(session) ? session.last_event_at : undefined)}</span>
        </div>
      </div>

      {!isDone ? (
        <button
          className="jump"
          type="button"
          aria-label="Jump to terminal"
          onClick={(event) => {
            event.stopPropagation();
            console.log("jump", session.session_id);
          }}
        >
          <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><path d="M3 9L9 3M4.5 3H9v4.5" stroke="currentColor" strokeWidth="1.3" fill="none" strokeLinecap="round" strokeLinejoin="round" /></svg>
        </button>
      ) : null}

      {showApproval ? (
        <>
          <div className="cmdchip">{commandText(session)}</div>
          {!isDemo ? (
            <input
              className="reasonInput"
              type="text"
              value={reason}
              onChange={(event) => setReason(event.target.value)}
              placeholder="Optional deny reason"
              aria-label="Optional deny reason"
            />
          ) : null}
          <div className="btns">
            <button
              className="allow"
              type="button"
              disabled={pendingAction !== null}
              onClick={() => handleAllow(false)}
            >
              {pendingAction === "allow" ? "Allowing" : "Allow"}
            </button>
            {!isDemo ? (
              <button
                className="alwaysAllow"
                type="button"
                disabled={pendingAction !== null}
                onClick={() => handleAllow(true)}
              >
                {pendingAction === "always" ? "Saving" : "Always allow"}
              </button>
            ) : null}
            <button
              className="deny"
              type="button"
              disabled={pendingAction !== null}
              onClick={handleDeny}
            >
              {pendingAction === "deny" ? "Denying" : "Deny"}
            </button>
          </div>
          {approvalError ? <div className="cardError">{approvalError}</div> : null}
        </>
      ) : null}

      {showQuestion ? (
        <>
          <div className="qtext">Claude Code has a question before continuing.</div>
          <div className="pills">
            {["Continue", "Explain first", "Stop"].map((option) => (
              <button
                className="pill"
                type="button"
                key={option}
                onClick={() => {
                  console.log("question option", option, session.session_id);
                  setResolved(`-> ${option}`);
                }}
              >
                {option}
              </button>
            ))}
          </div>
        </>
      ) : null}

      {resolved ? <div className="cardResolved">{resolved}</div> : null}

      {isDone ? <div className="dismiss">click to dismiss</div> : null}
    </article>
  );
}

export default SessionCard;
