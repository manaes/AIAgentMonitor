import {
  listenSnapshot,
  listTriggerRules,
  addTriggerRule,
  removeTriggerRule,
  toggleTriggerRule,
  fireTriggerNow,
  bleStatus,
  bleSetEnabled,
  bleBeginPairing,
  bleUnpair,
  bleUnpairAll,
  listenBleStatus,
  type Snapshot,
  type TriggerRule,
  type BleStatus,
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
    this.#bleUnlisten?.();
    this.#bleUnlisten = null;
    this.#bleInitialized = false;
  }

  async loadTriggers() {
    this.triggers = await listTriggerRules();
  }

  async addTrigger(
    agent: "claude" | "codex" | "antigravity",
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

  ble = $state<BleStatus | null>(null);
  // setBleEnabled 자체 실패(IPC 등)만 담는다 — 권한 거부 같은 백엔드 오류는 ble.last_error 로 들어온다
  bleActionError = $state<string | null>(null);
  // initBle 은 멱등해야 한다 — DevicePanel 은 탭 전환마다 remount 되어 onMount 에서 매번 호출된다.
  // #bleInitialized 를 첫 await 이전에 세워야, 두 번째 호출이 첫 호출의 await 도중에 들어와도
  // 가드를 통과하지 못한다. #bleUnlisten 은 오직 dispose 용 핸들로만 쓴다.
  #bleInitialized = false;
  #bleUnlisten: (() => void) | null = null;
  // ble_status 재조회가 겹쳐 도착할 때 오래된 응답이 최신 응답을 덮어쓰지 않도록 순번을 매긴다
  #bleReqSeq = 0;

  async initBle() {
    if (this.#bleInitialized) return;
    this.#bleInitialized = true;
    const seq = ++this.#bleReqSeq;
    const status = await bleStatus();
    if (seq === this.#bleReqSeq) this.ble = status;
    this.#bleUnlisten = await listenBleStatus(async () => {
      const seq = ++this.#bleReqSeq;
      const status = await bleStatus();
      if (seq !== this.#bleReqSeq) return;
      this.ble = status;
      // 백엔드가 새 상태를 말한 순간부터는 그쪽이 최신 진실이다. 여기서 지우지 않으면
      // 실패한 켜기가 남긴 bleActionError 가 (DevicePanel 이 그것을 우선 표시하므로)
      // 이후 도착하는 백엔드 오류를 계속 가린다.
      this.bleActionError = null;
    });
  }

  async setBleEnabled(on: boolean) {
    try {
      await bleSetEnabled(on);
      const seq = ++this.#bleReqSeq;
      const status = await bleStatus();
      if (seq === this.#bleReqSeq) this.ble = status;
      this.bleActionError = null;
    } catch (e) {
      this.bleActionError = `BLE 설정을 변경하지 못했습니다: ${e}`;
    }
  }

  async beginPairing() {
    try {
      await bleBeginPairing();
      const seq = ++this.#bleReqSeq;
      const status = await bleStatus();
      if (seq === this.#bleReqSeq) this.ble = status;
      this.bleActionError = null;
    } catch (e) {
      this.bleActionError = `페어링을 시작하지 못했습니다: ${e}`;
    }
  }

  async unpair(peerId: string) {
    try {
      await bleUnpair(peerId);
      const seq = ++this.#bleReqSeq;
      const status = await bleStatus();
      if (seq === this.#bleReqSeq) this.ble = status;
      this.bleActionError = null;
    } catch (e) {
      this.bleActionError = `기기를 해제하지 못했습니다: ${e}`;
    }
  }

  async unpairAll() {
    try {
      await bleUnpairAll();
      const seq = ++this.#bleReqSeq;
      const status = await bleStatus();
      if (seq === this.#bleReqSeq) this.ble = status;
      this.bleActionError = null;
    } catch (e) {
      this.bleActionError = `전체 해제에 실패했습니다: ${e}`;
    }
  }
}

export const store = new SnapshotStore();
