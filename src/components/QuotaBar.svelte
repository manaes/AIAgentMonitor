<script lang="ts">
  import { formatTokensTotal } from "../lib/format";
  import type { TokenCounts } from "../lib/tauri";

  // auto_pct: 프록시가 헤더에서 읽은 실제 5h 사용률(%). 동기화 전이면 null.
  let { tokens_5h, auto_pct = null }: { tokens_5h: TokenCounts; auto_pct?: number | null } = $props();

  let localUsed = $derived(tokens_5h.tokens_in + tokens_5h.tokens_out);
  let pct = $derived(auto_pct !== null ? Math.min(100, auto_pct) : null);

  let barColor = $derived(
    pct === null ? "#30d158"
    : pct >= 90 ? "linear-gradient(90deg, #ff9f0a, #ff453a)"
    : pct >= 70 ? "linear-gradient(90deg, #30d158, #ff9f0a)"
    : "linear-gradient(90deg, #30d158, #34c759)"
  );
</script>

<div class="qb">
  {#if pct !== null}
    <div class="row">
      <span class="subtle">5h 사용량</span>
      <span class="pct">{pct.toFixed(0)}%</span>
    </div>
    <div class="bar">
      <span class="fill" style="width:{pct}%; background:{barColor}"></span>
    </div>
  {:else}
    <div class="row">
      <span class="subtle">input+output (5h): {formatTokensTotal(localUsed)}</span>
      <span class="subtle hint">· 동기화 전</span>
    </div>
  {/if}
</div>

<style>
  .qb { font-size: 11px; font-variant-numeric: tabular-nums; }
  .row { display: flex; justify-content: space-between; align-items: baseline; margin-bottom: 5px; }
  .pct { color: #30d158; font-size: 13px; font-weight: 700; }
  .bar { height: 6px; background: #1c1c1e; border-radius: 3px; overflow: hidden; margin-bottom: 2px; }
  .fill { display: block; height: 100%; border-radius: 3px; transition: width 0.4s ease; }
  .subtle { color: #8e8e93; font-size: 10px; }
  .hint { color: #636366; }
</style>
