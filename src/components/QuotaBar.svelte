<script lang="ts">
  import { formatTokensTotal, formatResetClock } from "../lib/format";
  import type { TokenCounts } from "../lib/tauri";

  let { tokens_5h, quota_limit, reset_at } = $props<{
    tokens_5h: TokenCounts;
    quota_limit: number | null;
    reset_at: { secs_since_epoch: number } | null;
  }>();

  let used = $derived(tokens_5h.tokens_in + tokens_5h.tokens_out + tokens_5h.tokens_cache_create + tokens_5h.tokens_cache_read);
  let pct = $derived(quota_limit ? Math.min(100, (used / quota_limit) * 100) : null);
</script>

<div class="qb">
  <div class="row">
    <span>{formatTokensTotal(used)} / 5h{quota_limit ? ` of ${formatTokensTotal(quota_limit)}` : ""}</span>
    {#if reset_at}
      <span class="subtle">reset {formatResetClock(reset_at.secs_since_epoch)}</span>
    {/if}
  </div>
  {#if pct !== null}
    <div class="bar"><span style="width:{pct}%"></span></div>
  {/if}
</div>

<style>
  .qb { font-size: 11px; font-variant-numeric: tabular-nums; }
  .row { display: flex; justify-content: space-between; color: #8e8e93; margin-bottom: 4px; }
  .bar { height: 4px; background: #2c2c2e; border-radius: 2px; overflow: hidden; }
  .bar > span { display: block; height: 100%; background: linear-gradient(90deg, #30d158, #ff9f0a); }
</style>
