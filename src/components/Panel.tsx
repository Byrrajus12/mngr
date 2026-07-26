import type { ClaudeUsageState, ClaudeUsageWindow, Session } from "../types";
import SessionCard from "./SessionCard";

type PanelProps = {
  sessions: Session[];
  expanded: boolean;
  now: number;
  claudeUsage: ClaudeUsageState | null;
  onClose: () => void;
  onDismiss: (id: string) => void;
};

function formatResetCountdown(resetsAt: number | null | undefined, now: number) {
  if (!resetsAt) return "reset unknown";
  const remainingMs = resetsAt - now;
  if (remainingMs <= 0) return "reset due";

  const totalMinutes = Math.ceil(remainingMs / 60000);
  const days = Math.floor(totalMinutes / 1440);
  const hours = Math.floor((totalMinutes % 1440) / 60);
  const minutes = totalMinutes % 60;

  if (days > 0) return hours > 0 ? `resets in ${days}d${hours}h` : `resets in ${days}d`;
  if (hours > 0) return `resets in ${hours}h${minutes}m`;
  return `resets in ${minutes}m`;
}

function isUsageStale(lastUpdated: number | null | undefined, now: number) {
  return !!lastUpdated && now - lastUpdated > 15 * 60 * 1000;
}

function UsageBar({ label, window, now }: { label: string; window?: ClaudeUsageWindow | null; now: number }) {
  if (!window) {
    return (
      <div className="usageBar empty">
        <div className="txt">
          <span>{label} --</span>
          <span>waiting</span>
        </div>
        <div className="track"><div className="fill" style={{ width: "0%" }} /></div>
      </div>
    );
  }

  const percent = Math.max(0, Math.min(100, Math.round(window.used_percentage)));

  return (
    <div className="usageBar">
      <div className="txt">
        <span>{label} {percent}%</span>
        <span>{formatResetCountdown(window.resets_at, now)}</span>
      </div>
      <div className="track"><div className="fill" style={{ width: `${percent}%` }} /></div>
    </div>
  );
}

function aggregateStatus(sessions: Session[]) {
  const working = sessions.filter((session) => session.status === "Working").length;
  const needs = sessions.filter(
    (session) => session.status === "WaitingForApproval" || session.status === "WaitingForInput",
  ).length;
  const done = sessions.filter((session) => session.status === "Done" || session.status === "Idle").length;

  const parts: string[] = [];
  if (working) parts.push(`${working} working`);
  if (needs) parts.push(`${needs} needs you`);
  if (done) parts.push(`${done} done`);
  return parts.length ? parts.join(" - ") : "idle";
}

function Panel({ sessions, expanded, now, claudeUsage, onClose, onDismiss }: PanelProps) {
  return (
    <aside className={`panel ${expanded ? "open" : ""}`} aria-hidden={!expanded}>
      <header className="phead">
        <div>
          <div className="wm">mngr</div>
          <div className="agg">{aggregateStatus(sessions)}</div>
          <div className={`usage ${isUsageStale(claudeUsage?.last_updated, now) ? "stale" : ""}`}>
            <UsageBar label="5h" window={claudeUsage?.five_hour} now={now} />
            <UsageBar label="7d" window={claudeUsage?.seven_day} now={now} />
          </div>
        </div>
        <button className="pclose" type="button" onClick={onClose}>esc</button>
      </header>

      <section className="cards" aria-label="Agent sessions">
        {sessions.length === 0 ? (
          <div className="emptyPanel">No active agents.</div>
        ) : (
          sessions.map((session, index) => (
            <SessionCard
              session={session}
              index={index}
              now={now}
              key={session.session_id}
              onDismiss={() => onDismiss(session.session_id)}
            />
          ))
        )}
      </section>
    </aside>
  );
}

export default Panel;
