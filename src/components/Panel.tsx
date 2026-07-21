import type { Session } from "../types";
import SessionCard from "./SessionCard";

type PanelProps = {
  sessions: Session[];
  expanded: boolean;
  now: number;
  onClose: () => void;
  onDismiss: (id: string) => void;
};

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

function Panel({ sessions, expanded, now, onClose, onDismiss }: PanelProps) {
  return (
    <aside className={`panel ${expanded ? "open" : ""}`} aria-hidden={!expanded}>
      <header className="phead">
        <div>
          <div className="wm">mngr</div>
          <div className="agg">{aggregateStatus(sessions)}</div>
          <div className="usage">
            <div className="txt">
              <span>Opus 62%</span>
              <span>resets 4:00 PM</span>
            </div>
            <div className="track"><div className="fill" /></div>
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