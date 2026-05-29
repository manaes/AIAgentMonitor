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
  return invoke<TriggerRule>("add_trigger_rule", { agent, hour, minute, working_dir, prompt });
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
