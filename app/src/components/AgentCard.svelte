<script lang="ts">
  import { onMount } from "svelte";
  import { formatTokensPerSec } from "../lib/format";
  import type { AgentState } from "../lib/tauri";
  import QuotaBar from "./QuotaBar.svelte";
  import { invoke } from "@tauri-apps/api/core";

  let { agent }: { agent: AgentState } = $props();

  let dotColor = $derived(
    agent.kind === "claude"
      ? "#30d158"
      : agent.kind === "antigravity"
        ? "#388bfd"
        : "#ff9f0a"
  );
  let primaryProj = $derived(
    agent.projects.find((p) => p.status === "active") ?? agent.projects[0]
  );

  // 초 단위 카운트다운 갱신
  let nowSecs = $state(Math.floor(Date.now() / 1000));
  onMount(() => {
    const tick = setInterval(() => { nowSecs = Math.floor(Date.now() / 1000); }, 1_000);
    return () => clearInterval(tick);
  });

  // 리셋 시각(epoch secs) — 5h 및 주간
  let resetEpochSecs = $derived(agent.quota_reset_at?.secs_since_epoch ?? null);
  let resetWeeklyEpochSecs = $derived(agent.quota_reset_at_weekly?.secs_since_epoch ?? null);

  // 5h 윈도우 리셋 여부 — 리셋됨이면 백엔드 갱신 전까지 5h 사용률을 0%로 표시
  let isReset5h = $derived(resetEpochSecs !== null && resetEpochSecs - nowSecs <= 0);

  let countdown = $derived.by((): string | null => {
    // 한도를 못 읽는 중이면 리셋 카운트다운도 숨긴다 — 같은 (낡은) 스냅샷에서
    // 나온 값이라 %만 가리고 이건 남기면 앞뒤가 안 맞는다.
    if (agent.quota_error) return null;
    if (resetEpochSecs !== null) {
      const rem = resetEpochSecs - nowSecs;
      if (rem <= 0) return "리셋됨";
      const h = Math.floor(rem / 3600);
      const m = Math.floor((rem % 3600) / 60);
      const s = rem % 60;
      return h > 0 ? `약 ${h}시간 ${m}분 ${s}초 남음` : `약 ${m}분 ${s}초 남음`;
    }
    if (resetWeeklyEpochSecs !== null) {
      const rem = resetWeeklyEpochSecs - nowSecs;
      if (rem <= 0) return "주간 리셋됨";
      const d = Math.floor(rem / 86400);
      const h = Math.floor((rem % 86400) / 3600);
      const m = Math.floor((rem % 3600) / 60);
      return d > 0 ? `약 ${d}일 ${h}시간 남음` : `약 ${h}시간 ${m}분 남음`;
    }
    return null;
  });

  // 수동 동기화: Claude는 프록시 핑, Codex는 app-server 한도 조회 RPC,
  // Antigravity는 `agy -p /usage`를 즉시 조회한다.
  let syncing = $state(false);
  const SYNC_COMMAND: Record<string, string> = {
    claude: "sync_quota",
    codex: "sync_codex_quota",
    antigravity: "sync_antigravity_quota",
  };
  const SYNC_HINT: Record<string, string> = {
    claude: "프록시로 실제 5h 사용량 동기화 (claude를 1회 핑). 활동 중엔 10분마다 자동 보정.",
    codex: "codex app-server의 한도 조회 RPC로 5시간·주간 사용량을 즉시 갱신합니다 (토큰 소모 없음).",
    antigravity: "agy /usage로 Gemini 5시간·주간 사용량을 즉시 갱신합니다.",
  };
  async function syncQuota() {
    if (syncing) return;
    syncing = true;
    const command = SYNC_COMMAND[agent.kind] ?? "sync_quota";
    try { await invoke(command); } catch (e) { console.error(command, e); }
    setTimeout(() => { syncing = false; }, 6000);
  }
</script>

<div class="card">
  <div class="top">
    <div class="agent">
      <span class="dot" style="background:{dotColor}"></span>
      <span class="name">
        {agent.kind === "claude" ? "Claude Code" : agent.kind === "antigravity" ? "Antigravity" : "Codex"}
      </span>
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

  <!-- 사용량을 못 읽는 중이면 이유를 그대로 띄운다. 이게 없으면 "안 쓰는 중"과
       "로그인이 풀려서 못 읽는 중"이 둘 다 0%로 똑같이 보인다. -->
  {#if agent.quota_error}
    <div class="quota-error" title={agent.quota_error}>⚠ {agent.quota_error}</div>
  {/if}

  <QuotaBar tokens_5h={agent.tokens_5h} auto_pct={agent.quota_used_pct} weekly_pct={agent.quota_used_pct_weekly} reset_5h={isReset5h} unreadable={!!agent.quota_error} />

  <div class="sync-row">
    <button class="inline-btn" onclick={syncQuota} disabled={syncing} title={SYNC_HINT[agent.kind]}>
      {syncing ? "동기화 중…" : "🔄 동기화"}
    </button>
  </div>
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
  .quota-error {
    font-size: 11px; line-height: 1.35; color: #ff9f0a;
    background: rgba(255, 159, 10, 0.12);
    border-radius: 6px; padding: 4px 6px; margin-bottom: 6px;
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .sync-row { display: flex; justify-content: flex-end; margin-top: 6px; }
  .inline-btn {
    background: none; border: none; padding: 0;
    color: #636366; font-size: 10px; cursor: pointer;
    text-decoration: underline dotted;
  }
  .inline-btn:hover:not(:disabled) { color: #0a84ff; }
  .inline-btn:disabled { opacity: 0.6; cursor: default; }
</style>
