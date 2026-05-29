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

  // Claude Code 요금제별 5h 토큰 한도 (근사치)
  // 출처: Anthropic 공식 플랜 + 커뮤니티 보고
  const CLAUDE_PLANS = agent.kind === "claude"
    ? [
        { label: "Free",      limit: 40_000 },
        { label: "Pro",       limit: 900_000 },
        { label: "Max (5×)",  limit: 4_500_000 },
        { label: "Max (20×)", limit: 18_000_000 },
      ]
    : [
        { label: "Codex Free",  limit: 200_000 },
        { label: "Codex Plus",  limit: 2_000_000 },
      ];

  const STORAGE_KEY      = `ai-monitor-quota-limit-${agent.kind}`;
  const STORAGE_PLAN_KEY = `ai-monitor-quota-plan-${agent.kind}`;

  let localLimit  = $state<number | null>(null);
  let selectedPlan = $state<string>("custom");   // plan label 또는 "custom"
  let editingCustom = $state(false);
  let customValue   = $state("");

  onMount(() => {
    const rawLimit = localStorage.getItem(STORAGE_KEY);
    const rawPlan  = localStorage.getItem(STORAGE_PLAN_KEY);
    if (rawLimit) localLimit = parseInt(rawLimit, 10) || null;
    if (rawPlan)  selectedPlan = rawPlan;
  });

  let effectiveLimit = $derived(agent.quota_limit ?? localLimit);

  function onPlanChange(e: Event) {
    const val = (e.target as HTMLSelectElement).value;
    selectedPlan = val;
    localStorage.setItem(STORAGE_PLAN_KEY, val);

    if (val === "custom") {
      editingCustom = true;
      customValue = localLimit ? String(localLimit) : "";
      return;
    }
    if (val === "none") {
      localLimit = null;
      localStorage.removeItem(STORAGE_KEY);
      return;
    }
    const plan = CLAUDE_PLANS.find(p => p.label === val);
    if (plan) {
      localLimit = plan.limit;
      localStorage.setItem(STORAGE_KEY, String(plan.limit));
    }
  }

  function commitCustom() {
    editingCustom = false;
    const val = parseInt(customValue.replace(/[^0-9]/g, ""), 10);
    if (val > 0) {
      localLimit = val;
      localStorage.setItem(STORAGE_KEY, String(val));
    } else {
      localLimit = null;
      localStorage.removeItem(STORAGE_KEY);
      selectedPlan = "none";
      localStorage.setItem(STORAGE_PLAN_KEY, "none");
    }
  }

  function onCustomKey(e: KeyboardEvent) {
    if (e.key === "Enter") commitCustom();
    if (e.key === "Escape") { editingCustom = false; }
  }

  // 현재 선택된 플랜 레이블 표시용
  let planLabel = $derived(
    selectedPlan === "custom" ? "직접 입력"
    : selectedPlan === "none"  ? "한도 설정"
    : selectedPlan
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

  <!-- 플랜 선택 행 -->
  <div class="plan-row">
    <select class="plan-sel" value={selectedPlan} onchange={onPlanChange}>
      <option value="none">— 한도 미설정 —</option>
      {#each CLAUDE_PLANS as p}
        <option value={p.label}>{p.label}</option>
      {/each}
      <option value="custom">직접 입력…</option>
    </select>

    {#if editingCustom}
      <input
        class="custom-input"
        type="text"
        bind:value={customValue}
        onkeydown={onCustomKey}
        onblur={commitCustom}
        placeholder="토큰 수 (예: 500000)"
        autofocus
      />
    {/if}
  </div>

  <QuotaBar
    tokens_5h={agent.tokens_5h}
    quota_limit={effectiveLimit}
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
  .proj { margin-bottom: 6px; }
  .subtle { color: #8e8e93; font-size: 11px; }

  .plan-row {
    display: flex; align-items: center; gap: 6px;
    margin-bottom: 8px;
  }
  .plan-sel {
    background: #1c1c1e; border: 1px solid #3a3a3c;
    border-radius: 5px; color: #8e8e93;
    font-size: 10px; padding: 2px 6px;
    outline: none; cursor: pointer;
    flex-shrink: 0;
  }
  .plan-sel:focus { border-color: #0a84ff; }
  .custom-input {
    background: #1c1c1e; border: 1px solid #0a84ff;
    border-radius: 4px; color: #f2f2f7;
    font-size: 11px; padding: 2px 6px;
    outline: none; width: 130px;
    font-variant-numeric: tabular-nums;
  }
</style>
