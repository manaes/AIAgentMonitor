<script lang="ts">
  import { onMount } from "svelte";
  import QRCode from "qrcode";
  import { store } from "../lib/store.svelte";

  onMount(() => {
    store.initBle();
    store.initNetwork();
    store.initLan();
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

  // BLE·네트워크·LAN 은 **독립 토글**이다(2026-08-25 스펙 7장) — 페어링 창을
  // 공유하므로 예전의 "둘 중 하나만" 제약이 필요 없어졌다. 폰 A 는 BLE 로,
  // 폰 B 는 네트워크로, 전용 기기는 LAN 으로 동시에 붙을 수 있다.
  //
  // 탭은 지원되는 전송이 **하나라도** 있으면 보이고(`Detail.svelte`), 그 안에서
  // 토글은 전송마다 자기 `supported` 로 정한다 — 프론트는 OS 를 추측하지 않는다.
  // BLE 만 macOS 전용이고(CoreBluetooth), 네트워크와 LAN 은 어디서나 true 다.
  // 그래서 이 탭은 윈도우에서도 보이고, 거기서는 그 두 토글만 보인다.
  let bleSupported = $derived(store.ble?.supported ?? false);
  let networkSupported = $derived(store.network?.supported ?? false);
  let lanSupported = $derived(store.lan?.supported ?? false);
  let bleEnabled = $derived(store.ble?.enabled ?? false);
  let networkEnabled = $derived(store.network?.enabled ?? false);
  let lanEnabled = $derived(store.lan?.enabled ?? false);
  let anyEnabled = $derived(bleEnabled || networkEnabled || lanEnabled);

  // 백엔드가 포트까지 붙여 준 완성된 문자열이다(`192.168.0.12:4320`). 여기서
  // 포트를 덧붙이지 않는다 — 4320 은 Rust 의 `lan::server::PORT` 한 곳에만 있고,
  // 그 값을 프론트가 베껴 적으면 상수가 움직이는 날 이 패널이 거짓말을 한다.
  let lanAddress = $derived(store.lan?.address ?? null);

  // 6자리 코드를 **손으로 입력하는** 전송들. 네트워크는 QR 로 대신하고, LAN 기기
  // (CYD)는 카메라가 없어 BLE 와 같은 길을 쓴다. 코드 자체는 창 하나에서 나오므로
  // 화면에도 한 번만 찍고 문구만 켜져 있는 전송에 맞춘다.
  //
  // LAN 단독 문구가 **주소가 있을 때만** 「위 주소를」이라고 말한다. 주소가 없으면
  // 위에 있는 것은 주소가 아니라 「찾지 못했습니다」이거나 빨간 오류이고, 그 상태에서
  // 「위 주소를 넣고」라고 하면 사용자는 존재하지 않는 값을 찾는다.
  let codeTyped = $derived(bleEnabled || lanEnabled);
  let codeLabel = $derived(
    bleEnabled && lanEnabled
      ? "BLE·LAN 으로 붙일 기기에는 아래 6자리를 입력하세요"
      : bleEnabled
        ? "BLE 로 붙일 기기에는 아래 6자리를 입력하세요"
        : lanAddress
          ? "LAN 으로 붙일 기기에는 위 주소를 넣고 아래 6자리를 입력하세요"
          : "LAN 으로 붙일 기기에는 아래 6자리를 입력하세요"
  );

  // 이 전송의 실패만 따로 본다. 화면의 빨간 줄(`shownError`)은 세 전송 중 하나만
  // 보여주므로, "LAN 주소가 없는 이유를 우리가 이미 알고 있는가"를 그것으로 판단하면
  // 남의 전송 오류에 끌려간다.
  let lanError = $derived(store.lanActionError ?? store.lan?.last_error ?? null);

  let shownError = $derived(
    store.bleActionError ??
      store.networkActionError ??
      store.lanActionError ??
      store.pairingActionError ??
      store.ble?.last_error ??
      store.network?.last_error ??
      store.lan?.last_error ??
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
    const payload = pairingWindow.kind === "open" ? (store.pairing?.qr_payload ?? null) : null;
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

  {#if lanSupported}
    <div class="row" style="margin-top: 8px;">
      <div class="text">
        <strong>LAN 공유</strong>
        <!-- 「WiFi」로만 쓰면 랜선으로 붙은 사용자가 이 기능이 자기에게 해당하지
             않는다고 읽는다. 리스너는 인터페이스를 가리지 않는다.

             뒷문장은 **노출**을 말한다. 앞문장만 있으면 「전용 기기에만 간다」로
             읽히는데, 실제로 켜지는 것은 같은 망의 누구나 두드릴 수 있는 4320
             포트이고(미인증 상대는 데이터를 한 바이트도 못 받지만 포트는 열려
             있다), mDNS 는 이 맥의 이름을 망 전체에 광고한다(`AIM-<호스트>`,
             `<호스트>.local.`). 카페·호텔 WiFi 에서 이 토글을 누르는 사람이
             그 사실을 알 수 있는 자리는 여기뿐이다. -->
        <span class="subtle">
          같은 망(WiFi·유선)의 전용 기기에 전송합니다 · 켜는 동안 이 맥의 이름이
          같은 망에 광고되고 4320 포트가 열립니다
        </span>
      </div>
      <button class="toggle" class:on={lanEnabled} onclick={() => store.setLanEnabled(!lanEnabled)}>
        {lanEnabled ? "켜짐" : "꺼짐"}
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

  <!--
    LAN 주소는 **리스너가 서 있는 동안 언제나** 보인다 — 페어링 창이 열려 있든
    아니든, 자동 검색이 되든 안 되든.

    자동 검색(mDNS)이 막힌 망에서 무슨 일이 벌어지는지가 이 배치의 이유다:
    방화벽이 5353 을 막았거나 게스트 VLAN 이면 게시는 **실패한 채 아무 신호도
    남기지 않는다**(`lan/discovery.rs` 모듈 doc 에 크레이트 소스 위치까지 적혀
    있다). 그러니 "못 찾으면 주소를 넣으세요"를 오류 표시에 매달면 가장 필요한
    순간에 아무 말도 하지 않게 된다. 문구도 같은 이유로 "찾지 못하면 알려
    준다"고 말하지 않는다 — 우리는 그것을 알지 못한다.

    **다만 "켜져 있는 동안"이 아니라 "리스너가 서 있는 동안"이다.** 백엔드가
    `address` 를 리스너에 매달아 준다(`lib.rs` 의 `lan_address`) — bind 에
    실패하면 토글은 켜진 채로 주소가 `null` 이 되므로, 이 패널이 열려 있지 않은
    포트를 손으로 넣으라고 안내하는 일이 없다.
  -->
  {#if lanEnabled}
    {#if lanAddress}
      <p class="subtle status">
        LAN 주소 <span class="mono">{lanAddress}</span> · 기기가 자동으로 찾지 못하면 이 주소를
        직접 넣으세요
      </p>
    {:else if !lanError}
      <!-- 주소가 없고 할 말도 없다면 기본 경로가 없다는 뜻이다(`local_ipv4`) —
           랜선이 빠졌거나 어느 망에도 붙어 있지 않다.

           LAN 오류가 떠 있을 때는 이 줄을 내보내지 않는다. bind 실패도 주소를
           없애는데(위 주석), 그때 「WiFi·유선 연결을 확인하세요」라고 말하면
           멀쩡한 망을 탓하는 **틀린 진단**이 된다. 그 경우에 무엇이 잘못됐는지는
           빨간 줄이나 바로 아래 갈래가 말한다. -->
      <p class="subtle status">LAN 주소를 찾지 못했습니다 — WiFi·유선 연결을 확인하세요</p>
    {/if}
    <!-- **빨간 줄이 LAN 얘기를 하고 있지 않은 경우.** `shownError` 는 세 전송 중
         하나만 고정 우선순위로 보여주고 LAN 은 뒤쪽이라, 예컨대 BLE 권한 거부와
         4320 점유가 겹치면 빨간 줄은 BLE 것이 된다. 그러면 위 두 갈래는 둘 다
         침묵하므로(주소는 없고 `lanError` 는 있다) LAN 은 「켜짐」 토글만 남는다 —
         사용자가 켠 전송이 실패했으면 그 전송에 대해 뭔가는 읽을 수 있어야 한다.

         주소 유무와 무관하게 건다. 리스너는 떴는데 게시 시작에 실패한 경우처럼
         주소가 있는 채로 오류만 있는 갈래도 있기 때문이다. -->
    {#if lanError && lanError !== shownError}
      <p class="subtle status">LAN 공유: {lanError}</p>
    {/if}
  {/if}

  <!-- 페어링 영역은 하나다 — 창·코드·시도 예산을 세 전송이 공유한다. -->
  {#if anyEnabled}
    {#if pairingWindow.kind !== "open"}
      <button class="inline-btn" onclick={() => store.beginPairing()}>페어링 시작</button>
    {/if}

    {#if pairingWindow.kind === "open"}
      <div class="code-box">
        <p class="code-label">
          {secondsLeft}초 남음 · 시도 {pairingWindow.attempts_left}회 남음
        </p>
        <!-- 코드는 창 하나에서 나오므로 화면에도 한 번만 찍는다 — BLE 와 LAN 이
             같은 6자리를 쓴다. LAN 기기는 카메라가 없어 QR 대신 이 길이다. -->
        {#if codeTyped}
          <p class="code-label">{codeLabel}</p>
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
