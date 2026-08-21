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
  // 이 값이 false 면 Devices 탭 자체를 노출하지 않는다.
  supported: boolean;
  enabled: boolean;
  advertising: boolean;
  peers: BlePeer[];
  // 마지막 BLE 오류. 이 앱에는 tracing subscriber 가 없어 tracing::error! 출력이 전부 유실되므로,
  // 블루투스 권한 거부 같은 실패는 이 필드로만 사용자에게 도달한다.
  last_error: string | null;
  // 페어링 창 상태. UI 가 만료와 시도 소진을 구분해 보여줘야 한다 —
  // 소진이 보인다는 것이 창에 소유자를 두지 않기로 한 근거의 절반이다(스펙 5.1).
  // expires_at 은 절대 epoch 초다 — ble_status 이벤트가 BLE 활동이 있을 때만
  // 발행되므로, 프론트가 이 값을 한 번만 받아 자체 타이머로 카운트다운을
  // 계산해야 한다(AgentCard 의 quota_reset_at 과 같은 패턴).
  pairing_window:
    | { kind: "open"; code: string; expires_at: number; attempts_left: number }
    | { kind: "exhausted" }
    | { kind: "closed" };
  paired_peers: { peer_id: string; paired_at: number; connected: boolean }[];
};

export async function bleStatus(): Promise<BleStatus> {
  return invoke<BleStatus>("ble_status");
}

export async function bleSetEnabled(enabled: boolean): Promise<void> {
  return invoke<void>("ble_set_enabled", { enabled });
}

export async function bleBeginPairing(): Promise<void> {
  return invoke<void>("ble_begin_pairing");
}

export async function bleUnpair(peerId: string): Promise<void> {
  return invoke<void>("ble_unpair", { peerId });
}

export async function bleUnpairAll(): Promise<void> {
  return invoke<void>("ble_unpair_all");
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
  pairing_window:
    | { kind: "open"; code: string; expires_at: number; attempts_left: number }
    | { kind: "exhausted" }
    | { kind: "closed" };
  paired_peers: { peer_id: string; paired_at: number; connected: boolean }[];
};

export type NetworkPairingInfo = {
  code: string;
  // iOS가 QR로 스캔할 페이로드(EndpointId + 코드) — 스캔 한 번으로 dial과
  // CODE: 제출이 자동으로 끝난다.
  qr_payload: string;
};

export async function networkStatus(): Promise<NetworkStatus> {
  return invoke<NetworkStatus>("network_status");
}

export async function networkSetEnabled(enabled: boolean): Promise<void> {
  return invoke<void>("network_set_enabled", { enabled });
}

export async function networkBeginPairing(): Promise<NetworkPairingInfo> {
  return invoke<NetworkPairingInfo>("network_begin_pairing");
}

export async function networkUnpair(peerId: string): Promise<void> {
  return invoke<void>("network_unpair", { peerId });
}

export async function networkUnpairAll(): Promise<void> {
  return invoke<void>("network_unpair_all");
}

export async function listenNetworkStatus(cb: () => void): Promise<UnlistenFn> {
  return listen("network_status", () => cb());
}
