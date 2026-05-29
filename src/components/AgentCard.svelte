<script lang="ts">
  import { onMount } from "svelte";
  import { formatTokensPerSec } from "../lib/format";
  import type { AgentState } from "../lib/tauri";
  import QuotaBar from "./QuotaBar.svelte";

  let { agent, otelActive = false } = $props<{ agent: AgentState; otelActive?: boolean }>();

  let dotColor = $derived(agent.kind === "claude" ? "#30d158" : "#ff9f0a");
  let primaryProj = $derived(
    agent.projects.find((p) => p.status === "active") ?? agent.projects[0]
  );

  // ── localStorage 키 — agent.kind는 고정값이므로 $derived 불필요
  // eslint-disable-next-line svelte/no-reactive-in-const (kind is constant per instance)
  const kind = agent.kind;   // 한 번만 읽어서 const에 보관
  const KEY_LIMIT          = `ai-monitor-quota-limit-${kind}`;
  const KEY_PLAN           = `ai-monitor-quota-plan-${kind}`;
  const KEY_MANUAL_PCT     = `ai-monitor-manual-pct-${kind}`;
  const KEY_MANUAL_RESET   = `ai-monitor-manual-reset-${kind}`;
  const KEY_BASELINE_TOKENS= `ai-monitor-baseline-tokens-${kind}`;
  const KEY_PCT_ENTERED_AT = `ai-monitor-pct-entered-at-${kind}`; // 입력 시각 (epoch secs)

  // ── 플랜 선택 ────────────────────────────────────────────────────
  const PLANS = kind === "claude"
    ? [
        { label: "Free",      limit: 30_000 },
        { label: "Pro",       limit: 300_000 },
        { label: "Max (5×)",  limit: 1_500_000 },
        { label: "Max (20×)", limit: 6_000_000 },
      ]
    : [
        { label: "Codex Free",  limit: 100_000 },
        { label: "Codex Plus",  limit: 1_000_000 },
      ];

  let localLimit   = $state<number | null>(null);
  let selectedPlan = $state<string>("none");

  // ── 수동 입력 오버라이드 ─────────────────────────────────────────
  let manualPct      = $state<number | null>(null);
  let baselineTokens = $state<number | null>(null);
  let pctEnteredAt   = $state<number | null>(null);  // 입력 시각 (epoch secs)
  let manualReset    = $state<string>("");
  let editing        = $state<"none" | "plan" | "pct" | "reset">("none");
  let editPct        = $state("");
  let editReset      = $state("");
  let editCustom     = $state("");

  // ── 카운트다운 타이머 ──────────────────────────────────────────────
  let nowSecs = $state(Math.floor(Date.now() / 1000));

  onMount(() => {
    const rawLimit    = localStorage.getItem(KEY_LIMIT);
    const rawPlan     = localStorage.getItem(KEY_PLAN);
    const rawPct      = localStorage.getItem(KEY_MANUAL_PCT);
    const rawReset    = localStorage.getItem(KEY_MANUAL_RESET);
    const rawBaseline = localStorage.getItem(KEY_BASELINE_TOKENS);
    if (rawLimit)    localLimit = parseInt(rawLimit, 10) || null;
    if (rawPlan)     selectedPlan = rawPlan;
    if (rawPct)      manualPct = parseFloat(rawPct);
    if (rawReset)    manualReset = rawReset;
    if (rawBaseline) baselineTokens = parseInt(rawBaseline, 10);
    const rawEnteredAt = localStorage.getItem(KEY_PCT_ENTERED_AT);
    if (rawEnteredAt) pctEnteredAt = parseInt(rawEnteredAt, 10);

    // 1초마다 갱신 (초 단위 카운트다운)
    const tick = setInterval(() => { nowSecs = Math.floor(Date.now() / 1000); }, 1_000);
    return () => clearInterval(tick);
  });

  // reset_at을 epoch seconds로 통일
  let resetEpochSecs = $derived((): number | null => {
    if (manualReset) {
      const [hh, mm] = manualReset.split(":").map(Number);
      if (isNaN(hh) || isNaN(mm)) return null;
      const d = new Date(nowSecs * 1000);
      d.setHours(hh, mm, 0, 0);
      // 5h quota 창 — 지나면 +24h가 아니라 +5h 씩 다음 창 탐색
      while (d.getTime() / 1000 <= nowSecs) {
        d.setTime(d.getTime() + 5 * 3600 * 1000);
      }
      return Math.floor(d.getTime() / 1000);
    }
    return agent.quota_reset_at?.secs_since_epoch ?? null;
  });

  // "약 NNN분 SS초 남음" 형태 카운트다운
  let countdown = $derived((): string | null => {
    const r = resetEpochSecs();
    if (r === null) return null;
    const rem = r - nowSecs;
    if (rem <= 0) return "리셋됨";
    const h = Math.floor(rem / 3600);
    const m = Math.floor((rem % 3600) / 60);
    const s = rem % 60;
    return h > 0
      ? `약 ${h}시간 ${m}분 ${s}초 남음`
      : `약 ${m}분 ${s}초 남음`;
  });

  let effectiveLimit = $derived(agent.quota_limit ?? localLimit);

  // ── 플랜 변경 ────────────────────────────────────────────────────
  function onPlanChange(e: Event) {
    const val = (e.target as HTMLSelectElement).value;
    selectedPlan = val;
    localStorage.setItem(KEY_PLAN, val);
    if (val === "none") { localLimit = null; localStorage.removeItem(KEY_LIMIT); return; }
    if (val === "custom") { editing = "plan"; editCustom = localLimit ? String(localLimit) : ""; return; }
    const p = PLANS.find(p => p.label === val);
    if (p) { localLimit = p.limit; localStorage.setItem(KEY_LIMIT, String(p.limit)); }
  }
  function commitCustomPlan() {
    editing = "none";
    const val = parseInt(editCustom.replace(/[^0-9]/g, ""), 10);
    if (val > 0) { localLimit = val; localStorage.setItem(KEY_LIMIT, String(val)); }
    else { localLimit = null; localStorage.removeItem(KEY_LIMIT); }
  }

  // ── % 수동 입력 ──────────────────────────────────────────────────
  function startEditPct() { editing = "pct"; editPct = manualPct !== null ? String(manualPct) : ""; }
  function commitPct() {
    editing = "none";
    const v = parseFloat(editPct);
    if (v >= 0 && v <= 100) {
      manualPct = v;
      // 입력 시점의 input+output 토큰 수를 baseline으로 기록
      const snapshot = agent.tokens_5h.tokens_in + agent.tokens_5h.tokens_out;
      baselineTokens = snapshot;
      pctEnteredAt = nowSecs;
      localStorage.setItem(KEY_MANUAL_PCT, String(v));
      localStorage.setItem(KEY_BASELINE_TOKENS, String(snapshot));
      localStorage.setItem(KEY_PCT_ENTERED_AT, String(nowSecs));
    } else {
      manualPct = null;
      baselineTokens = null;
      pctEnteredAt = null;
      localStorage.removeItem(KEY_MANUAL_PCT);
      localStorage.removeItem(KEY_BASELINE_TOKENS);
      localStorage.removeItem(KEY_PCT_ENTERED_AT);
    }
  }
  function onPctKey(e: KeyboardEvent) { if (e.key === "Enter") commitPct(); if (e.key === "Escape") editing = "none"; }

  // ── 리셋 시각 수동 입력 ──────────────────────────────────────────
  function startEditReset() { editing = "reset"; editReset = manualReset; }
  function commitReset() {
    editing = "none";
    const m = editReset.match(/^(\d{1,2}):(\d{2})$/);
    if (m) { manualReset = editReset; localStorage.setItem(KEY_MANUAL_RESET, editReset); }
    else { manualReset = ""; localStorage.removeItem(KEY_MANUAL_RESET); }
  }
  function onResetKey(e: KeyboardEvent) { if (e.key === "Enter") commitReset(); if (e.key === "Escape") editing = "none"; }
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
    {#if countdown()}
      <span class="countdown">{countdown()}</span>
    {/if}
  </div>

  <!-- 설정 행: 플랜 | 실제사용% | 리셋시각 -->
  <div class="settings-row">
    <!-- 플랜 선택 -->
    <select class="plan-sel" value={selectedPlan} onchange={onPlanChange}>
      <option value="none">— 한도 —</option>
      {#each PLANS as p}
        <option value={p.label}>{p.label}</option>
      {/each}
      <option value="custom">직접 입력…</option>
    </select>

    {#if editing === "plan"}
      <input class="mini-input" bind:value={editCustom}
        onblur={commitCustomPlan}
        onkeydown={(e) => { if (e.key==="Enter") commitCustomPlan(); if (e.key==="Escape") editing="none"; }}
        placeholder="한도 토큰 수" autofocus />
    {/if}

    <span class="divider">|</span>

    <!-- 실제 사용 % — OTEL 연결 시 자동, 아니면 수동 -->
    {#if otelActive && kind === "claude"}
      <span class="otel-auto">● OTEL 자동</span>
    {:else if editing === "pct"}
      <input class="mini-input pct-input" bind:value={editPct}
        onblur={commitPct} onkeydown={onPctKey}
        placeholder="0~100" autofocus />
      <span class="unit-hint">%</span>
    {:else}
      <button class="inline-btn" onclick={startEditPct}
        title="Claude Code /usage 에서 확인한 실제 사용%를 입력 (30분마다 갱신 권장)">
        {#if manualPct !== null}
          {manualPct}% ✎{(pctEnteredAt && (nowSecs - pctEnteredAt) > 1800) ? " ⚠" : ""}
        {:else}
          사용% 입력
        {/if}
      </button>
    {/if}

    <span class="divider">|</span>

    <!-- 리셋 시각 -->
    {#if editing === "reset"}
      <input class="mini-input time-input" bind:value={editReset}
        onblur={commitReset} onkeydown={onResetKey}
        placeholder="HH:MM" autofocus />
    {:else}
      <button class="inline-btn" onclick={startEditReset}
        title="Claude Code에서 확인한 리셋 시각 (예: 11:50)">
        {manualReset || "리셋시각 ✎"}
      </button>
    {/if}
  </div>

  <QuotaBar
    tokens_5h={agent.tokens_5h}
    quota_limit={effectiveLimit}
    reset_at={agent.quota_reset_at}
    manual_pct={manualPct}
    baseline_tokens={baselineTokens}
    manual_reset={manualReset}
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
  .proj-row {
    display: flex; justify-content: space-between; align-items: center;
    margin-bottom: 6px;
  }
  .countdown {
    font-size: 11px; font-variant-numeric: tabular-nums; font-weight: 600;
    color: #ff9f0a;
  }
  .subtle { color: #8e8e93; font-size: 11px; }

  .settings-row {
    display: flex; align-items: center; gap: 5px;
    flex-wrap: wrap; margin-bottom: 8px;
  }
  .plan-sel {
    background: #1c1c1e; border: 1px solid #3a3a3c;
    border-radius: 5px; color: #8e8e93;
    font-size: 10px; padding: 2px 5px; outline: none; cursor: pointer;
  }
  .plan-sel:focus { border-color: #0a84ff; }
  .mini-input {
    background: #1c1c1e; border: 1px solid #0a84ff;
    border-radius: 4px; color: #f2f2f7;
    font-size: 11px; padding: 2px 5px; outline: none;
  }
  .pct-input { width: 45px; }
  .time-input { width: 52px; }
  .unit-hint { color: #8e8e93; font-size: 10px; }
  .otel-auto { color: #30d158; font-size: 10px; font-weight: 500; }
  .inline-btn {
    background: none; border: none; padding: 0;
    color: #636366; font-size: 10px; cursor: pointer;
    text-decoration: underline dotted;
  }
  .inline-btn:hover { color: #0a84ff; }
  .divider { color: #3a3a3c; font-size: 10px; }
</style>
