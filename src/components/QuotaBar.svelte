<script lang="ts">
  import { formatTokensTotal } from "../lib/format";
  import type { TokenCounts } from "../lib/tauri";

  // auto_pct: 실제 5h 사용률(%), weekly_pct: 주간(7d) 사용률(%). 둘 다 동기화 전이면 null.
  let { tokens_5h, auto_pct = null, weekly_pct = null }: {
    tokens_5h: TokenCounts;
    auto_pct?: number | null;
    weekly_pct?: number | null;
  } = $props();

  let localUsed = $derived(tokens_5h.tokens_in + tokens_5h.tokens_out);
  let pct = $derived(auto_pct !== null ? Math.min(100, auto_pct) : null);
  let wpct = $derived(weekly_pct !== null ? Math.min(100, weekly_pct) : null);

  function color(p: number): string {
    return p >= 90 ? "linear-gradient(90deg, #ff9f0a, #ff453a)"
      : p >= 70 ? "linear-gradient(90deg, #30d158, #ff9f0a)"
      : "linear-gradient(90deg, #30d158, #34c759)";
  }
</script>

<div class="qb">
  {#if pct !== null}
    <div class="row">
      <span class="label">5h</span>
      <span class="pct">{pct.toFixed(0)}%</span>
    </div>
    <div class="bar"><span class="fill" style="width:{pct}%; background:{color(pct)}"></span></div>
    {#if wpct !== null}
      <div class="row wk">
        <span class="label">주간</span>
        <span class="pct">{wpct.toFixed(0)}%</span>
      </div>
      <div class="bar"><span class="fill" style="width:{wpct}%; background:{color(wpct)}"></span></div>
    {/if}
  {:else}
    <div class="row">
      <span class="subtle">5h 토큰: {formatTokensTotal(localUsed)}</span>
      <span class="subtle hint">· 동기화 전</span>
    </div>
  {/if}
</div>

<style>
  .qb { font-size: 11px; font-variant-numeric: tabular-nums; }
  .row { display: flex; justify-content: space-between; align-items: baseline; margin-bottom: 3px; }
  .row.wk { margin-top: 6px; }
  .label { color: #8e8e93; font-size: 10px; }
  .pct { color: #30d158; font-size: 13px; font-weight: 700; }
  .bar { height: 6px; background: #1c1c1e; border-radius: 3px; overflow: hidden; margin-bottom: 2px; }
  .fill { display: block; height: 100%; border-radius: 3px; transition: width 0.4s ease; }
  .subtle { color: #8e8e93; font-size: 10px; }
  .hint { color: #636366; }
</style>
