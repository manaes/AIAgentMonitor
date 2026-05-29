import {
  listenSnapshot,
  listTriggerRules,
  addTriggerRule,
  removeTriggerRule,
  toggleTriggerRule,
  fireTriggerNow,
  type Snapshot,
  type TriggerRule,
} from "./tauri";

class SnapshotStore {
  snap = $state<Snapshot | null>(null);
  lastReceived = $state<number>(0);
  staleSeconds = $state<number>(0);

  // Anchor Trigger 룰 목록
  triggers = $state<TriggerRule[]>([]);

  async init() {
    const unlisten = await listenSnapshot((s) => {
      this.snap = s;
      this.lastReceived = Date.now();
    });
    setInterval(() => {
      this.staleSeconds = Math.floor((Date.now() - this.lastReceived) / 1000);
    }, 1000);
    return unlisten;
  }

  async loadTriggers() {
    this.triggers = await listTriggerRules();
  }

  async addTrigger(
    agent: "claude" | "codex",
    hour: number,
    minute: number,
    working_dir: string,
    prompt: string
  ) {
    const rule = await addTriggerRule(agent, hour, minute, working_dir, prompt);
    this.triggers = [...this.triggers, rule];
  }

  async removeTrigger(id: string) {
    await removeTriggerRule(id);
    this.triggers = this.triggers.filter((r) => r.id !== id);
  }

  async toggleTrigger(id: string) {
    const updated = await toggleTriggerRule(id);
    this.triggers = this.triggers.map((r) => (r.id === id ? updated : r));
  }

  async fireNow(id: string) {
    await fireTriggerNow(id);
  }
}

export const store = new SnapshotStore();
