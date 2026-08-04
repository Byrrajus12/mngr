import { Fragment, type ReactNode } from "react";
import { useState } from "react";
import claudeLogo from "../assets/providers/claude.svg";
import codexLogo from "../assets/providers/codex.svg";
import type { ClaudeUsageState, CodexUsageState, Session } from "../types";
import SessionCard from "./SessionCard";

type PanelProps = {
  sessions: Session[];
  expanded: boolean;
  now: number;
  claudeUsage: ClaudeUsageState | null;
  codexUsage: CodexUsageState | null;
  onClose: () => void;
  onDismiss: (id: string) => void;
};

type UsageDisplayWindow = {
  label: string;
  used_percentage: number;
  resets_at?: number | string | null;
};

type UsageProviderRow = {
  providerId: string;
  glyph: ReactNode;
  windows: UsageDisplayWindow[];
  lastUpdated?: number | string | null;
};

function timestampMs(value: number | string | null | undefined) {
  if (typeof value === "number") return value;
  if (typeof value === "string") {
    const parsed = Date.parse(value);
    return Number.isNaN(parsed) ? null : parsed;
  }
  return null;
}

function formatResetCountdown(resetsAt: number | string | null | undefined, now: number) {
  const resetMs = timestampMs(resetsAt);
  if (!resetMs) return "reset unknown";
  const remainingMs = resetMs - now;
  if (remainingMs <= 10000) return "resetting";

  const totalMinutes = Math.ceil(remainingMs / 60000);
  const days = Math.floor(totalMinutes / 1440);
  const hours = Math.floor((totalMinutes % 1440) / 60);
  const minutes = totalMinutes % 60;

  if (days > 0) return hours > 0 ? `resets in ${days}d${hours}h` : `resets in ${days}d`;
  if (hours > 0) return `resets in ${hours}h${minutes}m`;
  return `resets in ${minutes}m`;
}

function isUsageStale(lastUpdated: number | string | null | undefined, now: number) {
  const updatedMs = timestampMs(lastUpdated);
  return !!updatedMs && now - updatedMs > 15 * 60 * 1000;
}

function formatStaleness(lastUpdated: number | string | null | undefined, now: number): string | null {
  const updatedMs = timestampMs(lastUpdated);
  if (!updatedMs) return null;
  const ageMs = now - updatedMs;
  if (ageMs <= 15 * 60 * 1000) return null;

  const minutes = Math.floor(ageMs / 60000);
  if (minutes < 1) return null;
  if (minutes < 60) return `as of ${minutes}m ago`;

  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `as of ${hours}h ago`;

  return `as of ${Math.floor(hours / 24)}d ago`;
}

function formatVisibleCountdown(
  window: UsageDisplayWindow,
  now: number,
  staleness: string | null,
) {
  if (staleness) return staleness;
  return formatResetCountdown(window.resets_at, now).replace(/^resets in /, "");
}

function usageTier(percent: number) {
  if (percent >= 90) return "crit";
  if (percent >= 50) return "hot";
  return "";
}

function UsageWindowGroup({
  window,
  now,
  staleness,
}: {
  window: UsageDisplayWindow;
  now: number;
  staleness: string | null;
}) {
  const percent = Math.max(0, Math.min(100, Math.round(window.used_percentage)));
  const tier = usageTier(percent);
  const countdown = formatVisibleCountdown(window, now, staleness);

  return (
    <div className={`win ${tier} ${staleness ? "usage-stale-values" : ""}`}>
      <div className="wtop">
        <span className="wl">{window.label}</span>
        <b className="pct">{percent}%</b>
        <span className={`cd ${countdown === "resetting" ? "resetting" : ""}`}>{countdown}</span>
      </div>
      <div className="fil"><i style={{ width: `${percent}%` }} /></div>
    </div>
  );
}

function CompactUsageWindow({
  window,
  now,
  staleness,
}: {
  window: UsageDisplayWindow;
  now: number;
  staleness: string | null;
}) {
  const percent = Math.max(0, Math.min(100, Math.round(window.used_percentage)));
  const tier = usageTier(percent);
  const countdown = formatVisibleCountdown(window, now, staleness);

  return (
    <span className={`cwin ${staleness ? "usage-stale-values" : ""}`}>
      <span>{window.label}</span>
      <b className={tier}>{percent}%</b>
      <span className={`cd ${countdown === "resetting" ? "resetting" : ""}`}>{countdown}</span>
    </span>
  );
}

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
  const visibleRows = rows.filter((row) => row.windows.length > 0);

  if (visibleRows.length === 0) return null;

  return (
    <div className={`usage enter ${stale ? "stale" : ""} ${compact ? "compact" : ""}`} onClick={onToggleCompact}>
      {visibleRows.map((row) => (
        <div className="prow" key={row.providerId}>
          <span className="pglyph" aria-hidden="true">{row.glyph}</span>
          <div className="usageBody">
            <div className="usageExpanded" aria-hidden={compact}>
              <span className="pname">{row.providerId}</span>
              {row.windows.map((window, index) => {
                const staleness = formatStaleness(row.lastUpdated, now);
                return (
                  <Fragment key={window.label}>
                    {index > 0 ? <div className="vsep" /> : null}
                    <UsageWindowGroup window={window} now={now} staleness={staleness} />
                  </Fragment>
                );
              })}
            </div>
            <span className="usageCompact cline" aria-hidden={!compact}>
              {row.windows.map((window, index) => {
                const staleness = formatStaleness(row.lastUpdated, now);
                return (
                  <Fragment key={window.label}>
                    {index > 0 ? <span className="bsep">|</span> : null}
                    <CompactUsageWindow window={window} now={now} staleness={staleness} />
                  </Fragment>
                );
              })}
            </span>
          </div>
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

function Panel({ sessions, expanded, now, claudeUsage, codexUsage, onClose, onDismiss }: PanelProps) {
  const [usageCompact, setUsageCompact] = useState(true);
  const usageRows: UsageProviderRow[] = [
    {
      providerId: "Claude",
      glyph: <img src={claudeLogo} alt="" width="14" height="14" style={{ display: "block" }} />,
      windows: [
        ...(claudeUsage?.five_hour ? [{ ...claudeUsage.five_hour, label: "5h" }] : []),
        ...(claudeUsage?.seven_day ? [{ ...claudeUsage.seven_day, label: "7d" }] : []),
      ],
      lastUpdated: claudeUsage?.last_updated,
    },
    {
      providerId: "OpenAI",
      glyph: <img src={codexLogo} alt="" width="14" height="14" style={{ display: "block" }} />,
      windows: (codexUsage?.windows ?? []).map((window) => ({
        label: window.window_label,
        used_percentage: window.used_percentage,
        resets_at: window.resets_at,
      })),
      lastUpdated: codexUsage?.last_updated,
    },
  ];
  const stale =
    isUsageStale(claudeUsage?.last_updated, now) ||
    ((codexUsage?.windows.length ?? 0) > 0 && isUsageStale(codexUsage?.last_updated, now));

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
        stale={stale}
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
