import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Snapshot = {
  emitted_at: { secs_since_epoch: number; nanos_since_epoch: number };
  agents: AgentState[];
};

export type AgentKind = "claude" | "codex";
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
  triggered_by: string | null;
};

export async function listenSnapshot(cb: (s: Snapshot) => void): Promise<UnlistenFn> {
  return listen<Snapshot>("snapshot", (e) => cb(e.payload));
}

// ── Anchor Trigger ──────────────────────────────────────────────

export type TriggerRule = {
  id: string;
  agent: "claude" | "codex";
  cron: string;
  working_dir: string;
  prompt: string;
  enabled: boolean;
  created_at: number;
};

export async function listTriggerRules(): Promise<TriggerRule[]> {
  return invoke<TriggerRule[]>("list_trigger_rules");
}

export async function addTriggerRule(
  agent: "claude" | "codex",
  hour: number,
  minute: number,
  working_dir: string,
  prompt: string
): Promise<TriggerRule> {
  // Tauri 2는 Rust snake_case → JS camelCase 자동 변환: working_dir → workingDir
  return invoke<TriggerRule>("add_trigger_rule", { agent, hour, minute, workingDir: working_dir, prompt });
}

export async function removeTriggerRule(id: string): Promise<void> {
  return invoke<void>("remove_trigger_rule", { id });
}

export async function toggleTriggerRule(id: string): Promise<TriggerRule> {
  return invoke<TriggerRule>("toggle_trigger_rule", { id });
}

export async function fireTriggerNow(id: string): Promise<void> {
  return invoke<void>("fire_trigger_now", { id });
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
