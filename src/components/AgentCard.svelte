<script lang="ts">
  import { formatTokensPerSec } from "../lib/format";
  import type { AgentState } from "../lib/tauri";
  import QuotaBar from "./QuotaBar.svelte";

  let { agent } = $props<{ agent: AgentState }>();
  let dotColor = $derived(
    agent.kind === "claude" ? "#30d158" : "#ff9f0a"
  );
  let primaryProj = $derived(
    agent.projects.find((p) => p.status === "active") ?? agent.projects[0]
  );
</script>

<div class="card">
  <div class="top">
    <div class="agent">
      <span class="dot" style="background:{dotColor}"></span>
      <span class="name">{agent.kind === "claude" ? "Claude Code" : "Codex"}</span>
    </div>
    <span class="subtle">{primaryProj?.model ?? "—"}</span>
  </div>
  <div class="big">
    {formatTokensPerSec(agent.rate_tok_per_sec)}
    <span class="unit">tok/s</span>
  </div>
  <div class="proj subtle">
    {primaryProj?.name ?? "no active session"}
  </div>
  <QuotaBar
    tokens_5h={agent.tokens_5h}
    quota_limit={agent.quota_limit}
    reset_at={agent.quota_reset_at}
  />
</div>

<style>
  .card { background: #2c2c2e; border-radius: 10px; padding: 10px 12px; }
  .card + :global(.card) { margin-top: 8px; }
  .top { display: flex; justify-content: space-between; align-items: center; }
  .agent { display: flex; align-items: center; gap: 6px; }
  .dot { width: 8px; height: 8px; border-radius: 50%; display: inline-block; }
  .name { font-weight: 600; }
  .big { font-size: 22px; font-weight: 700; font-variant-numeric: tabular-nums; margin: 4px 0 2px; }
  .unit { font-size: 11px; color: #8e8e93; font-weight: 500; margin-left: 4px; }
  .proj { margin-bottom: 8px; }
  .subtle { color: #8e8e93; font-size: 11px; }
</style>
