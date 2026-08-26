<script lang="ts">
  import { onMount } from "svelte";
  import { store } from "../lib/store.svelte";
  import AgentCard from "../components/AgentCard.svelte";
  import SessionList from "../components/SessionList.svelte";
  import DevicePanel from "../components/DevicePanel.svelte";
  import SettingsPanel from "../components/SettingsPanel.svelte";

  let activeTab = $state<"sessions" | "devices" | "settings">("sessions");

  onMount(() => {
    // Devices 탭 노출 여부가 **세 전송 모두**의 supported 에 달려 있으므로 패널을
    // 열기 전에 상태를 받아둔다. 하나라도 빠뜨리면 그 전송만 켤 수 있는 상황에서
    // 탭이 나타나지 않는다. init* 은 멱등하므로 DevicePanel 의 onMount 와 중복
    // 호출되어도 안전하다.
    store.initBle();
    store.initNetwork();
    store.initLan();
  });

  // macOS 외 빌드에는 실제 BLE 구현이 없다(FakePeripheral). 토글이 성공을 보고하면서
  // 아무 일도 일어나지 않는 상태를 보여주지 않도록 BLE 만 보고 감추면 안 된다 —
  // 네트워크(iroh)와 LAN 은 크로스플랫폼이라 Windows 에서도 이 탭이 있어야 한다.
  //
  // **지원되는 전송을 하나도 빠뜨리지 않는다.** 오늘 `NETWORK_SUPPORTED` 가 무조건
  // true 라서 이 식은 늘 참이지만, 그것에 얹혀 있으면 네트워크가 `cfg` 로 갈리는 날
  // 백엔드는 LAN 을 켤 수 있다고 말하는데 사용자에게는 그것을 켤 탭이 없다.
  let bleSupported = $derived(store.ble?.supported ?? false);
  let networkSupported = $derived(store.network?.supported ?? false);
  let lanSupported = $derived(store.lan?.supported ?? false);
  let devicesTabSupported = $derived(bleSupported || networkSupported || lanSupported);
</script>

<div class="window-root">
  {#if store.snap}
    <div class="agents">
      {#each store.snap.agents as agent (agent.kind)}
        <AgentCard {agent} />
      {/each}
    </div>
  {:else}
    <p class="subtle">Waiting for snapshot…</p>
  {/if}

  <!-- 탭 바 -->
  <div class="tab-bar">
    <button class="tab" class:active={activeTab === "sessions"} onclick={() => (activeTab = "sessions")}>
      Sessions
    </button>
    {#if devicesTabSupported}
      <button class="tab" class:active={activeTab === "devices"} onclick={() => (activeTab = "devices")}>
        Devices
      </button>
    {/if}
    <button class="tab" class:active={activeTab === "settings"} onclick={() => (activeTab = "settings")}>
      설정
    </button>
  </div>

  {#if activeTab === "devices" && devicesTabSupported}
    <DevicePanel />
  {:else if activeTab === "settings"}
    <SettingsPanel />
  {:else}
    <!-- 탭이 감춰진 뒤에도 devices 가 남아 있을 수 있으므로 sessions 를 폴백으로 둔다 -->
    {#if store.snap}
      <div class="sessions">
        <SessionList snap={store.snap} />
      </div>
    {:else}
      <p class="subtle">Waiting for snapshot…</p>
    {/if}
  {/if}
</div>

<style>
  .window-root { display: flex; flex-direction: column; gap: 12px; }
  /* 칸 수를 3으로 고정하면 설정 탭에서 에이전트를 2개만 켰을 때 세 번째 칸이
     빈 여백으로 남는다. auto-fit 은 실제 카드 수만큼만 칸을 만들어 남는 폭을
     카드들이 나눠 갖는다. */
  .agents { display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 8px; }
  .agents :global(.card + .card) { margin-top: 0; }

  .tab-bar {
    display: flex;
    gap: 2px;
    border-bottom: 1px solid #3a3a3c;
    padding-bottom: 0;
  }
  .tab {
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    color: #8e8e93;
    cursor: pointer;
    font-size: 12px;
    font-weight: 500;
    padding: 4px 12px 6px;
    margin-bottom: -1px;
  }
  .tab.active {
    border-bottom-color: #0a84ff;
    color: #f2f2f7;
  }
  .tab:hover:not(.active) {
    color: #c7c7cc;
  }
  @media (max-width: 720px) {
    .agents {
      grid-template-columns: 1fr;
    }

    .agents :global(.card + .card) {
      margin-top: 0;
    }
  }
</style>
