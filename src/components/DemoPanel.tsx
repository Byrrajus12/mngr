import { useState } from "react";

type DemoPanelProps = {
  agentCount: number;
  onAddAgent: () => void;
  onRemoveAgent: () => void;
  onTriggerPermission: () => void;
  onTriggerQuestion: () => void;
  onCompleteTask: () => void;
};

function DemoPanel({
  agentCount,
  onAddAgent,
  onRemoveAgent,
  onTriggerPermission,
  onTriggerQuestion,
  onCompleteTask,
}: DemoPanelProps) {
  const [visible, setVisible] = useState(true);

  if (!visible) {
    return (
      <div className="demoPanel collapsed">
        <button className="dbtn mute off" type="button" onClick={() => setVisible(true)}>
          <span className="sq" />
          Show demo
        </button>
      </div>
    );
  }

  return (
    <div className="demoPanel">
      <div className="dh">
        <span className="lbl">DEMO</span>
        <span className="cnt">{agentCount} agents</span>
      </div>
      <div className="grid">
        <button className="dbtn" type="button" onClick={onAddAgent}>Add Agent</button>
        <button className="dbtn" type="button" onClick={onRemoveAgent}>Remove Agent</button>
        <button className="dbtn wide" type="button" onClick={onTriggerPermission}>Trigger Permission</button>
        <button className="dbtn wide" type="button" onClick={onTriggerQuestion}>Trigger Question</button>
        <button className="dbtn wide" type="button" onClick={onCompleteTask}>Complete Task</button>
        <button className="dbtn mute" type="button" onClick={() => setVisible(false)}>
          <span className="sq" />
          Hide demo
        </button>
      </div>
    </div>
  );
}

export default DemoPanel;