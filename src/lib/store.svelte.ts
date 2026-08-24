import {
  listenSnapshot,
  bleStatus,
  bleSetEnabled,
  bleBeginPairing,
  bleUnpair,
  bleUnpairAll,
  listenBleStatus,
  networkStatus,
  networkSetEnabled,
  networkBeginPairing,
  networkUnpair,
  networkUnpairAll,
  listenNetworkStatus,
  getSettings,
  setEnabledAgents,
  type Snapshot,
  type BleStatus,
  type NetworkStatus,
  type AppSettings,
  type AgentKind,
} from "./tauri";

class SnapshotStore {
  snap = $state<Snapshot | null>(null);
  lastReceived = $state<number>(0);
  staleSeconds = $state<number>(0);

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
    this.#networkUnlisten?.();
    this.#networkUnlisten = null;
    this.#networkInitialized = false;
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

  // ── 네트워크(iroh) — BLE와 같은 패턴(멱등 init, 순번 기반 재조회 경합 방지) ──

  network = $state<NetworkStatus | null>(null);
  networkActionError = $state<string | null>(null);
  // begin_pairing 이 돌려준 QR 페이로드. pairing_window 가 "open" 이 아니게
  // 되면(만료/소진/닫힘) DevicePanel 이 이 값을 무시하고 숨긴다.
  networkQrPayload = $state<string | null>(null);
  #networkInitialized = false;
  #networkUnlisten: (() => void) | null = null;
  #networkReqSeq = 0;

  async initNetwork() {
    if (this.#networkInitialized) return;
    this.#networkInitialized = true;
    const seq = ++this.#networkReqSeq;
    const status = await networkStatus();
    if (seq === this.#networkReqSeq) this.network = status;
    this.#networkUnlisten = await listenNetworkStatus(async () => {
      const seq = ++this.#networkReqSeq;
      const status = await networkStatus();
      if (seq !== this.#networkReqSeq) return;
      this.network = status;
      this.networkActionError = null;
    });
  }

  async setNetworkEnabled(on: boolean) {
    try {
      await networkSetEnabled(on);
      const seq = ++this.#networkReqSeq;
      const status = await networkStatus();
      if (seq === this.#networkReqSeq) this.network = status;
      if (!on) this.networkQrPayload = null;
      this.networkActionError = null;
    } catch (e) {
      this.networkActionError = `네트워크 설정을 변경하지 못했습니다: ${e}`;
    }
  }

  async beginNetworkPairing() {
    try {
      const info = await networkBeginPairing();
      this.networkQrPayload = info.qr_payload;
      const seq = ++this.#networkReqSeq;
      const status = await networkStatus();
      if (seq === this.#networkReqSeq) this.network = status;
      this.networkActionError = null;
    } catch (e) {
      this.networkActionError = `페어링을 시작하지 못했습니다: ${e}`;
    }
  }

  async unpairNetwork(peerId: string) {
    try {
      await networkUnpair(peerId);
      const seq = ++this.#networkReqSeq;
      const status = await networkStatus();
      if (seq === this.#networkReqSeq) this.network = status;
      this.networkActionError = null;
    } catch (e) {
      this.networkActionError = `기기를 해제하지 못했습니다: ${e}`;
    }
  }

  async unpairAllNetwork() {
    try {
      await networkUnpairAll();
      const seq = ++this.#networkReqSeq;
      const status = await networkStatus();
      if (seq === this.#networkReqSeq) this.network = status;
      this.networkQrPayload = null;
      this.networkActionError = null;
    } catch (e) {
      this.networkActionError = `전체 해제에 실패했습니다: ${e}`;
    }
  }

  // ── 표시 설정(에이전트 선택) ──────────────────────────
  // 다른 기기가 설정을 바꿀 일이 없어(로컬 전용) ble/network 처럼 이벤트를
  // 구독하지 않는다 — 이 앱 안에서 사용자가 SettingsPanel 로 직접 바꿀 때만
  // 값이 바뀐다.
  settings = $state<AppSettings | null>(null);
  settingsActionError = $state<string | null>(null);
  #settingsInitialized = false;

  async initSettings() {
    if (this.#settingsInitialized) return;
    this.#settingsInitialized = true;
    this.settings = await getSettings();
  }

  async setEnabledAgents(agents: AgentKind[]) {
    try {
      await setEnabledAgents(agents);
      this.settings = { enabled_agents: agents };
      this.settingsActionError = null;
    } catch (e) {
      this.settingsActionError = `설정을 저장하지 못했습니다: ${e}`;
    }
  }
}

export const store = new SnapshotStore();
