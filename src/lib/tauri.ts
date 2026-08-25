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

// ── 페어링 (BLE·네트워크 공유) ──────────────────────────────
// 창도 코드도 기기 목록도 하나다(2026-08-25 스펙) — 전송별 status 와 분리해
// 여기서만 다룬다. 그래야 시도 5회 예산이 두 전송에 걸쳐 하나로 유지된다.

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
};

export type PairingInfo = {
  code: string;
  // 네트워크 공유가 켜져 있을 때만 채워진다 — BLE 만 켜져 있으면 QR 을 그릴
  // 이유가 없다. 같은 코드가 6자리로도, QR 안에도 들어 있다.
  qr_payload: string | null;
};

export async function pairingStatus(): Promise<PairingStatus> {
  return invoke<PairingStatus>("pairing_status");
}

export async function beginPairing(): Promise<PairingInfo> {
  return invoke<PairingInfo>("begin_pairing");
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
