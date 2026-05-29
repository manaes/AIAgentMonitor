<script lang="ts">
  import { formatTokensTotal, formatResetClock } from "../lib/format";
  import type { TokenCounts } from "../lib/tauri";

  let { tokens_5h, quota_limit, reset_at } = $props<{
    tokens_5h: TokenCounts;
    quota_limit: number | null;
    reset_at: { secs_since_epoch: number } | null;
  }>();

  let used = $derived(
    tokens_5h.tokens_in +
    tokens_5h.tokens_out +
    tokens_5h.tokens_cache_create +
    tokens_5h.tokens_cache_read
  );
  let remaining = $derived(quota_limit ? Math.max(0, quota_limit - used) : null);
  let pct = $derived(quota_limit ? Math.min(100, (used / quota_limit) * 100) : null);

  // 바 색상: 80% 이상이면 주황→빨강
  let barColor = $derived(
    pct === null ? "#30d158"
    : pct >= 90 ? "linear-gradient(90deg, #ff9f0a, #ff453a)"
    : pct >= 70 ? "linear-gradient(90deg, #30d158, #ff9f0a)"
    : "linear-gradient(90deg, #30d158, #34c759)"
  );
</script>

<div class="qb">
  {#if quota_limit}
    <!-- 잔량 / 전체 + 퍼센트 -->
    <div class="row">
      <span class="remain">
        <span class="val">{formatTokensTotal(remaining ?? 0)}</span>
        <span class="sep"> / </span>
        <span class="total">{formatTokensTotal(quota_limit)} 남음</span>
      </span>
      <span class="pct">{pct?.toFixed(0)}% 사용</span>
    </div>
    <div class="bar">
      <span class="fill" style="width:{pct}%; background:{barColor}"></span>
    </div>
    <div class="sub-row">
      <span class="used-label">사용: {formatTokensTotal(used)}</span>
      {#if reset_at}
        <span class="subtle">reset {formatResetClock(reset_at.secs_since_epoch)}</span>
      {/if}
    </div>
  {:else}
    <!-- 한도 미설정: 사용량만 표시 -->
    <div class="row">
      <span class="subtle">5h 사용: {formatTokensTotal(used)}</span>
      {#if reset_at}
        <span class="subtle">reset {formatResetClock(reset_at.secs_since_epoch)}</span>
      {/if}
    </div>
  {/if}
</div>

<style>
  .qb { font-size: 11px; font-variant-numeric: tabular-nums; }

  .row { display: flex; justify-content: space-between; align-items: baseline; margin-bottom: 5px; }
  .remain { display: flex; align-items: baseline; gap: 1px; }
  .val { color: #f2f2f7; font-size: 13px; font-weight: 600; }
  .sep, .total { color: #8e8e93; font-size: 10px; }
  .pct { color: #8e8e93; font-size: 10px; }

  .bar {
    height: 6px;
    background: #1c1c1e;
    border-radius: 3px;
    overflow: hidden;
    margin-bottom: 4px;
  }
  .fill {
    display: block; height: 100%;
    border-radius: 3px;
    transition: width 0.4s ease;
  }

  .sub-row { display: flex; justify-content: space-between; }
  .used-label { color: #636366; font-size: 10px; }
  .subtle { color: #8e8e93; font-size: 10px; }
</style>
