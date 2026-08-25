<script lang="ts">
  import { onMount } from "svelte";
  import QRCode from "qrcode";
  import { store } from "../lib/store.svelte";

  onMount(() => {
    store.initBle();
    store.initNetwork();
    store.initPairing();
  });

  // 초 단위 카운트다운 갱신. *_status 이벤트는 활동이 있을 때만 발행되므로
  // (전체 브랜치 리뷰 I-1), 백엔드가 준 절대 만료 시각(expires_at)을 이 로컬
  // 시계로 재계산해야 근처에 기기가 없어도 카운트다운이 멈추지 않는다.
  // AgentCard.svelte 의 quota_reset_at 카운트다운과 같은 패턴이다.
  let nowSecs = $state(Math.floor(Date.now() / 1000));
  onMount(() => {
    const tick = setInterval(() => { nowSecs = Math.floor(Date.now() / 1000); }, 1_000);
    return () => clearInterval(tick);
  });

  // BLE 와 네트워크는 이제 **독립 토글**이다(2026-08-25 스펙 7장) — 페어링 창을
  // 공유하므로 예전의 "둘 중 하나만" 제약이 필요 없어졌다. 폰 A 는 BLE 로,
  // 폰 B 는 네트워크로 동시에 붙을 수 있다.
  let bleSupported = $derived(store.ble?.supported ?? false);
  let networkSupported = $derived(store.network?.supported ?? false);
  let bleEnabled = $derived(store.ble?.enabled ?? false);
  let networkEnabled = $derived(store.network?.enabled ?? false);
  let anyEnabled = $derived(bleEnabled || networkEnabled);

  let shownError = $derived(
    store.bleActionError ??
      store.networkActionError ??
      store.pairingActionError ??
      store.ble?.last_error ??
      store.network?.last_error ??
      null
  );

  // ── 공유 페어링 창 ──
  let backendWindow = $derived(store.pairing?.pairing_window ?? { kind: "closed" as const });
  // 백엔드가 "열림" 이라고 말해도 로컬 시계로 만료됐으면 닫힌 것으로 본다 —
  // pairing_status 는 push 되지 않으므로 이게 없으면 만료된 코드가 계속 남는다.
  let pairingWindow = $derived.by(() => {
    const w = backendWindow;
    if (w.kind === "open" && w.expires_at - nowSecs <= 0) return { kind: "closed" as const };
    return w;
  });
  let secondsLeft = $derived(
    pairingWindow.kind === "open" ? Math.max(0, pairingWindow.expires_at - nowSecs) : 0
  );
  let pairedPeers = $derived(store.pairing?.paired_peers ?? []);

  // QR 은 네트워크가 켜져 있을 때만 만들어진다(백엔드가 null 로 준다).
  let qrDataUrl = $state<string | null>(null);
  $effect(() => {
    const payload = pairingWindow.kind === "open" ? store.qrPayload : null;
    if (!payload) {
      qrDataUrl = null;
      return;
    }
    QRCode.toDataURL(payload, { margin: 1, width: 160 })
      .then((url) => { qrDataUrl = url; })
      .catch(() => { qrDataUrl = null; });
  });

  function formatPairedAt(secs: number): string {
    return new Date(secs * 1000).toLocaleString();
  }
</script>

<div class="panel">
  {#if bleSupported}
    <div class="row">
      <div class="text">
        <strong>BLE 공유</strong>
        <span class="subtle">가까이 있는 기기에 블루투스로 전송합니다</span>
      </div>
      <button class="toggle" class:on={bleEnabled} onclick={() => store.setBleEnabled(!bleEnabled)}>
        {bleEnabled ? "켜짐" : "꺼짐"}
      </button>
    </div>
  {/if}

  {#if networkSupported}
    <div class="row" style="margin-top: 8px;">
      <div class="text">
        <strong>네트워크 공유</strong>
        <span class="subtle">떨어져 있는 기기에 인터넷으로 전송합니다</span>
      </div>
      <button
        class="toggle"
        class:on={networkEnabled}
        onclick={() => store.setNetworkEnabled(!networkEnabled)}
      >
        {networkEnabled ? "켜짐" : "꺼짐"}
      </button>
    </div>
  {/if}

  {#if shownError}
    <p class="error">{shownError}</p>
  {/if}

  {#if bleEnabled}
    <p class="subtle status">
      {store.ble?.advertising ? "BLE 광고 중 · AIM-*" : "BLE 광고 시작 대기 중…"}
    </p>
  {/if}

  <!-- 페어링 영역은 하나다 — 창·코드·시도 예산을 두 전송이 공유한다. -->
  {#if anyEnabled}
    {#if pairingWindow.kind !== "open"}
      <button class="inline-btn" onclick={() => store.beginPairing()}>페어링 시작</button>
    {/if}

    {#if pairingWindow.kind === "open"}
      <div class="code-box">
        <p class="code-label">
          {secondsLeft}초 남음 · 시도 {pairingWindow.attempts_left}회 남음
        </p>
        {#if bleEnabled}
          <p class="code-label">BLE 로 붙일 기기에는 아래 6자리를 입력하세요</p>
          <p class="code">{pairingWindow.code}</p>
        {/if}
        {#if networkEnabled && qrDataUrl}
          <div class="qr-box">
            <p class="code-label">네트워크로 붙일 기기에서는 아래 QR 을 스캔하세요</p>
            <img class="qr-image" src={qrDataUrl} alt="네트워크 페어링 QR 코드" />
          </div>
        {/if}
      </div>
    {:else if pairingWindow.kind === "exhausted"}
      <p class="warn">
        시도 5회가 모두 틀렸습니다. 근처의 다른 기기가 코드를 추측했을 수 있습니다.
        다시 시작하려면 [페어링 시작] 을 누르세요.
      </p>
    {/if}
  {/if}

  <!-- 기기 목록도 하나다 — 어느 전송으로 페어링했든 같은 저장소에 들어간다. -->
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

  {#if bleEnabled}
    <p class="label">BLE 연결</p>
    {#each store.ble?.peers ?? [] as peer (peer.id)}
      <div class="peer">
        <span class="dot"></span>
        <span class="pid">{peer.id}</span>
        <span class="subtle">MTU {peer.mtu}</span>
      </div>
    {:else}
      <p class="subtle">연결된 기기가 없습니다.</p>
    {/each}
  {/if}
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
  .qr-box { display: flex; flex-direction: column; align-items: center; text-align: center; }
  .qr-image { border-radius: 4px; margin: 4px 0; background: #fff; padding: 6px; }

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
