<script lang="ts">
  import { onMount } from "svelte";
  import { formatTokensPerSec } from "../lib/format";
  import type { AgentState } from "../lib/tauri";
  import QuotaBar from "./QuotaBar.svelte";
  import { invoke } from "@tauri-apps/api/core";

  let { agent }: { agent: AgentState } = $props();

  let dotColor = $derived(agent.kind === "claude" ? "#30d158" : "#ff9f0a");
  let primaryProj = $derived(
    agent.projects.find((p) => p.status === "active") ?? agent.projects[0]
  );

  // 초 단위 카운트다운 갱신
  let nowSecs = $state(Math.floor(Date.now() / 1000));
  onMount(() => {
    const tick = setInterval(() => { nowSecs = Math.floor(Date.now() / 1000); }, 1_000);
    return () => clearInterval(tick);
  });

  // 리셋 시각(epoch secs) — 백엔드가 프록시 실측 또는 첫-메시지 앵커 추정으로 제공
  let resetEpochSecs = $derived(agent.quota_reset_at?.secs_since_epoch ?? null);

  // 5h 윈도우 리셋 여부 — 리셋됨이면 백엔드 갱신 전까지 5h 사용률을 0%로 표시
  let isReset5h = $derived(resetEpochSecs !== null && resetEpochSecs - nowSecs <= 0);

  let countdown = $derived.by((): string | null => {
    const r = resetEpochSecs;
    if (r === null) return null;
    const rem = r - nowSecs;
    if (rem <= 0) return "리셋됨";
    const h = Math.floor(rem / 3600);
    const m = Math.floor((rem % 3600) / 60);
    const s = rem % 60;
    return h > 0 ? `약 ${h}시간 ${m}분 ${s}초 남음` : `약 ${m}분 ${s}초 남음`;
  });

  // 수동 동기화: claude를 프록시 경유로 1회 핑(이 핑만 프록시를 거침). 값은 스냅샷으로 자동 반영.
  let syncing = $state(false);
  async function syncQuota() {
    if (syncing) return;
    syncing = true;
    try { await invoke("sync_quota"); } catch (e) { console.error("sync_quota", e); }
    setTimeout(() => { syncing = false; }, 6000);
  }
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

  <div class="proj-row">
    <span class="subtle">{primaryProj?.name ?? "no active session"}</span>
    {#if countdown}
      <span class="countdown">{countdown}</span>
    {/if}
  </div>

  <QuotaBar tokens_5h={agent.tokens_5h} auto_pct={agent.quota_used_pct} weekly_pct={agent.quota_used_pct_weekly} reset_5h={isReset5h} />

  {#if agent.kind === "claude"}
    <div class="sync-row">
      <button class="inline-btn" onclick={syncQuota} disabled={syncing}
        title="프록시로 실제 5h 사용량 동기화 (claude를 1회 핑). 활동 중엔 10분마다 자동 보정.">
        {syncing ? "동기화 중…" : "🔄 동기화"}
      </button>
    </div>
  {/if}
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
  .proj-row {
    display: flex; justify-content: space-between; align-items: center;
    margin-bottom: 6px;
  }
  .countdown {
    font-size: 11px; font-variant-numeric: tabular-nums; font-weight: 600;
    color: #ff9f0a;
  }
  .subtle { color: #8e8e93; font-size: 11px; }
  .sync-row { display: flex; justify-content: flex-end; margin-top: 6px; }
  .inline-btn {
    background: none; border: none; padding: 0;
    color: #636366; font-size: 10px; cursor: pointer;
    text-decoration: underline dotted;
  }
  .inline-btn:hover:not(:disabled) { color: #0a84ff; }
  .inline-btn:disabled { opacity: 0.6; cursor: default; }
</style>
