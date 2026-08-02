import type { ReactNode } from "react";
import claudeLogo from "../assets/providers/claude.svg";
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
  if (remainingMs <= 10000) return "resetting";

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

function UsageMeter({ label, window, now }: { label: string; window: ClaudeUsageWindow; now: number }) {
  const percent = Math.max(0, Math.min(100, Math.round(window.used_percentage)));
  const hot = percent >= 50;

  return (
    <div className={`meter ${hot ? "hot" : ""}`}>
      <span className="w">{label}</span>
      <div className="bar"><i style={{ width: `${percent}%` }} /></div>
      <span className="v">
        <b>{percent}%</b>{" \u00b7 "}{formatResetCountdown(window.resets_at, now)}
      </span>
    </div>
  );
}

type UsageProviderRow = {
  providerId: string;
  glyph: ReactNode;
  fiveHour?: ClaudeUsageWindow | null;
  sevenDay?: ClaudeUsageWindow | null;
};

function UsageSection({ rows, now, stale }: { rows: UsageProviderRow[]; now: number; stale: boolean }) {
  const visibleRows = rows.filter((row) => row.fiveHour || row.sevenDay);

  if (visibleRows.length === 0) return null;

  return (
    <div className={`usage ${stale ? "stale" : ""}`}>
      {visibleRows.map((row) => (
        <div className="prow" key={row.providerId}>
          <div className="providerHead">
            <span className="pglyph" aria-hidden="true">{row.glyph}</span>
            <span className="pname">{row.providerId}</span>
          </div>
          <div className="providerMeters">
            {row.fiveHour ? <UsageMeter label="5h" window={row.fiveHour} now={now} /> : null}
            {row.sevenDay ? <UsageMeter label="7d" window={row.sevenDay} now={now} /> : null}
          </div>
        </div>
      ))}
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
  const usageRows: UsageProviderRow[] = [
    {
      providerId: "claude",
      glyph: <img src={claudeLogo} alt="" width="14" height="14" style={{ display: "block" }} />,
      fiveHour: claudeUsage?.five_hour,
      sevenDay: claudeUsage?.seven_day,
    },
  ];

  return (
    <aside className={`panel ${expanded ? "open" : ""}`} aria-hidden={!expanded}>
      <header className="phead">
        <div>
          <div className="wm">mngr</div>
          <div className="agg">{aggregateStatus(sessions)}</div>
        </div>
        <button className="pclose" type="button" onClick={onClose}>esc</button>
      </header>
      <UsageSection rows={usageRows} now={now} stale={isUsageStale(claudeUsage?.last_updated, now)} />

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


