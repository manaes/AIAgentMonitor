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

  // localStorage 키 — 에이전트별 사용자 설정 한도 (토큰 수)
  const STORAGE_KEY = `ai-monitor-quota-limit-${agent.kind}`;

  let localLimit = $state<number | null>(null);
  let editing = $state(false);
  let editValue = $state("");

  onMount(() => {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) localLimit = parseInt(raw, 10) || null;
  });

  // Rust가 quota_limit을 알면 우선 사용, 아니면 로컬 설정값
  let effectiveLimit = $derived(agent.quota_limit ?? localLimit);

  function startEdit() {
    editValue = effectiveLimit ? String(effectiveLimit) : "";
    editing = true;
  }

  function commitEdit() {
    editing = false;
    const val = parseInt(editValue.replace(/[^0-9]/g, ""), 10);
    if (val > 0) {
      localLimit = val;
      localStorage.setItem(STORAGE_KEY, String(val));
    } else {
      localLimit = null;
      localStorage.removeItem(STORAGE_KEY);
    }
  }

  function onEditKey(e: KeyboardEvent) {
    if (e.key === "Enter") commitEdit();
    if (e.key === "Escape") editing = false;
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

  <div class="proj subtle">
    {primaryProj?.name ?? "no active session"}
  </div>

  <!-- 한도 설정 버튼 -->
  <div class="limit-row">
    {#if editing}
      <input
        class="limit-input"
        type="text"
        bind:value={editValue}
        onkeydown={onEditKey}
        onblur={commitEdit}
        placeholder="ex) 500000"
        autofocus
      />
      <span class="hint">토큰 수 입력 후 Enter (비우면 삭제)</span>
    {:else}
      <button class="limit-btn" onclick={startEdit}>
        {effectiveLimit ? `한도: ${Number(effectiveLimit).toLocaleString()}` : "한도 설정…"}
      </button>
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
  .limit-row { margin-bottom: 6px; }
  .limit-btn {
    background: none; border: none;
    color: #636366; font-size: 10px;
    padding: 0; cursor: pointer;
    text-decoration: underline dotted;
  }
  .limit-btn:hover { color: #8e8e93; }
  .limit-input {
    background: #1c1c1e; border: 1px solid #0a84ff;
    border-radius: 4px; color: #f2f2f7;
    font-size: 11px; padding: 2px 6px;
    outline: none; width: 120px;
    font-variant-numeric: tabular-nums;
  }
  .hint { color: #636366; font-size: 9px; margin-left: 6px; }
</style>
