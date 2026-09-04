<script lang="ts">
  import { formatTokensTotal } from "../lib/format";
  import type { TokenCounts } from "../lib/tauri";

  // auto_pct: 실제 5h 사용률(%), weekly_pct: 주간(7d) 사용률(%). 둘 다 동기화 전이면 null.
  // reset_5h: 5h 윈도우가 리셋된 직후면 true → 백엔드 갱신 전까지 5h 사용률을 0%로 표시.
  // unreadable: 사용량 조회가 실패 중이면 true → %와 막대를 아예 숨긴다. 마지막으로
  //   받아둔 값이 남아 있어도 지금 상태를 말해주지 못하므로, 낡은 숫자를 멀쩡한 척
  //   보여주느니 안 보여주는 편이 정직하다(이유는 카드의 에러 배지가 말한다).
  //   로컬에서 직접 센 5h 토큰 수는 서버 한도가 아니라 계속 유효하므로 그건 남긴다.
  let { tokens_5h, auto_pct = null, weekly_pct = null, reset_5h = false, unreadable = false }: {
    tokens_5h: TokenCounts;
    auto_pct?: number | null;
    weekly_pct?: number | null;
    reset_5h?: boolean;
    unreadable?: boolean;
  } = $props();

  let localUsed = $derived(tokens_5h.tokens_in + tokens_5h.tokens_out);
  let pct = $derived(
    unreadable ? null : reset_5h ? 0 : auto_pct !== null ? Math.min(100, auto_pct) : null
  );
  let wpct = $derived(
    unreadable ? null : weekly_pct !== null ? Math.min(100, weekly_pct) : null
  );

  function color(p: number): string {
    return p >= 90 ? "linear-gradient(90deg, #ff9f0a, #ff453a)"
      : p >= 70 ? "linear-gradient(90deg, #30d158, #ff9f0a)"
      : "linear-gradient(90deg, #30d158, #34c759)";
  }

  function pctColor(p: number): string {
    return p >= 90 ? "#ff453a" : p >= 70 ? "#ff9f0a" : "#30d158";
  }
</script>

<div class="qb">
  {#if pct !== null || wpct !== null}
    {#if pct !== null}
      <div class="row">
        <span class="label">5h 한도</span>
        <span class="pct" style="color:{pctColor(pct)}">{pct.toFixed(0)}% <span class="rem-hint">({(100 - pct).toFixed(0)}% 남음)</span></span>
      </div>
      <div class="bar"><span class="fill" style="width:{pct}%; background:{color(pct)}"></span></div>
    {:else}
      <div class="row">
        <span class="label">5h 사용량</span>
        <span class="subtle">{formatTokensTotal(localUsed)}</span>
      </div>
    {/if}
    {#if wpct !== null}
      <div class="row" class:wk={pct !== null || localUsed > 0}>
        <span class="label">주간 한도</span>
        <span class="pct" style="color:{pctColor(wpct)}">{wpct.toFixed(0)}% <span class="rem-hint">({(100 - wpct).toFixed(0)}% 남음)</span></span>
      </div>
      <div class="bar"><span class="fill" style="width:{wpct}%; background:{color(wpct)}"></span></div>
    {/if}
  {:else}
    <div class="row">
      <span class="subtle">5h 토큰: {formatTokensTotal(localUsed)}</span>
      <span class="subtle hint">{unreadable ? "· 한도 조회 실패" : "· 동기화 전"}</span>
    </div>
  {/if}
</div>

<style>
  .qb { font-size: 11px; font-variant-numeric: tabular-nums; }
  .row { display: flex; justify-content: space-between; align-items: baseline; margin-bottom: 3px; }
  .row.wk { margin-top: 6px; }
  .label { color: #8e8e93; font-size: 10px; }
  .pct { color: #30d158; font-size: 13px; font-weight: 700; }
  .rem-hint { font-size: 10px; font-weight: 400; color: #8e8e93; }
  .bar { height: 6px; background: #1c1c1e; border-radius: 3px; overflow: hidden; margin-bottom: 2px; }
  .fill { display: block; height: 100%; border-radius: 3px; transition: width 0.4s ease; }
  .subtle { color: #8e8e93; font-size: 10px; }
  .hint { color: #636366; }
</style>
