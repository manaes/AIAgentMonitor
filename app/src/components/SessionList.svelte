<script lang="ts">
  import { formatTokensPerSec, relativeTime } from "../lib/format";
  import type { Snapshot, ProjectActivity } from "../lib/tauri";

  let { snap }: { snap: Snapshot } = $props();

  type Row = ProjectActivity & { agent: "claude" | "codex" | "antigravity" };
  let rows = $derived<Row[]>(
    snap.agents
      .flatMap((a) => a.projects.map((p) => ({ ...p, agent: a.kind })))
      .sort((a, b) => b.last_event_at.secs_since_epoch - a.last_event_at.secs_since_epoch)
  );

  function dotColor(agent: string, status: string) {
    if (status === "dormant") return "#636366";
    if (status === "idle") return "#ff9f0a";
    if (agent === "claude") return "#30d158";
    if (agent === "antigravity") return "#388bfd";
    return "#bf5af2"; // codex — idle(주황)과 혼동되지 않도록 전용 색 배정
  }

  function statusLabel(status: string): string {
    if (status === "idle") return "유휴";
    if (status === "dormant") return "휴면";
    return status;
  }
</script>

<!--
  session_id 를 각 (agent, session) 행의 each-키로만 쓴다(화면엔 안 보임) —
  같은 폴더에서 세션 두 개가 동시에 돌 때 path 만으로는 키가 겹친다
  (2026-09-02, 실기 재현 확인 후 세션 단위 집계로 수정).
-->
<div class="list">
  <p class="label">Active sessions · sorted by recent activity</p>
  {#each rows as row (row.agent + row.session_id)}
    <div class="row">
      <div class="line1">
        <span class="left">
          <span class="dot" style="background:{dotColor(row.agent, row.status)}"></span>
          <strong>{row.agent === "claude" ? "Claude" : row.agent === "antigravity" ? "Antigravity" : "Codex"}</strong>
          <span class="proj">· {row.name}</span>
          <span class="model subtle">{row.model}</span>
        </span>
        <span class="right">
          {#if row.status === "active"}
            <span class="rate">{formatTokensPerSec(row.rate_tok_per_sec)} tok/s</span>
          {:else}
            <span class="subtle">{statusLabel(row.status)}</span>
          {/if}
          <span class="subtle">{relativeTime(row.last_event_at.secs_since_epoch)}</span>
        </span>
      </div>
      <div class="line2 subtle">
        <span class="path" title={row.path}>{row.path}</span>
      </div>
      {#if row.prompt_preview}
        <div class="line3" title={row.prompt_preview}>“{row.prompt_preview}”</div>
      {/if}
    </div>
  {:else}
    <p class="subtle">No sessions yet.</p>
  {/each}
</div>

<style>
  .list { background: #2c2c2e; border-radius: 8px; padding: 10px 12px; }
  .label { font-size: 9px; color: #8e8e93; text-transform: uppercase; letter-spacing: 0.4px; margin: 0 0 6px; }
  .row { padding: 6px 0; font-size: 11px; }
  .row + .row { border-top: 1px solid #3a3a3c; }
  .line1 { display: flex; justify-content: space-between; align-items: center; }
  .line2 { margin-top: 2px; font-size: 9px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .left { display: flex; align-items: center; gap: 6px; }
  .right { display: flex; gap: 12px; align-items: center; }
  .dot { width: 6px; height: 6px; border-radius: 50%; display: inline-block; }
  .proj { font-weight: 500; }
  .model { margin-left: 4px; }
  .rate { color: #0a84ff; font-variant-numeric: tabular-nums; font-weight: 600; }
  .subtle { color: #8e8e93; }
  .line3 {
    margin-top: 3px;
    font-size: 11px;
    color: #c7c7cc;
    font-style: italic;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
