<script lang="ts">
  import { onMount } from "svelte";
  import { store } from "../lib/store.svelte";

  onMount(() => {
    store.initBle();
  });

  let enabled = $derived(store.ble?.enabled ?? false);
  let peers = $derived(store.ble?.peers ?? []);
  let pairedPeers = $derived(store.ble?.paired_peers ?? []);
  let pairingWindow = $derived(store.ble?.pairing_window ?? { kind: "closed" as const });
  // 토글 자체 실패(로컬)를 백엔드 last_error 보다 우선 표시한다 — 둘은 같은 오류 영역을 공유한다
  let shownError = $derived(store.bleActionError ?? store.ble?.last_error ?? null);

  function formatPairedAt(secs: number): string {
    return new Date(secs * 1000).toLocaleString();
  }
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
    <p class="subtle status">
      {store.ble?.advertising ? "광고 중 · AIM-*" : "광고 시작 대기 중…"}
    </p>

    {#if pairingWindow.kind !== "open"}
      <button class="inline-btn" onclick={() => store.beginPairing()}>페어링 시작</button>
    {/if}

    {#if pairingWindow.kind === "open"}
      <div class="code-box">
        <p class="code-label">iPhone 에 아래 6자리를 입력하세요 ({pairingWindow.seconds_left}초 남음)</p>
        <p class="code">{pairingWindow.code}</p>
        <p class="subtle">시도 {pairingWindow.attempts_left}회 남음</p>
      </div>
    {:else if pairingWindow.kind === "exhausted"}
      <p class="warn">
        시도 5회가 모두 틀렸습니다. 근처의 다른 기기가 코드를 추측했을 수 있습니다.
        다시 시작하려면 [페어링 시작] 을 누르세요.
      </p>
    {/if}
  {/if}

  {#if pairedPeers.length > 0}
    <p class="label">페어링된 기기</p>
    <ul class="peer-list">
      {#each pairedPeers as peer (peer.peer_id)}
        <li class="row">
          <span class="mono">{peer.peer_id}</span>
          <span class="subtle">{formatPairedAt(peer.paired_at)}</span>
          {#if peer.connected}<span class="dot-live">연결됨</span>{/if}
          <button class="inline-btn" onclick={() => store.unpair(peer.peer_id)}>해제</button>
        </li>
      {/each}
    </ul>
    <button class="inline-btn" onclick={() => store.unpairAll()}>전체 해제</button>
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

  .inline-btn {
    background: #3a3a3c; border: none; border-radius: 6px; color: #f2f2f7;
    cursor: pointer; font-size: 10px; font-weight: 600; padding: 4px 10px;
    margin-top: 8px;
  }
  .inline-btn:hover { background: #48484a; }

  .code-box {
    background: #1c1c1e; border-radius: 6px; padding: 8px 10px; margin-top: 8px;
  }
  .code-label { font-size: 10px; color: #8e8e93; margin: 0 0 4px; }
  .code {
    font-family: ui-monospace, monospace; font-size: 22px; font-weight: 700;
    color: #f2f2f7; letter-spacing: 4px; margin: 0 0 4px;
  }

  .peer-list { list-style: none; margin: 0; padding: 0; }
  .peer-list li.row {
    justify-content: flex-start; font-size: 11px; padding: 4px 0; gap: 8px;
  }
  .peer-list li.row + li.row { border-top: 1px solid #3a3a3c; }
  .mono { font-family: ui-monospace, monospace; font-size: 10px; color: #f2f2f7; }
  .dot-live {
    color: #30d158; font-size: 9px; font-weight: 600;
    background: rgba(48, 209, 88, 0.15); border-radius: 4px; padding: 1px 6px;
  }
  .peer-list .inline-btn { margin-top: 0; margin-left: auto; }
</style>
