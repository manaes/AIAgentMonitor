<script lang="ts">
  import { onMount } from "svelte";
  import { store } from "../lib/store.svelte";
  import type { AgentKind } from "../lib/tauri";

  onMount(() => {
    store.initSettings();
  });

  // AgentCard.svelte 가 쓰는 표시 이름과 맞춘다 — 같은 종류가 화면마다
  // 다른 이름으로 보이면 안 된다.
  const AGENT_LABELS: Record<AgentKind, string> = {
    claude: "Claude Code",
    codex: "Codex",
    antigravity: "Antigravity",
  };
  const ALL_KINDS: AgentKind[] = ["claude", "codex", "antigravity"];

  let enabled = $derived(new Set(store.settings?.enabled_agents ?? ALL_KINDS));
  let antigravityPollInterval = $derived(store.settings?.antigravity_poll_interval_secs ?? 300);

  function toggle(kind: AgentKind, checked: boolean) {
    const next = new Set(enabled);
    if (checked) next.add(kind);
    else next.delete(kind);
    // 워처는 계속 돈다 — 여기서 켜고 끄는 건 다음 틱부터 화면(과 iOS 미러)에
    // 무엇을 내보낼지일 뿐이다. 그래서 전부 꺼도 데이터 자체가 사라지지
    // 않고, 다시 켜면 끊김 없이 바로 보인다.
    store.setEnabledAgents(ALL_KINDS.filter((k) => next.has(k)));
  }

  function setAntigravityPollInterval(seconds: number) {
    store.setAntigravityPollInterval(seconds);
  }
</script>

<div class="panel">
  <p class="label">표시할 에이전트</p>
  <span class="subtle">체크 해제해도 백그라운드 수집은 계속됩니다 — 화면에서만 숨겨집니다</span>

  {#if store.settingsActionError}
    <p class="error">{store.settingsActionError}</p>
  {/if}

  <ul class="agent-list">
    {#each ALL_KINDS as kind (kind)}
      <li class="row">
        <label class="check-row">
          <input
            type="checkbox"
            checked={enabled.has(kind)}
            onchange={(e) => toggle(kind, e.currentTarget.checked)}
          />
          <span>{AGENT_LABELS[kind]}</span>
        </label>
      </li>
    {/each}
  </ul>

  <div class="polling">
    <p class="label">Antigravity 갱신 주기</p>
    <div class="poll-row">
      <span class="subtle"><code>agy -p /usage</code> 자동 조회</span>
      <select
        value={antigravityPollInterval}
        onchange={(e) => setAntigravityPollInterval(Number(e.currentTarget.value))}
        aria-label="Antigravity 갱신 주기"
      >
        <option value={60}>1분</option>
        <option value={300}>5분</option>
        <option value={600}>10분</option>
        <option value={900}>15분</option>
        <option value={1800}>30분</option>
        <option value={3600}>1시간</option>
      </select>
    </div>
  </div>
</div>

<style>
  .panel { background: #2c2c2e; border-radius: 8px; padding: 10px 12px; }
  .label {
    font-size: 9px; color: #8e8e93; text-transform: uppercase;
    letter-spacing: 0.4px; margin: 0 0 4px;
  }
  .subtle { color: #8e8e93; font-size: 10px; }
  .error {
    color: #ff453a; font-size: 11px; line-height: 1.4; margin: 8px 0 0;
    background: #3a2a2a; border-radius: 6px; padding: 6px 8px;
  }
  .agent-list { list-style: none; margin: 10px 0 0; padding: 0; }
  .row { padding: 4px 0; }
  .row + .row { border-top: 1px solid #3a3a3c; }
  .check-row {
    display: flex; align-items: center; gap: 8px; cursor: pointer;
    font-size: 12px; color: #f2f2f7;
  }
  .check-row input { accent-color: #0a84ff; }
  .polling { border-top: 1px solid #3a3a3c; margin-top: 10px; padding-top: 10px; }
  .poll-row { display: flex; align-items: center; justify-content: space-between; gap: 10px; }
  code { font-size: 9px; }
  select {
    background: #3a3a3c; border: 1px solid #48484a; border-radius: 5px;
    color: #f2f2f7; font-size: 11px; padding: 3px 5px;
  }
</style>
