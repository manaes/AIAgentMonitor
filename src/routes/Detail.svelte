<script lang="ts">
  import { onMount } from "svelte";
  import { store } from "../lib/store.svelte";
  import AgentCard from "../components/AgentCard.svelte";
  import SessionList from "../components/SessionList.svelte";
  import TriggerList from "../components/TriggerList.svelte";
  import AddTriggerForm from "../components/AddTriggerForm.svelte";
  import DevicePanel from "../components/DevicePanel.svelte";

  let activeTab = $state<"sessions" | "triggers" | "devices">("sessions");

  onMount(() => {
    store.loadTriggers();
  });
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
    <button class="tab" class:active={activeTab === "triggers"} onclick={() => (activeTab = "triggers")}>
      Triggers
    </button>
    <button class="tab" class:active={activeTab === "devices"} onclick={() => (activeTab = "devices")}>
      Devices
    </button>
  </div>

  {#if activeTab === "sessions"}
    {#if store.snap}
      <div class="sessions">
        <SessionList snap={store.snap} />
      </div>
    {:else}
      <p class="subtle">Waiting for snapshot…</p>
    {/if}
  {:else if activeTab === "triggers"}
    <div class="triggers">
      <TriggerList />
      <AddTriggerForm />
    </div>
  {:else}
    <DevicePanel />
  {/if}
</div>

<style>
  .window-root { display: flex; flex-direction: column; gap: 12px; }
  .agents { display: grid; grid-template-columns: repeat(2, minmax(260px, 1fr)); gap: 8px; }
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
  .triggers { display: flex; flex-direction: column; gap: 0; }

  @media (max-width: 600px) {
    .agents {
      grid-template-columns: 1fr;
    }

    .agents :global(.card + .card) {
      margin-top: 0;
    }
  }
</style>
