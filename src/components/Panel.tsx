import type { ReactNode } from "react";
import { useState } from "react";
import claudeLogo from "../assets/providers/claude.svg";
import codexLogo from "../assets/providers/codex.svg";
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

function formatVisibleCountdown(window: ClaudeUsageWindow, now: number) {
  return formatResetCountdown(window.resets_at, now).replace(/^resets in /, "");
}

function usageTier(percent: number) {
  if (percent >= 90) return "crit";
  if (percent >= 50) return "hot";
  return "";
}

function UsageWindowGroup({ label, window, now }: { label: string; window: ClaudeUsageWindow; now: number }) {
  const percent = Math.max(0, Math.min(100, Math.round(window.used_percentage)));
  const tier = usageTier(percent);
  const countdown = formatVisibleCountdown(window, now);

  return (
    <div className={`win ${tier}`}>
      <div className="wtop">
        <span className="wl">{label}</span>
        <b className="pct">{percent}%</b>
        <span className={`cd ${countdown === "resetting" ? "resetting" : ""}`}>{countdown}</span>
      </div>
      <div className="fil"><i style={{ width: `${percent}%` }} /></div>
    </div>
  );
}

function CompactUsageWindow({ label, window, now }: { label: string; window: ClaudeUsageWindow; now: number }) {
  const percent = Math.max(0, Math.min(100, Math.round(window.used_percentage)));
  const tier = usageTier(percent);
  const countdown = formatVisibleCountdown(window, now);

  return (
    <span className="cwin">
      <span>{label}</span>
      <b className={tier}>{percent}%</b>
      <span className={`cd ${countdown === "resetting" ? "resetting" : ""}`}>{countdown}</span>
    </span>
  );
}

type UsageProviderRow = {
  providerId: string;
  glyph: ReactNode;
  fiveHour?: ClaudeUsageWindow | null;
  sevenDay?: ClaudeUsageWindow | null;
};

function UsageSection({
  rows,
  now,
  stale,
  compact,
  onToggleCompact,
}: {
  rows: UsageProviderRow[];
  now: number;
  stale: boolean;
  compact: boolean;
  onToggleCompact: () => void;
}) {
  const visibleRows = rows.filter((row) => row.fiveHour || row.sevenDay);

  if (visibleRows.length === 0) return null;

  return (
    <div className={`usage enter ${stale ? "stale" : ""} ${compact ? "compact" : ""}`} onClick={onToggleCompact}>
      {visibleRows.map((row) => (
        <div className="prow" key={row.providerId}>
          <div className="pid">
            <span className="pglyph" aria-hidden="true">{row.glyph}</span>
            {compact ? null : <span className="pname">{row.providerId}</span>}
          </div>
          {compact ? (
            <span className="cline">
              {row.fiveHour ? <CompactUsageWindow label="5h" window={row.fiveHour} now={now} /> : null}
              {row.fiveHour && row.sevenDay ? <span className="bsep">|</span> : null}
              {row.sevenDay ? <CompactUsageWindow label="7d" window={row.sevenDay} now={now} /> : null}
            </span>
          ) : (
            <>
              {row.fiveHour ? <UsageWindowGroup label="5h" window={row.fiveHour} now={now} /> : null}
              {row.fiveHour && row.sevenDay ? <div className="vsep" /> : null}
              {row.sevenDay ? <UsageWindowGroup label="7d" window={row.sevenDay} now={now} /> : null}
            </>
          )}
        </div>
      ))}
      <span className="utoggle" aria-hidden="true"><svg width="10" height="10" viewBox="0 0 10 10" fill="none"><path d="M2 4l3 3 3-3" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" /></svg></span>
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
  const [usageCompact, setUsageCompact] = useState(true);
  const usageRows: UsageProviderRow[] = [
    {
      providerId: "Claude",
      glyph: <img src={claudeLogo} alt="" width="14" height="14" style={{ display: "block" }} />,
      fiveHour: claudeUsage?.five_hour,
      sevenDay: claudeUsage?.seven_day,
    },
    {
      providerId: "OpenAI",
      glyph: <img src={codexLogo} alt="" width="14" height="14" style={{ display: "block" }} />,
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
      <UsageSection
        rows={usageRows}
        now={now}
        stale={isUsageStale(claudeUsage?.last_updated, now)}
        compact={usageCompact}
        onToggleCompact={() => setUsageCompact((current) => !current)}
      />

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
