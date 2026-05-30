<script lang="ts">
  import { formatTokensTotal } from "../lib/format";
  import type { TokenCounts } from "../lib/tauri";

  let {
    tokens_5h, quota_limit,
    manual_pct = null,
    baseline_tokens = null,
    manual_reset = "",
    auto_pct = null
  }: {
    tokens_5h: TokenCounts;
    quota_limit: number | null;
    manual_pct?: number | null;
    baseline_tokens?: number | null;
    manual_reset?: string;
    auto_pct?: number | null;
  } = $props();

  // 로컬 계산 (input + output만)
  let localUsed = $derived(tokens_5h.tokens_in + tokens_5h.tokens_out);

  // 실효 %: 수동 입력 % + 입력 이후 로컬 delta (k=1, 단순 합산)
  //
  // Anthropic 서버 계산 공식이 비공개라 보정 계수는 불안정 → 제거.
  // 대신 delta를 보수적으로 합산하고, 주기적으로 사용자가 % 재입력.
  let pct = $derived(
    auto_pct !== null
      ? Math.min(100, auto_pct)
      : manual_pct !== null
      ? (() => {
          const delta = baseline_tokens !== null
            ? Math.max(0, localUsed - baseline_tokens)
            : 0;
          const deltaPct = (quota_limit && delta > 0)
            ? (delta / quota_limit) * 100
            : 0;
          return Math.min(100, manual_pct + deltaPct);
        })()
      : (quota_limit ? Math.min(100, (localUsed / quota_limit) * 100) : null)
  );

  // 표시용 사용량: pct 기반 (한도 있을 때) 또는 로컬
  let usedDisplay = $derived(
    pct !== null && quota_limit
      ? Math.round(quota_limit * pct / 100)
      : localUsed
  );
  let remaining = $derived(
    quota_limit ? Math.max(0, quota_limit - usedDisplay) : null
  );

  // 리셋 시각 레이블 (카운트다운이 AgentCard에 있으므로 여기선 생략)
  let resetLabel = $derived<string | null>(null);

  // 바 색상
  let barColor = $derived(
    pct === null ? "#30d158"
    : pct >= 90 ? "linear-gradient(90deg, #ff9f0a, #ff453a)"
    : pct >= 70 ? "linear-gradient(90deg, #30d158, #ff9f0a)"
    : "linear-gradient(90deg, #30d158, #34c759)"
  );

  // 수동 입력 여부 표시
  let isManual = $derived(manual_pct !== null || !!manual_reset);
  let isAuto = $derived(auto_pct !== null);
</script>

<div class="qb">
  {#if pct !== null}
    <!-- 한도 설정 또는 수동 % 입력 시 -->
    <div class="row">
      {#if quota_limit}
        <span class="usage-text">
          <span class="val">{formatTokensTotal(usedDisplay)}</span>
          <span class="sep"> / </span>
          <span class="total">{formatTokensTotal(quota_limit)}</span>
        </span>
      {:else}
        <!-- 한도 미설정, % 만 수동 입력된 경우 -->
        <span class="subtle">사용 {pct.toFixed(0)}%</span>
      {/if}
      <span class="pct" class:manual={isManual} class:auto={isAuto}>
        {pct.toFixed(0)}%{isAuto || isManual ? "" : " ~"}
      </span>
    </div>
    <div class="bar">
      <span class="fill" style="width:{pct}%; background:{barColor}"></span>
    </div>
    <div class="sub-row">
      {#if quota_limit}
        <span class="remain">남음: {formatTokensTotal(remaining ?? 0)}</span>
      {:else}
        <span class="remain">&nbsp;</span>
      {/if}
      {#if resetLabel}
        <span class="subtle">reset {resetLabel}</span>
      {/if}
    </div>
  {:else}
    <!-- 한도도 수동%도 없음 -->
    <div class="row">
      <span class="subtle">input+output (5h): {formatTokensTotal(localUsed)}</span>
      {#if resetLabel}
        <span class="subtle">reset {resetLabel}</span>
      {/if}
    </div>
  {/if}
</div>

<style>
  .qb { font-size: 11px; font-variant-numeric: tabular-nums; }
  .row { display: flex; justify-content: space-between; align-items: baseline; margin-bottom: 5px; }
  .usage-text { display: flex; align-items: baseline; gap: 2px; }
  .val { color: #f2f2f7; font-size: 13px; font-weight: 600; }
  .sep, .total { color: #8e8e93; font-size: 11px; }
  .pct { color: #8e8e93; font-size: 10px; }
  .pct.manual { color: #0a84ff; }   /* 수동 입력 시 파란색으로 구분 */
  .pct.auto { color: #30d158; }     /* 프록시 실측값 — 초록색 */
  .bar { height: 6px; background: #1c1c1e; border-radius: 3px; overflow: hidden; margin-bottom: 4px; }
  .fill { display: block; height: 100%; border-radius: 3px; transition: width 0.4s ease; }
  .sub-row { display: flex; justify-content: space-between; }
  .remain { color: #636366; font-size: 10px; }
  .subtle { color: #8e8e93; font-size: 10px; }
</style>
