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

  // teardown용 핸들 — init은 멱등(중복 호출 무시)이라 리스너/타이머가 쌓이지 않는다
  #initialized = false;
  #unlisten: (() => void) | null = null;
  #staleTimer: ReturnType<typeof setInterval> | null = null;

  async init() {
    if (this.#initialized) return;
    this.#initialized = true;
    this.#unlisten = await listenSnapshot((s) => {
      this.snap = s;
      this.lastReceived = Date.now();
    });
    this.#staleTimer = setInterval(() => {
      this.staleSeconds = Math.floor((Date.now() - this.lastReceived) / 1000);
    }, 1000);
  }

  dispose() {
    this.#unlisten?.();
    this.#unlisten = null;
    if (this.#staleTimer !== null) {
      clearInterval(this.#staleTimer);
      this.#staleTimer = null;
    }
    this.#initialized = false;
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
