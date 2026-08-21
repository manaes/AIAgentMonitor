<script lang="ts">
  import { onMount } from "svelte";
  import QRCode from "qrcode";
  import { store } from "../lib/store.svelte";

  onMount(() => {
    store.initBle();
    store.initNetwork();
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

  let bleSupported = $derived(store.ble?.supported ?? false);
  let networkSupported = $derived(store.network?.supported ?? false);
  let bleEnabled = $derived(store.ble?.enabled ?? false);
  let networkEnabled = $derived(store.network?.enabled ?? false);
  let anyEnabled = $derived(bleEnabled || networkEnabled);

  // 공유를 켤 때 BLE/네트워크 중 하나를 고른다 — 이미 켜진 뒤에는 바꿀 수
  // 없다(끄고 다시 골라야 한다), 두 전송을 동시에 켜면 페어링 창이 두
  // 화면으로 나뉘어 "창은 하나" 라는 전제(스펙 5.1)가 전송별로 깨진다.
  // 사용자가 고른 값은 그대로 두고(selectedMode), 실제 반영값(mode)만
  // bleSupported 로 걸러낸다 — 상태 로딩 중(초기 bleSupported=false) 잠깐의
  // 값으로 selectedMode 를 영구히 덮어써버리면 macOS 에서도 BLE 를 다시
  // 고를 수 없게 된다.
  let selectedMode = $state<"ble" | "network">("ble");
  let mode = $derived(bleSupported ? selectedMode : "network");
  $effect(() => {
    if (bleEnabled) selectedMode = "ble";
    else if (networkEnabled) selectedMode = "network";
  });

  function toggleShare() {
    if (bleEnabled) {
      store.setBleEnabled(false);
    } else if (networkEnabled) {
      store.setNetworkEnabled(false);
    } else if (mode === "ble") {
      store.setBleEnabled(true);
    } else {
      store.setNetworkEnabled(true);
    }
  }

  // ── BLE 페어링 창(기존 그대로) ──
  let blePeers = $derived(store.ble?.peers ?? []);
  let blePairedPeers = $derived(store.ble?.paired_peers ?? []);
  let bleBackendPairingWindow = $derived(store.ble?.pairing_window ?? { kind: "closed" as const });
  let blePairingWindow = $derived.by(() => {
    const w = bleBackendPairingWindow;
    if (w.kind === "open" && w.expires_at - nowSecs <= 0) {
      return { kind: "closed" as const };
    }
    return w;
  });
  let blePairingSecondsLeft = $derived(
    blePairingWindow.kind === "open" ? Math.max(0, blePairingWindow.expires_at - nowSecs) : 0
  );
  let bleShownError = $derived(store.bleActionError ?? store.ble?.last_error ?? null);

  // ── 네트워크 페어링 창(같은 패턴 + QR) ──
  let networkPairedPeers = $derived(store.network?.paired_peers ?? []);
  let networkBackendPairingWindow = $derived(store.network?.pairing_window ?? { kind: "closed" as const });
  let networkPairingWindow = $derived.by(() => {
    const w = networkBackendPairingWindow;
    if (w.kind === "open" && w.expires_at - nowSecs <= 0) {
      return { kind: "closed" as const };
    }
    return w;
  });
  let networkPairingSecondsLeft = $derived(
    networkPairingWindow.kind === "open" ? Math.max(0, networkPairingWindow.expires_at - nowSecs) : 0
  );
  let networkShownError = $derived(store.networkActionError ?? store.network?.last_error ?? null);

  let qrDataUrl = $state<string | null>(null);
  $effect(() => {
    const payload = networkPairingWindow.kind === "open" ? store.networkQrPayload : null;
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
  <div class="row">
    <div class="text">
      <strong>공유</strong>
      <span class="subtle">iPhone 등 클라이언트에 모니터링 화면을 전송합니다</span>
    </div>
    <button class="toggle" class:on={anyEnabled} onclick={toggleShare}>
      {anyEnabled ? "켜짐" : "꺼짐"}
    </button>
  </div>

  {#if !anyEnabled && bleSupported && networkSupported}
    <div class="mode-select">
      <button class="mode-btn" class:active={selectedMode === "ble"} onclick={() => (selectedMode = "ble")}>
        BLE
      </button>
      <button class="mode-btn" class:active={selectedMode === "network"} onclick={() => (selectedMode = "network")}>
        네트워크
      </button>
    </div>
  {/if}

  {#if mode === "ble"}
    {#if bleShownError}
      <p class="error">{bleShownError}</p>
    {/if}

    {#if bleEnabled}
      <p class="subtle status">
        {store.ble?.advertising ? "광고 중 · AIM-*" : "광고 시작 대기 중…"}
      </p>

      {#if blePairingWindow.kind !== "open"}
        <button class="inline-btn" onclick={() => store.beginPairing()}>페어링 시작</button>
      {/if}

      {#if blePairingWindow.kind === "open"}
        <div class="code-box">
          <p class="code-label">iPhone 에 아래 6자리를 입력하세요 ({blePairingSecondsLeft}초 남음)</p>
          <p class="code">{blePairingWindow.code}</p>
          <p class="subtle">시도 {blePairingWindow.attempts_left}회 남음</p>
        </div>
      {:else if blePairingWindow.kind === "exhausted"}
        <p class="warn">
          시도 5회가 모두 틀렸습니다. 근처의 다른 기기가 코드를 추측했을 수 있습니다.
          다시 시작하려면 [페어링 시작] 을 누르세요.
        </p>
      {/if}
    {/if}

    {#if blePairedPeers.length > 0}
      <p class="label">페어링된 기기</p>
      <ul class="peer-list">
        {#each blePairedPeers as peer (peer.peer_id)}
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
    {#each blePeers as peer (peer.id)}
      <div class="peer">
        <span class="dot"></span>
        <span class="pid">{peer.id}</span>
        <span class="subtle">MTU {peer.mtu}</span>
      </div>
    {:else}
      <p class="subtle">연결된 기기가 없습니다.</p>
    {/each}
  {:else}
    {#if networkShownError}
      <p class="error">{networkShownError}</p>
    {/if}

    {#if networkEnabled}
      {#if networkPairingWindow.kind !== "open"}
        <button class="inline-btn" onclick={() => store.beginNetworkPairing()}>페어링 시작</button>
      {/if}

      {#if networkPairingWindow.kind === "open"}
        <div class="code-box qr-box">
          <p class="code-label">iPhone 에서 스캔하세요 ({networkPairingSecondsLeft}초 남음)</p>
          {#if qrDataUrl}
            <img class="qr-image" src={qrDataUrl} alt="페어링 QR 코드" width="160" height="160" />
          {/if}
          <p class="subtle">시도 {networkPairingWindow.attempts_left}회 남음</p>
        </div>
      {:else if networkPairingWindow.kind === "exhausted"}
        <p class="warn">
          시도 5회가 모두 틀렸습니다. 다시 시작하려면 [페어링 시작] 을 누르세요.
        </p>
      {/if}
    {/if}

    {#if networkPairedPeers.length > 0}
      <p class="label">페어링된 기기</p>
      <ul class="peer-list">
        {#each networkPairedPeers as peer (peer.peer_id)}
          <li class="row">
            <span class="mono">{peer.peer_id}</span>
            <span class="subtle">{formatPairedAt(peer.paired_at)}</span>
            {#if peer.connected}<span class="dot-live">연결됨</span>{/if}
            <button class="inline-btn" onclick={() => store.unpairNetwork(peer.peer_id)}>해제</button>
          </li>
        {/each}
      </ul>
      <button class="inline-btn" onclick={() => store.unpairAllNetwork()}>전체 해제</button>
    {:else if networkEnabled}
      <p class="subtle status">아직 페어링된 기기가 없습니다.</p>
    {/if}
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

  .mode-select {
    display: flex; gap: 4px; margin-top: 8px; background: #1c1c1e;
    border-radius: 6px; padding: 2px;
  }
  .mode-btn {
    flex: 1; background: none; border: none; border-radius: 5px; color: #8e8e93;
    cursor: pointer; font-size: 10px; font-weight: 600; padding: 5px 0;
  }
  .mode-btn.active { background: #3a3a3c; color: #f2f2f7; }

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
