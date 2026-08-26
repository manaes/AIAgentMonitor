import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Snapshot = {
  emitted_at: { secs_since_epoch: number; nanos_since_epoch: number };
  agents: AgentState[];
};

export type AgentKind = "claude" | "codex" | "antigravity";
export type ActivityStatus = "active" | "idle" | "dormant";

export type TokenCounts = {
  tokens_in: number;
  tokens_out: number;
  tokens_cache_read: number;
  tokens_cache_create: number;
};

export type ProjectActivity = {
  path: string;
  name: string;
  model: string;
  rate_tok_per_sec: number;
  last_event_at: { secs_since_epoch: number };
  status: ActivityStatus;
};

export type AgentState = {
  kind: AgentKind;
  rate_tok_per_sec: number;
  tokens_5h: TokenCounts;
  quota_limit: number | null;
  quota_reset_at: { secs_since_epoch: number } | null;
  quota_used_pct: number | null;
  quota_reset_at_weekly: { secs_since_epoch: number } | null;
  quota_used_pct_weekly: number | null;
  projects: ProjectActivity[];
};

export async function listenSnapshot(cb: (s: Snapshot) => void): Promise<UnlistenFn> {
  return listen<Snapshot>("snapshot", (e) => cb(e.payload));
}

// ── BLE 미러 ────────────────────────────────────────────────

export type BlePeer = { id: string; mtu: number };
export type BleStatus = {
  // 이 빌드에 실제 BLE 구현이 있는지(macOS 만 true). Windows 는 아무 일도 일어나지 않으므로
  // 이 값이 false 면 **BLE 토글만** 숨긴다 — 탭 자체는 그대로다. 네트워크와 LAN 은
  // 어디서나 supported 라 윈도우에서도 그 둘은 보여야 한다.
  supported: boolean;
  enabled: boolean;
  advertising: boolean;
  peers: BlePeer[];
  // 마지막 BLE 오류. 이 앱에는 tracing subscriber 가 없어 tracing::error! 출력이 전부 유실되므로,
  // 블루투스 권한 거부 같은 실패는 이 필드로만 사용자에게 도달한다.
  last_error: string | null;
};

export async function bleStatus(): Promise<BleStatus> {
  return invoke<BleStatus>("ble_status");
}

export async function bleSetEnabled(enabled: boolean): Promise<void> {
  return invoke<void>("ble_set_enabled", { enabled });
}

export async function listenBleStatus(cb: () => void): Promise<UnlistenFn> {
  return listen("ble_status", () => cb());
}

// ── 네트워크(iroh) 미러 ─────────────────────────────────────
//
// BLE와 같은 페어링 인증 프로토콜(6자리 코드, nonce+HMAC 재인증)을 그대로
// 쓰므로 pairing_window/paired_peers 모양은 BleStatus와 동일하다 — 백엔드가
// 같은 Rust 타입(ble::pairing::PairingWindow/PairedPeer)을 직렬화한다.

export type NetworkStatus = {
  // iroh는 크로스플랫폼이라 이 값은 항상 true — BLE의 supported와 대칭되는
  // "백엔드가 선언하는 능력" 규약을 그대로 따른다.
  supported: boolean;
  enabled: boolean;
  endpoint_id: string;
  last_error: string | null;
};

export async function networkStatus(): Promise<NetworkStatus> {
  return invoke<NetworkStatus>("network_status");
}

export async function networkSetEnabled(enabled: boolean): Promise<void> {
  return invoke<void>("network_set_enabled", { enabled });
}

export async function listenNetworkStatus(cb: () => void): Promise<UnlistenFn> {
  return listen("network_status", () => cb());
}

// ── LAN(WebSocket) 미러 ─────────────────────────────────────
//
// 같은 WiFi 의 전용 기기(CYD)용 전송이다. QR 은 없다 — 그 기기에는 카메라가
// 없어서, 사람이 주소를 손으로 넣고 6자리 코드를 입력한다.

