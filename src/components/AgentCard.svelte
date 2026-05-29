<script lang="ts">
  import { onMount } from "svelte";
  import { formatTokensPerSec } from "../lib/format";
  import type { AgentState } from "../lib/tauri";
  import QuotaBar from "./QuotaBar.svelte";

  let { agent } = $props<{ agent: AgentState }>();

  let dotColor = $derived(agent.kind === "claude" ? "#30d158" : "#ff9f0a");
  let primaryProj = $derived(
    agent.projects.find((p) => p.status === "active") ?? agent.projects[0]
  );

  // ── localStorage 키 ──────────────────────────────────────────────
  const KEY_LIMIT     = `ai-monitor-quota-limit-${agent.kind}`;
  const KEY_PLAN      = `ai-monitor-quota-plan-${agent.kind}`;
  const KEY_MANUAL_PCT  = `ai-monitor-manual-pct-${agent.kind}`;
  const KEY_MANUAL_RESET= `ai-monitor-manual-reset-${agent.kind}`; // "HH:MM"

  // ── 플랜 선택 ────────────────────────────────────────────────────
  const PLANS = agent.kind === "claude"
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
  let manualPct   = $state<number | null>(null); // Claude Code에서 직접 읽은 %
  let manualReset = $state<string>("");          // "HH:MM"
  let editing     = $state<"none" | "plan" | "pct" | "reset">("none");
  let editPct     = $state("");
  let editReset   = $state("");
  let editCustom  = $state("");

  onMount(() => {
    const rawLimit = localStorage.getItem(KEY_LIMIT);
    const rawPlan  = localStorage.getItem(KEY_PLAN);
    const rawPct   = localStorage.getItem(KEY_MANUAL_PCT);
    const rawReset = localStorage.getItem(KEY_MANUAL_RESET);
    if (rawLimit) localLimit = parseInt(rawLimit, 10) || null;
    if (rawPlan)  selectedPlan = rawPlan;
    if (rawPct)   manualPct = parseFloat(rawPct);
    if (rawReset) manualReset = rawReset;
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
    if (v >= 0 && v <= 100) { manualPct = v; localStorage.setItem(KEY_MANUAL_PCT, String(v)); }
    else { manualPct = null; localStorage.removeItem(KEY_MANUAL_PCT); }
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

  <div class="proj subtle">
    {primaryProj?.name ?? "no active session"}
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

    <!-- 실제 사용 % (Claude Code에서 확인한 값 입력) -->
    {#if editing === "pct"}
      <input class="mini-input pct-input" bind:value={editPct}
        onblur={commitPct} onkeydown={onPctKey}
        placeholder="0~100" autofocus />
      <span class="unit-hint">%</span>
    {:else}
      <button class="inline-btn" onclick={startEditPct}
        title="Claude Code /usage 에서 확인한 실제 사용%를 입력하세요">
        {manualPct !== null ? `${manualPct}% ✎` : "사용% 입력"}
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
  .proj { margin-bottom: 6px; }
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
  .inline-btn {
    background: none; border: none; padding: 0;
    color: #636366; font-size: 10px; cursor: pointer;
    text-decoration: underline dotted;
  }
  .inline-btn:hover { color: #0a84ff; }
  .divider { color: #3a3a3c; font-size: 10px; }
</style>
