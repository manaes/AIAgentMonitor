import {
  listenSnapshot,
  bleStatus,
  bleSetEnabled,
  listenBleStatus,
  networkStatus,
  networkSetEnabled,
  listenNetworkStatus,
  lanStatus,
  lanSetEnabled,
  listenLanStatus,
  pairingStatus,
  beginPairing,
  unpair,
  unpairAll,
  getSettings,
  setEnabledAgents,
  setAntigravityPollInterval,
  type Snapshot,
  type BleStatus,
  type NetworkStatus,
  type LanStatus,
  type AppSettings,
  type AgentKind,
  type PairingStatus,
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
    this.#lanUnlisten?.();
    this.#lanUnlisten = null;
    this.#lanInitialized = false;
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
      // 전송을 켜고 끄면 페어링 영역에 무엇을 보여줄지가 달라진다.
      await this.#refreshPairing();
    } catch (e) {
      this.bleActionError = `BLE 설정을 변경하지 못했습니다: ${e}`;
    }
  }

  // ── 네트워크(iroh) — BLE와 같은 패턴(멱등 init, 순번 기반 재조회 경합 방지) ──

  network = $state<NetworkStatus | null>(null);
  networkActionError = $state<string | null>(null);
  // begin_pairing 이 돌려준 QR 페이로드. pairing_window 가 "open" 이 아니게
  // 되면(만료/소진/닫힘) DevicePanel 이 이 값을 무시하고 숨긴다.
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
      this.networkActionError = null;
      // 네트워크를 켜면 (창이 열려 있다면) QR 이 생긴다 — 다시 읽어야 보인다.
      await this.#refreshPairing();
    } catch (e) {
      this.networkActionError = `네트워크 설정을 변경하지 못했습니다: ${e}`;
    }
  }

  // ── LAN(WebSocket) — BLE·네트워크와 같은 패턴 ──────────
  // 세 토글은 서로 독립이다. 백엔드도 그렇게 배선돼 있어(lan_set_enabled)
  // 여기서 다른 전송을 건드릴 이유가 없다.

  lan = $state<LanStatus | null>(null);
  lanActionError = $state<string | null>(null);
  #lanInitialized = false;
  #lanUnlisten: (() => void) | null = null;
  #lanReqSeq = 0;

  async initLan() {
    if (this.#lanInitialized) return;
    this.#lanInitialized = true;
    const seq = ++this.#lanReqSeq;
    const status = await lanStatus();
    if (seq === this.#lanReqSeq) this.lan = status;
    // 리스너 bind 실패(포트 점유)는 토글을 켠 **뒤에** 도착한다 — 백엔드가
    // lan_status 이벤트를 쏘고 여기서 다시 읽는 이 경로가 그 오류가 화면에
    // 닿는 유일한 길이다. 폴링은 없다.
    this.#lanUnlisten = await listenLanStatus(async () => {
      const seq = ++this.#lanReqSeq;
      const status = await lanStatus();
      if (seq !== this.#lanReqSeq) return;
      this.lan = status;
      this.lanActionError = null;
    });
  }

  async setLanEnabled(on: boolean) {
    try {
      await lanSetEnabled(on);
      const seq = ++this.#lanReqSeq;
      const status = await lanStatus();
      if (seq === this.#lanReqSeq) this.lan = status;
      this.lanActionError = null;
      // LAN 만 켠 상태에서도 페어링 영역이 나와야 한다 — 켜고 끄면 무엇을
      // 보여줄지가 달라지므로 공유 페어링 상태를 다시 읽는다.
      await this.#refreshPairing();
    } catch (e) {
      this.lanActionError = `LAN 설정을 변경하지 못했습니다: ${e}`;
    }
  }

  // ── 페어링 (BLE·네트워크·LAN 공유) ────────────────────
  // 창도 코드도 기기 목록도 하나다(2026-08-25 스펙). 전송 토글과 달리
  // 여기엔 이벤트 push 가 없어 동작 직후 직접 다시 읽는다.
  pairing = $state<PairingStatus | null>(null);
  pairingActionError = $state<string | null>(null);
  #pairingInitialized = false;
  #pairingReqSeq = 0;

  async #refreshPairing() {
    const seq = ++this.#pairingReqSeq;
    const status = await pairingStatus();
    if (seq === this.#pairingReqSeq) this.pairing = status;
  }

  async initPairing() {
    if (this.#pairingInitialized) return;
    this.#pairingInitialized = true;
    await this.#refreshPairing();
  }

  async beginPairing() {
    try {
      await beginPairing();
      await this.#refreshPairing();
      this.pairingActionError = null;
    } catch (e) {
      this.pairingActionError = `페어링을 시작하지 못했습니다: ${e}`;
    }
  }

  async unpair(peerId: string) {
    try {
      await unpair(peerId);
      await this.#refreshPairing();
      this.pairingActionError = null;
    } catch (e) {
      this.pairingActionError = `기기를 해제하지 못했습니다: ${e}`;
    }
  }

  async unpairAll() {
    try {
      await unpairAll();
      await this.#refreshPairing();
      this.pairingActionError = null;
    } catch (e) {
      this.pairingActionError = `전체 해제에 실패했습니다: ${e}`;
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
      this.settings = {
        enabled_agents: agents,
        antigravity_poll_interval_secs: this.settings?.antigravity_poll_interval_secs ?? 300,
      };
      this.settingsActionError = null;
    } catch (e) {
      this.settingsActionError = `설정을 저장하지 못했습니다: ${e}`;
    }
  }

  async setAntigravityPollInterval(seconds: number) {
    try {
      await setAntigravityPollInterval(seconds);
      this.settings = {
        enabled_agents: this.settings?.enabled_agents ?? ["claude", "codex", "antigravity"],
        antigravity_poll_interval_secs: seconds,
      };
      this.settingsActionError = null;
    } catch (e) {
      this.settingsActionError = `Antigravity 갱신 주기를 저장하지 못했습니다: ${e}`;
    }
  }
}

export const store = new SnapshotStore();
