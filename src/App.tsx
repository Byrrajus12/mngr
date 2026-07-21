import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useMemo, useRef, useState } from "react";
import Filament from "./components/Filament";
import Panel from "./components/Panel";
import type { Session, SessionStatus } from "./types";

type WindowMode = "collapsed" | "peek" | "expanded";

async function setWindowMode(mode: WindowMode) {
  if (mode === "expanded") await invoke("expand_panel");
  if (mode === "peek") await invoke("peek_panel");
  if (mode === "collapsed") await invoke("collapse_panel");
}

function needsAttention(status: SessionStatus) {
  return status === "WaitingForApproval" || status === "WaitingForInput";
}

function App() {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [mode, setMode] = useState<WindowMode>("collapsed");
  const [flash, setFlash] = useState<Set<string>>(() => new Set());
  const [dismissed, setDismissed] = useState<Set<string>>(() => new Set());
  const [now, setNow] = useState(() => Date.now());

  const leaveTimer = useRef<number | undefined>(undefined);
  const modeRef = useRef<WindowMode>(mode);
  const hoveredRef = useRef(false);
  const prevStatusRef = useRef<Map<string, SessionStatus>>(new Map());
  const flashTimers = useRef<Map<string, number>>(new Map());

  useEffect(() => {
    modeRef.current = mode;
  }, [mode]);

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  async function collapse() {
    window.clearTimeout(leaveTimer.current);
    await setWindowMode("collapsed");
    setMode("collapsed");
  }

  async function peek() {
    if (modeRef.current !== "collapsed") return;
    window.clearTimeout(leaveTimer.current);
    await setWindowMode("peek");
    setMode("peek");
  }

  async function expand() {
    window.clearTimeout(leaveTimer.current);
    await setWindowMode("expanded");
    setMode("expanded");
  }

  function unpeekSoon() {
    if (modeRef.current !== "peek") return;
    window.clearTimeout(leaveTimer.current);
    leaveTimer.current = window.setTimeout(() => {
      if (modeRef.current === "peek" && !hoveredRef.current) collapse();
    }, 300);
  }

  function handlePeek() {
    hoveredRef.current = true;
    peek();
  }

  function handleUnpeek() {
    hoveredRef.current = false;
    unpeekSoon();
  }

  useEffect(() => {
    invoke<Session[]>("get_sessions")
      .then(setSessions)
      .catch((error) => console.error("get_sessions failed", error));

    const unlisten = listen<Session[]>("sessions-updated", (event) => {
      setSessions(event.payload);
    });

    return () => {
      unlisten.then((dispose) => dispose());
    };
  }, []);

  // Click-outside-closes: Rust emits window-blurred on focus loss.
  useEffect(() => {
    const unlisten = listen("window-blurred", () => {
      if (modeRef.current === "expanded") collapse();
    });
    return () => {
      unlisten.then((dispose) => dispose());
    };
  }, []);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") collapse();
    }
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, []);

  // Attention flash: when a session newly needs attention while collapsed,
  // peek and surface its label for 3s.
  useEffect(() => {
    const prev = prevStatusRef.current;
    const newlyAttention: string[] = [];

    for (const session of sessions) {
      const wasAttn = needsAttention(prev.get(session.session_id) ?? "Idle");
      if (needsAttention(session.status) && !wasAttn) {
        newlyAttention.push(session.session_id);
      }
    }

    prevStatusRef.current = new Map(sessions.map((s) => [s.session_id, s.status]));

    if (newlyAttention.length === 0 || modeRef.current !== "collapsed") return;

    invoke("peek_panel").catch((error) => console.error("peek_panel failed", error));
    setMode("peek");
    setFlash((current) => {
      const next = new Set(current);
      newlyAttention.forEach((id) => next.add(id));
      return next;
    });

    for (const id of newlyAttention) {
      const existing = flashTimers.current.get(id);
      if (existing) window.clearTimeout(existing);
      const timer = window.setTimeout(() => {
        flashTimers.current.delete(id);
        setFlash((current) => {
          const next = new Set(current);
          next.delete(id);
          return next;
        });
        if (modeRef.current === "peek" && !hoveredRef.current) collapse();
      }, 3000);
      flashTimers.current.set(id, timer);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessions]);

  const activeSessions = useMemo(
    () =>
      sessions.filter(
        (session) =>
          !dismissed.has(session.session_id) &&
          (session.status !== "Done" || now - session.last_event_at < 30 * 60 * 1000),
      ),
    [dismissed, now, sessions],
  );

  function dismissSession(id: string) {
    setDismissed((current) => {
      const next = new Set(current);
      next.add(id);
      return next;
    });
  }

  return (
    <div
      className={`appShell mode-${mode}`}
      onMouseLeave={mode === "peek" ? handleUnpeek : undefined}
    >
      <Panel
        sessions={activeSessions}
        expanded={mode === "expanded"}
        now={now}
        onClose={collapse}
        onDismiss={dismissSession}
      />
      <Filament
        sessions={activeSessions}
        expanded={mode === "expanded"}
        peeking={mode === "peek"}
        flash={flash}
        now={now}
        onExpand={expand}
        onPeek={handlePeek}
        onUnpeek={handleUnpeek}
      />
    </div>
  );
}

export default App;