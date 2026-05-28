<script lang="ts">
  import { formatTokensPerSec, relativeTime } from "../lib/format";
  import type { Snapshot, ProjectActivity } from "../lib/tauri";

  let { snap } = $props<{ snap: Snapshot }>();

  type Row = ProjectActivity & { agent: "claude" | "codex" };
  let rows = $derived<Row[]>(
    snap.agents
      .flatMap((a) => a.projects.map((p) => ({ ...p, agent: a.kind })))
      .sort((a, b) => b.last_event_at.secs_since_epoch - a.last_event_at.secs_since_epoch)
  );

  function dotColor(agent: string, status: string) {
    if (status === "dormant") return "#636366";
    if (status === "idle") return "#ff9f0a";
    return agent === "claude" ? "#30d158" : "#ff9f0a";
  }
</script>

<div class="list">
  <p class="label">Active sessions · sorted by recent activity</p>
  {#each rows as row (row.agent + row.path)}
    <div class="row">
      <span class="left">
        <span class="dot" style="background:{dotColor(row.agent, row.status)}"></span>
        <strong>{row.agent === "claude" ? "Claude" : "Codex"}</strong>
        <span class="proj">· {row.name}</span>
        <span class="model subtle">{row.model}</span>
      </span>
      <span class="right">
        {#if row.status === "active"}
          <span class="rate">{formatTokensPerSec(row.rate_tok_per_sec)} tok/s</span>
        {:else}
          <span class="subtle">{row.status}</span>
        {/if}
        <span class="subtle">{relativeTime(row.last_event_at.secs_since_epoch)}</span>
      </span>
    </div>
  {:else}
    <p class="subtle">No sessions yet.</p>
  {/each}
</div>

<style>
  .list { background: #2c2c2e; border-radius: 8px; padding: 10px 12px; }
  .label { font-size: 9px; color: #8e8e93; text-transform: uppercase; letter-spacing: 0.4px; margin: 0 0 6px; }
  .row { display: flex; justify-content: space-between; align-items: center; padding: 6px 0; font-size: 11px; }
  .row + .row { border-top: 1px solid #3a3a3c; }
  .left { display: flex; align-items: center; gap: 6px; }
  .right { display: flex; gap: 12px; align-items: center; }
  .dot { width: 6px; height: 6px; border-radius: 50%; display: inline-block; }
  .proj { font-weight: 500; }
  .model { margin-left: 4px; }
  .rate { color: #0a84ff; font-variant-numeric: tabular-nums; font-weight: 600; }
  .subtle { color: #8e8e93; }
</style>
