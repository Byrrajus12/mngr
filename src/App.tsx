import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useMemo, useRef, useState } from "react";
import DemoPanel from "./components/DemoPanel";
import Filament from "./components/Filament";
import Panel from "./components/Panel";
import type { ClaudeUsageState, Session, SessionStatus } from "./types";

type WindowMode = "collapsed" | "peek" | "expanded";
type PeekReason = "hover" | "attention" | null;

const DEMO_PROJECTS = ["mngr", "api-server", "web-ui", "dotfiles", "infra"];

async function setWindowMode(mode: WindowMode) {
  if (mode === "expanded") await invoke("expand_panel");
}

function needsAttention(status: SessionStatus) {
  return status === "WaitingForApproval" || status === "WaitingForInput";
}

function isActiveStatus(status: SessionStatus) {
  return status === "Working" || status === "WaitingForApproval" || status === "WaitingForInput";
}

function App() {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [demoSessions, setDemoSessions] = useState<Session[]>([]);
  const [mode, setMode] = useState<WindowMode>("collapsed");
  const [peekReason, setPeekReason] = useState<PeekReason>(null);
  const [flash, setFlash] = useState<Set<string>>(() => new Set());
  const [dismissed, setDismissed] = useState<Set<string>>(() => new Set());
  const [now, setNow] = useState(() => Date.now());
  const [claudeUsage, setClaudeUsage] = useState<ClaudeUsageState | null>(null);

  const leaveTimer = useRef<number | undefined>(undefined);
  const demoId = useRef(1);
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
    setPeekReason(null);
    setMode("collapsed");
  }

  async function peek() {
    if (modeRef.current !== "collapsed") return;
    window.clearTimeout(leaveTimer.current);
    await setWindowMode("peek");
    requestAnimationFrame(() => {
      setMode("peek");
    });
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
    setPeekReason("hover");
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

  useEffect(() => {
    let alive = true;

    function refreshUsage() {
      invoke<ClaudeUsageState>("get_claude_usage")
        .then((usage) => {
          if (alive) setClaudeUsage(usage);
        })
        .catch((error) => console.error("get_claude_usage failed", error));
    }

    refreshUsage();
    const timer = window.setInterval(refreshUsage, 5000);
    return () => {
      alive = false;
      window.clearInterval(timer);
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

  // Cursor stream from the Rust watcher drives click-through: the window is
  // click-through everywhere except over interactive UI, and also drives
  // un-peek now that mouseleave no longer fires reliably once ignoring
  // cursor events.
  const ignoringRef = useRef(true);
  const unpeekRef = useRef(handleUnpeek);
  unpeekRef.current = handleUnpeek;

  useEffect(() => {
    const unlisten = listen<{ x: number; y: number }>("cursor-pos", (event) => {
      const { x, y } = event.payload;
      const el = document.elementFromPoint(x, y);
      const interactive = !!el?.closest(".filament, .panel, .demoPanel");
      const shouldIgnore = !interactive;

      if (ignoringRef.current !== shouldIgnore) {
        ignoringRef.current = shouldIgnore;
        getCurrentWindow().setIgnoreCursorEvents(shouldIgnore);
      }

      if (modeRef.current === "peek" && !interactive) {
        unpeekRef.current();
      }
    });
    return () => {
      unlisten.then((dispose) => dispose());
    };
  }, []);

  const allSessions = useMemo(() => [...sessions, ...demoSessions], [demoSessions, sessions]);

  useEffect(() => {
    const sessionById = new Map(allSessions.map((session) => [session.session_id, session]));
    const liveIds = new Set(sessionById.keys());

    setDismissed((current) => {
      let next: Set<string> | null = null;
      for (const id of current) {
        const session = sessionById.get(id);
        if (!session || isActiveStatus(session.status)) {
          next ??= new Set(current);
          next.delete(id);
        }
      }
      return next ?? current;
    });

    setFlash((current) => {
      let next: Set<string> | null = null;
      for (const id of current) {
        if (!liveIds.has(id)) {
          next ??= new Set(current);
          next.delete(id);
        }
      }
      return next ?? current;
    });

    for (const id of Array.from(flashTimers.current.keys())) {
      if (!liveIds.has(id)) {
        const timer = flashTimers.current.get(id);
        if (timer) window.clearTimeout(timer);
        flashTimers.current.delete(id);
      }
    }

    for (const id of Array.from(prevStatusRef.current.keys())) {
      if (!liveIds.has(id)) {
        prevStatusRef.current.delete(id);
      }
    }
  }, [allSessions]);

  // Attention flash: when a session newly needs attention while collapsed,
  // peek and surface its label for 3s.
  useEffect(() => {
    const prev = prevStatusRef.current;
    const newlyAttention: string[] = [];
    for (const session of allSessions) {
      const wasAttn = needsAttention(prev.get(session.session_id) ?? "Idle");
      if (needsAttention(session.status) && !wasAttn) {
        newlyAttention.push(session.session_id);
      }
    }

    prevStatusRef.current = new Map(allSessions.map((s) => [s.session_id, s.status]));

    if (newlyAttention.length === 0 || modeRef.current !== "collapsed") return;

    setPeekReason("attention");
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
  }, [allSessions]);

  const activeSessions = useMemo(
    () =>
      allSessions.filter(
        (session) =>
          (!dismissed.has(session.session_id) || isActiveStatus(session.status)) &&
          (session.status !== "Done" || now - session.last_event_at < 30 * 60 * 1000),
      ),
    [allSessions, dismissed, now],
  );

  function dismissSession(id: string) {
    setDismissed((current) => {
      const next = new Set(current);
      next.add(id);
      return next;
    });
  }

  function addDemoSession() {
    const id = demoId.current++;
    const projectName = DEMO_PROJECTS[(id - 1) % DEMO_PROJECTS.length];
    const timestamp = Date.now();
    setDemoSessions((current) => [
      ...current,
      {
        session_id: `demo-${id}`,
        started_at: timestamp,
        last_event_at: timestamp,
        status: "Working",
        project_name: projectName,
        project_path: `C:\\demo\\${projectName}`,
        agent_type: id % 2 === 0 ? "codex" : "claude-code",
        current_tool: null,
        permission_mode: null,
        pending_approval: null,
      },
    ]);
  }

  function removeDemoSession() {
    setDemoSessions((current) => current.slice(0, -1));
  }

  function toggleExpand() {
    if (modeRef.current === "expanded") {
      collapse();
    } else {
      expand();
    }
  }

  function updateRandomWorkingDemoSession(update: (session: Session) => Session) {
    setDemoSessions((current) => {
      const working = current
        .map((session, index) => ({ session, index }))
        .filter(({ session }) => session.status === "Working");
      if (working.length === 0) return current;

      const { index } = working[Math.floor(Math.random() * working.length)];
      return current.map((session, sessionIndex) => (sessionIndex === index ? update(session) : session));
    });
  }

  function triggerDemoPermission() {
    updateRandomWorkingDemoSession((session) => ({
      ...session,
      status: "WaitingForApproval",
      last_event_at: Date.now(),
      current_tool: "Bash",
      permission_mode: "default",
      pending_approval: {
        kind: "permission",
        request_id: `demo-${session.session_id}-approval`,
        tool_name: "Bash",
        tool_input: { command: "rm -rf node_modules && npm ci" },
        permission_mode: "default",
        permission_suggestions: [
          {
            type: "addDirectories",
            directories: [session.project_path],
            destination: "session",
          },
          {
            type: "setMode",
            mode: "acceptEdits",
            destination: "session",
          },
        ],
      },
    }));
  }

  function triggerDemoQuestion() {
    updateRandomWorkingDemoSession((session) => ({
      ...session,
      status: "WaitingForInput",
      last_event_at: Date.now(),
      current_tool: null,
      permission_mode: null,
      pending_approval: null,
    }));
  }

  function completeDemoTask() {
    updateRandomWorkingDemoSession((session) => ({
      ...session,
      status: "Done",
      last_event_at: Date.now(),
      current_tool: null,
      permission_mode: null,
      pending_approval: null,
    }));
  }

  return (
    <div className={`appShell mode-${mode}`}>
      <Panel
        sessions={activeSessions}
        expanded={mode === "expanded"}
        now={now}
        claudeUsage={claudeUsage}
        onClose={collapse}
        onDismiss={dismissSession}
      />
      <Filament
        sessions={activeSessions}
        expanded={mode === "expanded"}
        peekReason={mode === "peek" ? peekReason : null}
        flash={flash}
        now={now}
        onExpand={expand}
        onPeek={handlePeek}
        onUnpeek={handleUnpeek}
      />
      {import.meta.env.DEV ? (
        <DemoPanel
          agentCount={demoSessions.length}
          onAddAgent={addDemoSession}
          onRemoveAgent={removeDemoSession}
          onTriggerPermission={triggerDemoPermission}
          onTriggerQuestion={triggerDemoQuestion}
          onCompleteTask={completeDemoTask}
          onToggleExpand={toggleExpand}
        />
      ) : null}
    </div>
  );
}

export default App;