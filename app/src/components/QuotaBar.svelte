<script lang="ts">
  import { formatTokensTotal } from "../lib/format";
  import type { TokenCounts } from "../lib/tauri";

  // auto_pct: 실제 5h 사용률(%), weekly_pct: 주간(7d) 사용률(%).
  // reset_5h: 5h 윈도우가 리셋된 직후면 true → 백엔드 갱신 전까지 5h 사용률을 0%로 표시.
  // unreadable: 사용량 조회가 실패 중이면 true → 숫자를 못 믿으므로 %를 지운다.
  //   마지막으로 받아둔 값이 남아 있어도 지금 상태를 말해주지 못하므로, 낡은 숫자를
  //   멀쩡한 척 보여주느니 안 보여주는 편이 정직하다(이유는 카드의 에러 배지가 말한다).
  //   로컬에서 직접 센 5h 토큰 수는 서버 한도가 아니라 계속 유효하므로 그건 남긴다.
  //
  // **행 두 개(5h·주간)와 막대 두 개는 어떤 상태에서도 항상 그린다.** 값이 없다고
  // 행을 빼면 에이전트마다 카드 높이가 달라져 목록이 들쭉날쭉해진다(2026-09-15).
  // 값이 없을 땐 막대를 0%로 비워 두고 오른쪽에 이유를 적는다.
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

  // 값이 하나라도 왔다면 조회 자체는 성공한 것이다 — 그런데도 비어 있는 창은
  // "아직 안 받아온" 게 아니라 **그 계정/플랜이 안 주는** 창이다. 실제로 Codex 는
  // 2026-09 업데이트 이후 요금제에 따라 5h(300분) 창을 아예 빼고 주간(10080분)만
  // 돌려준다(app-server `account/rateLimits/read` 실측). 그 차이를 구분해야
  // "동기화 전"과 "이 플랜엔 없음"이 같은 빈칸으로 보이지 않는다.
  let synced = $derived(pct !== null || wpct !== null);

  // 값이 없는 행에 적을 이유. 조회 실패(unreadable)도 결국 "지금은 못 보여준다"라
  // 같은 문구를 쓴다 — 구체적인 사유는 카드 위 ⚠ 배지가 따로 말해 준다.
  function note(synced: boolean): string {
    return synced || unreadable ? "지원하지 않음" : "동기화 전";
  }

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
  <div class="row">
    <span class="label">5h 한도</span>
    {#if pct !== null}
      <span class="pct" style="color:{pctColor(pct)}">{pct.toFixed(0)}% <span class="rem-hint">({(100 - pct).toFixed(0)}% 남음)</span></span>
    {:else}
      <!-- 한도는 없어도 우리가 직접 센 5h 토큰은 유효하다 — 같은 줄에 덧붙여
           높이를 그대로 두면서 정보를 잃지 않는다. -->
      <span class="na">{note(synced)} <span class="rem-hint">· {formatTokensTotal(localUsed)} tok</span></span>
    {/if}
  </div>
  <div class="bar" class:idle={pct === null}>
    <span class="fill" style="width:{pct ?? 0}%; background:{color(pct ?? 0)}"></span>
  </div>

  <div class="row wk">
    <span class="label">주간 한도</span>
    {#if wpct !== null}
      <span class="pct" style="color:{pctColor(wpct)}">{wpct.toFixed(0)}% <span class="rem-hint">({(100 - wpct).toFixed(0)}% 남음)</span></span>
    {:else}
      <span class="na">{note(synced)}</span>
    {/if}
  </div>
  <div class="bar" class:idle={wpct === null}>
    <span class="fill" style="width:{wpct ?? 0}%; background:{color(wpct ?? 0)}"></span>
  </div>
</div>

<style>
  .qb { font-size: 11px; font-variant-numeric: tabular-nums; }
  .row { display: flex; justify-content: space-between; align-items: baseline; margin-bottom: 3px; }
  .row.wk { margin-top: 6px; }
  .label { color: #8e8e93; font-size: 10px; }
  .pct { color: #30d158; font-size: 13px; font-weight: 700; }
  /* 값이 없는 행. %와 같은 자리를 차지하되(높이 유지) 숫자처럼 읽히지 않게 눌러 둔다. */
  .na { color: #636366; font-size: 11px; font-weight: 500; }
  .rem-hint { font-size: 10px; font-weight: 400; color: #8e8e93; }
  .bar { height: 6px; background: #1c1c1e; border-radius: 3px; overflow: hidden; margin-bottom: 2px; }
  /* 0%인 빈 막대가 "0% 사용 중"으로 읽히지 않도록 트랙을 한 단계 죽인다. */
  .bar.idle { background: #161618; opacity: 0.55; }
  .fill { display: block; height: 100%; border-radius: 3px; transition: width 0.4s ease; }
</style>
