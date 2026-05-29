<script lang="ts">
  import { store } from "../lib/store.svelte";
  import type { TriggerRule } from "../lib/tauri";

  // cron "0 MM HH * * *" → { hour, minute }
  function parseCron(cron: string): { hour: number; minute: number } | null {
    const parts = cron.trim().split(" ");
    if (parts.length < 3) return null;
    const minute = parseInt(parts[1], 10);
    const hour = parseInt(parts[2], 10);
    if (isNaN(hour) || isNaN(minute)) return null;
    return { hour, minute };
  }

  // 다음 실행 예정 시각: 오늘 HH:MM이 이미 지났으면 "내일 HH:MM"
  function nextRunLabel(cron: string): string {
    const parsed = parseCron(cron);
    if (!parsed) return "—";
    const { hour, minute } = parsed;
    const hh = String(hour).padStart(2, "0");
    const mm = String(minute).padStart(2, "0");
    const now = new Date();
    const target = new Date();
    target.setHours(hour, minute, 0, 0);
    const prefix = target <= now ? "내일" : "오늘";
    return `${prefix} ${hh}:${mm}`;
  }

  // cron에서 HH:MM 표시용 문자열 추출
  function cronToHHMM(cron: string): string {
    const parsed = parseCron(cron);
    if (!parsed) return "??:??";
    return `${String(parsed.hour).padStart(2, "0")}:${String(parsed.minute).padStart(2, "0")}`;
  }

  let toggling = $state<Set<string>>(new Set());
  let firing = $state<Set<string>>(new Set());

  async function handleToggle(rule: TriggerRule) {
    toggling = new Set([...toggling, rule.id]);
    try {
      await store.toggleTrigger(rule.id);
    } finally {
      toggling = new Set([...toggling].filter((id) => id !== rule.id));
    }
  }

  async function handleRemove(rule: TriggerRule) {
    await store.removeTrigger(rule.id);
  }

  async function handleFireNow(rule: TriggerRule) {
    firing = new Set([...firing, rule.id]);
    try {
      await store.fireNow(rule.id);
    } finally {
      firing = new Set([...firing].filter((id) => id !== rule.id));
    }
  }
</script>

<div class="list">
  <p class="label">Anchor Triggers · 매일 지정 시각에 자동 실행</p>
  {#if store.triggers.length === 0}
    <p class="subtle empty">등록된 트리거가 없습니다. 아래에서 추가하세요.</p>
  {/if}
  {#each store.triggers as rule (rule.id)}
    <div class="row" class:disabled={!rule.enabled}>
      <span class="toggle-wrap">
        <input
          type="checkbox"
          class="toggle"
          checked={rule.enabled}
          disabled={toggling.has(rule.id)}
          onchange={() => handleToggle(rule)}
        />
      </span>
      <span class="badge" class:claude={rule.agent === "claude"} class:codex={rule.agent === "codex"}>
        {rule.agent === "claude" ? "Claude" : "Codex"}
      </span>
      <span class="time">{cronToHHMM(rule.cron)} daily</span>
      <span class="dir subtle" title={rule.working_dir}>{rule.working_dir}</span>
      <span class="prompt subtle" title={rule.prompt}>{rule.prompt}</span>
      <span class="next subtle">{rule.enabled ? nextRunLabel(rule.cron) : "비활성"}</span>
      <span class="actions">
        <button
          class="btn-fire"
          disabled={firing.has(rule.id)}
          onclick={() => handleFireNow(rule)}
          title="지금 실행"
        >
          {firing.has(rule.id) ? "…" : "▶"}
        </button>
        <button
          class="btn-del"
          onclick={() => handleRemove(rule)}
          title="삭제"
        >
          ✕
        </button>
      </span>
    </div>
  {/each}
</div>

<style>
  .list {
    background: #2c2c2e;
    border-radius: 8px;
    padding: 10px 12px;
  }
  .label {
    font-size: 9px;
    color: #8e8e93;
    text-transform: uppercase;
    letter-spacing: 0.4px;
    margin: 0 0 6px;
  }
  .empty {
    padding: 8px 0;
    font-size: 11px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 0;
    font-size: 11px;
  }
  .row + .row {
    border-top: 1px solid #3a3a3c;
  }
  .row.disabled {
    opacity: 0.45;
  }
  .toggle-wrap {
    flex-shrink: 0;
  }
  .toggle {
    cursor: pointer;
    accent-color: #30d158;
  }
  .badge {
    flex-shrink: 0;
    font-size: 10px;
    font-weight: 600;
    padding: 1px 6px;
    border-radius: 4px;
  }
  .badge.claude {
    background: #1a3a2a;
    color: #30d158;
  }
  .badge.codex {
    background: #2a2a1a;
    color: #ff9f0a;
  }
  .time {
    flex-shrink: 0;
    font-variant-numeric: tabular-nums;
    font-weight: 600;
    color: #f2f2f7;
    min-width: 70px;
  }
  .dir {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .prompt {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
    font-style: italic;
  }
  .next {
    flex-shrink: 0;
    min-width: 80px;
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .actions {
    flex-shrink: 0;
    display: flex;
    gap: 4px;
  }
  .btn-fire,
  .btn-del {
    background: none;
    border: none;
    cursor: pointer;
    font-size: 11px;
    padding: 2px 5px;
    border-radius: 4px;
    color: #8e8e93;
  }
  .btn-fire:hover:not(:disabled) {
    background: #1a3a2a;
    color: #30d158;
  }
  .btn-del:hover {
    background: #3a1a1a;
    color: #ff453a;
  }
  .btn-fire:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .subtle {
    color: #8e8e93;
  }
</style>