export type LanStatus = {
  // iroh 와 같은 이유로 언제나 true — 리스너는 그냥 WebSocket 이고 게시·주소
  // 조회도 표준 라이브러리만 쓴다(백엔드 LAN_SUPPORTED 의 doc).
  supported: boolean;
  enabled: boolean;
  // **포트까지 붙은 완성된 문자열**(`192.168.0.12:4320`). 프론트는 포트를 알지
  // 못하고 알 필요도 없다 — 4320 은 Rust 의 `lan::server::PORT` 한 곳에만 있다.
  // **리스너가 서 있을 때만** 값이 있다 — 공유가 꺼져 있을 때는 물론이고, 토글은
  // 켜졌는데 bind 에 실패한 경우에도(enabled 는 true 인 채로) null 이다. 열려 있지
  // 않은 포트를 손으로 넣으라고 안내하지 않기 위해서다. 라우팅 가능한 IPv4 가
  // 없어도 null 이다.
  address: string | null;
  // 포트 점유 같은 실패. BLE 의 last_error 와 같은 이유로 존재한다 — 이 앱은
  // 로그 파일을 남기지 않아 이 필드가 사용자가 알 수 있는 유일한 경로다.
  last_error: string | null;
};

export async function lanStatus(): Promise<LanStatus> {
  return invoke<LanStatus>("lan_status");
}

export async function lanSetEnabled(enabled: boolean): Promise<void> {
  return invoke<void>("lan_set_enabled", { enabled });
}

export async function listenLanStatus(cb: () => void): Promise<UnlistenFn> {
  return listen("lan_status", () => cb());
}

// ── 페어링 (BLE·네트워크·LAN 공유) ──────────────────────────
// 창도 코드도 기기 목록도 하나다(2026-08-25 스펙) — 전송별 status 와 분리해
// 여기서만 다룬다. 그래야 시도 5회 예산이 세 전송(BLE·네트워크·LAN)에 걸쳐
// 하나로 유지된다.

export type PairingWindow =
  // expires_at 은 절대 epoch 초다 — status 이벤트가 활동이 있을 때만 발행되므로,
  // 프론트가 이 값을 한 번만 받아 자체 타이머로 카운트다운을 계산해야 한다
  // (AgentCard 의 quota_reset_at 과 같은 패턴).
  | { kind: "open"; code: string; expires_at: number; attempts_left: number }
  | { kind: "exhausted" }
  | { kind: "closed" };

export type PairedPeer = { peer_id: string; paired_at: number; connected: boolean };

export type PairingStatus = {
  pairing_window: PairingWindow;
  paired_peers: PairedPeer[];
  // 창이 열려 있고 네트워크 공유가 켜져 있을 때만 채워진다. 상태에서 파생되므로
  // 창을 연 뒤에 네트워크를 켜도 바로 따라온다 — begin_pairing 응답에 한 번만
  // 실어 보내던 초안은 그 경우 QR 이 영영 안 나오는 버그였다.
  qr_payload: string | null;
};

export async function pairingStatus(): Promise<PairingStatus> {
  return invoke<PairingStatus>("pairing_status");
}

export async function beginPairing(): Promise<void> {
  return invoke<void>("begin_pairing");
}

export async function unpair(peerId: string): Promise<void> {
  return invoke<void>("unpair", { peerId });
}

export async function unpairAll(): Promise<void> {
  return invoke<void>("unpair_all");
}

// ── 표시 설정(에이전트 선택) ──────────────────────────────
// 워처는 이 설정과 무관하게 계속 돈다 — 백엔드가 매 틱마다 Snapshot 을 이
// 목록으로 걸러서 내보낼 뿐이다. BLE/네트워크 미러 페이로드도 같은 Snapshot
// 에서 만들어지므로 iOS 쪽도 자동으로 같은 필터를 반영한다.

export type AppSettings = {
  enabled_agents: AgentKind[];
};

export async function getSettings(): Promise<AppSettings> {
  return invoke<AppSettings>("get_settings");
}

export async function setEnabledAgents(agents: AgentKind[]): Promise<void> {
  return invoke<void>("set_enabled_agents", { agents });
}
