import type { CSSProperties } from "react";
import type { Session } from "../types";

type FilamentProps = {
  sessions: Session[];
  expanded: boolean;
  peeking: boolean;
  flash: Set<string>;
  now: number;
  onExpand: () => void;
  onPeek: () => void;
  onUnpeek: () => void;
};

const GAP = 3;

function statusKind(session: Session) {
  if (session.status === "WaitingForApproval") return "approval";
  if (session.status === "WaitingForInput") return "question";
  if (session.status === "Done" || session.status === "Idle") return "done";
  return "working";
}

function segmentWidth(session: Session) {
  const kind = statusKind(session);
  if (kind === "approval" || kind === "question") return 4;
  if (kind === "done") return 3;
  return 2;
}

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
      <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
        <path d="M5 3L1.5 7 5 11M9 3l3.5 4L9 11" stroke="#7fd0c0" strokeWidth="1.3" fill="none" strokeLinecap="round" strokeLinejoin="round" />
      </svg>
    );
  }

  return (
    <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
      <path d="M7 1v12M1 7h12M2.8 2.8l8.4 8.4M11.2 2.8l-8.4 8.4" stroke="#d9a4f5" strokeWidth="1.15" strokeLinecap="round" />
    </svg>
  );
}

function phaseFor(sessionId: string) {
  let hash = 0;
  for (const char of sessionId) hash = (hash * 31 + char.charCodeAt(0)) % 400;
  return `${(hash / 100).toFixed(2)}s`;
}

function Filament({ sessions, expanded, peeking, flash, now, onExpand, onPeek, onUnpeek }: FilamentProps) {
  const active = sessions;
  const count = active.length;

  return (
    <button
      className={`filament ${expanded ? "expanded" : ""} ${peeking ? "peeking" : ""}`}
      type="button"
      aria-label="Expand mngr sessions"
      onClick={onExpand}
      onMouseEnter={onPeek}
      onMouseLeave={onUnpeek}
    >
      <span className={`idleLine ${count === 0 ? "show" : ""}`} />
      {active.map((session, index) => {
        const height = count > 0 ? `calc((100% - ${(count - 1) * GAP}px) / ${count})` : "0px";
        const top = count > 0 ? `calc(${index} * (${height} + ${GAP}px))` : "0px";
        const kind = statusKind(session);

        return (
          <span
            className={`seg ${kind}`}
            data-status={kind}
            key={session.session_id}
            style={
              {
                top,
                height,
                width: `${segmentWidth(session)}px`,
                animationDelay: kind === "working" ? `-${phaseFor(session.session_id)}` : undefined,
              } as CSSProperties
            }
          >
            {!expanded ? (
              <span
                className={`seglabel ${flash.has(session.session_id) ? "attnchip" : ""}`}
                onClick={onExpand}
              >
                {glyph(session)}
                <span>{session.project_name}</span>
                <span className="el">
                  {elapsed(session.started_at, now, session.status === "Done" ? session.last_event_at : undefined)}
                </span>
              </span>
            ) : null}
          </span>
        );
      })}
    </button>
  );
}

export default Filament;