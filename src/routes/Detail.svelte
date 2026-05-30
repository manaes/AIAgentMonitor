<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { store } from "../lib/store.svelte";
  import AgentCard from "../components/AgentCard.svelte";
  import SessionList from "../components/SessionList.svelte";
  import TriggerList from "../components/TriggerList.svelte";
  import AddTriggerForm from "../components/AddTriggerForm.svelte";

  let activeTab = $state<"sessions" | "triggers">("sessions");
  let otelPortBound = $state(false);
  let otelDataReceived = $state(false);
  let showOtelHelp = $state(false);

  // onMount는 동기로 둔다 — async onMount가 반환하는 cleanup은 Svelte가 무시하므로 타이머가 누수된다.
  onMount(() => {
    store.loadTriggers();
    const check = async () => {
      const s = await invoke<{ port_bound: boolean; data_received: boolean }>("otel_status");
      otelPortBound = s.port_bound;
      otelDataReceived = s.data_received;
    };
    check();
    const timer = setInterval(check, 3000);
    return () => clearInterval(timer);
  });
</script>

<div class="window-root">
  {#if store.snap}
    <div class="agents">
      {#each store.snap.agents as agent (agent.kind)}
        <AgentCard {agent} />
      {/each}
    </div>
  {:else}
    <p class="subtle">Waiting for snapshot…</p>
  {/if}

  <!-- OTEL 상태 배지 -->
  <div class="otel-row">
    <button
      class="otel-badge"
      class:active={otelDataReceived}
      class:waiting={otelPortBound && !otelDataReceived}
      onclick={() => showOtelHelp = !showOtelHelp}
    >
      {#if otelDataReceived}
        ● OTEL 수신 중
      {:else if otelPortBound}
        ◎ OTEL 대기 중 (포트 4318)
      {:else}
        ○ OTEL 비활성
      {/if}
    </button>
    {#if showOtelHelp}
      <div class="otel-help">
        {#if !otelPortBound}
          <p class="hint warn">⚠ 포트 4318 바인딩 실패 — 앱 재시작 필요</p>
        {:else if otelDataReceived}
          <p class="hint ok">● OTEL 정상 수신 중. claude 실행 후 10초 이내 갱신.</p>
        {:else}
          <p class="hint">~/.claude/settings.json에 자동 추가됨. 새 claude 세션을 시작하세요.</p>
          <p class="hint">또는 현재 터미널에서 수동 적용:</p>
        {/if}
        {#if !otelDataReceived}
          <code>export CLAUDE_CODE_ENABLE_TELEMETRY=1
export OTEL_METRICS_EXPORTER=otlp
export OTEL_LOGS_EXPORTER=otlp
export OTEL_EXPORTER_OTLP_PROTOCOL=http/json
export OTEL_EXPORTER_OTLP_ENDPOINT="http://localhost:4318"
export OTEL_METRIC_EXPORT_INTERVAL=10000</code>
        {/if}
      </div>
    {/if}
  </div>

  <!-- 탭 바 -->
  <div class="tab-bar">
    <button
      class="tab"
      class:active={activeTab === "sessions"}
      onclick={() => (activeTab = "sessions")}
    >
      Sessions
    </button>
    <button
      class="tab"
      class:active={activeTab === "triggers"}
      onclick={() => (activeTab = "triggers")}
    >
      Triggers
    </button>
  </div>

  <!-- 탭 컨텐츠 -->
  {#if activeTab === "sessions"}
    {#if store.snap}
      <div class="sessions">
        <SessionList snap={store.snap} />
      </div>
    {:else}
      <p class="subtle">Waiting for snapshot…</p>
    {/if}
  {:else}
    <div class="triggers">
      <TriggerList />
      <AddTriggerForm />
    </div>
  {/if}
</div>

<style>
  .window-root { display: flex; flex-direction: column; gap: 12px; }
  .agents { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
  .agents :global(.card + .card) { margin-top: 0; }

  .tab-bar {
    display: flex;
    gap: 2px;
    border-bottom: 1px solid #3a3a3c;
    padding-bottom: 0;
  }
  .tab {
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    color: #8e8e93;
    cursor: pointer;
    font-size: 12px;
    font-weight: 500;
    padding: 4px 12px 6px;
    margin-bottom: -1px;
  }
  .tab.active {
    border-bottom-color: #0a84ff;
    color: #f2f2f7;
  }
  .tab:hover:not(.active) {
    color: #c7c7cc;
  }
  .triggers { display: flex; flex-direction: column; gap: 0; }

  .otel-row { display: flex; flex-direction: column; gap: 6px; }
  .otel-badge {
    align-self: flex-start;
    background: none; border: 1px solid #3a3a3c;
    border-radius: 4px; color: #636366; cursor: pointer;
    font-size: 10px; padding: 2px 8px;
  }
  .otel-badge.active   { border-color: #30d158; color: #30d158; }
  .otel-badge.waiting  { border-color: #ff9f0a; color: #ff9f0a; }
  .otel-badge:hover { border-color: #8e8e93; }
  .otel-help {
    background: #1c1c1e; border: 1px solid #3a3a3c; border-radius: 6px;
    padding: 8px 10px;
  }
  .otel-help .hint { color: #8e8e93; font-size: 10px; margin: 0 0 4px; }
  .otel-help .hint.warn { color: #ff453a; }
  .otel-help .hint.ok { color: #30d158; }
  .otel-help code {
    display: block; white-space: pre;
    color: #30d158; font-size: 10px; font-family: ui-monospace, monospace;
    line-height: 1.6;
  }
</style>
