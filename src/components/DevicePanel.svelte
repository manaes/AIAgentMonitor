<script lang="ts">
  import { onMount } from "svelte";
  import { store } from "../lib/store.svelte";

  onMount(() => {
    store.initBle();
  });

  let enabled = $derived(store.ble?.enabled ?? false);
  let peers = $derived(store.ble?.peers ?? []);
  // 토글 자체 실패(로컬)를 백엔드 last_error 보다 우선 표시한다 — 둘은 같은 오류 영역을 공유한다
  let shownError = $derived(store.bleActionError ?? store.ble?.last_error ?? null);
</script>

<div class="panel">
  <div class="row">
    <div class="text">
      <strong>BLE 공유</strong>
      <span class="subtle">iPhone 등 클라이언트에 모니터링 화면을 전송합니다</span>
    </div>
    <button
      class="toggle"
      class:on={enabled}
      onclick={() => store.setBleEnabled(!enabled)}
    >
      {enabled ? "켜짐" : "꺼짐"}
    </button>
  </div>

  {#if shownError}
    <p class="error">{shownError}</p>
  {/if}

  {#if enabled}
    <p class="warn">
      1단계에는 기기 인증이 없습니다. 주변의 누구나 연결할 수 있으니 필요할 때만 켜세요.
    </p>
    <p class="subtle status">
      {store.ble?.advertising ? "광고 중 · AIM-*" : "광고 시작 대기 중…"}
    </p>
  {/if}

  <p class="label">연결된 기기</p>
  {#each peers as peer (peer.id)}
    <div class="peer">
      <span class="dot"></span>
      <span class="pid">{peer.id}</span>
      <span class="subtle">MTU {peer.mtu}</span>
    </div>
  {:else}
    <p class="subtle">연결된 기기가 없습니다.</p>
  {/each}
</div>

<style>
  .panel { background: #2c2c2e; border-radius: 8px; padding: 10px 12px; }
  .row { display: flex; justify-content: space-between; align-items: center; gap: 12px; }
  .text { display: flex; flex-direction: column; gap: 2px; }
  .text strong { font-size: 12px; color: #f2f2f7; }
  .toggle {
    background: #3a3a3c; border: none; border-radius: 12px; color: #8e8e93;
    cursor: pointer; font-size: 11px; font-weight: 600; padding: 4px 14px;
  }
  .toggle.on { background: #0a84ff; color: #fff; }
  .warn { color: #ff9f0a; font-size: 10px; margin: 8px 0 0; }
  .error {
    color: #ff453a; font-size: 11px; line-height: 1.4; margin: 8px 0 0;
    background: #3a2a2a; border-radius: 6px; padding: 6px 8px;
  }
  .status { margin: 4px 0 0; }
  .label {
    font-size: 9px; color: #8e8e93; text-transform: uppercase;
    letter-spacing: 0.4px; margin: 12px 0 6px;
  }
  .peer { display: flex; align-items: center; gap: 6px; font-size: 11px; padding: 4px 0; }
  .peer + .peer { border-top: 1px solid #3a3a3c; }
  .dot { width: 6px; height: 6px; border-radius: 50%; background: #30d158; }
  .pid { font-family: ui-monospace, monospace; font-size: 10px; color: #f2f2f7; }
  .subtle { color: #8e8e93; font-size: 10px; }
</style>
