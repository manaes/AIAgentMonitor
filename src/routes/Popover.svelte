<script lang="ts">
  import { store } from "../lib/store.svelte";
  import AgentCard from "../components/AgentCard.svelte";
  import { invoke } from "@tauri-apps/api/core";

  async function openDetail() {
    await invoke("open_detail_window");
  }
</script>

<div class="popover-root">
  {#if store.snap}
    {#each store.snap.agents as agent (agent.kind)}
      <AgentCard {agent} />
    {/each}
    <button class="more" onclick={openDetail}>More details →</button>
    {#if store.staleSeconds > 10}
      <p class="warn">Backend not responding ({store.staleSeconds}s)</p>
    {/if}
  {:else}
    <p class="subtle">Waiting for snapshot…</p>
  {/if}
</div>

<style>
  .popover-root { width: 340px; }
  .more {
    width: 100%; margin-top: 8px; padding: 6px;
    background: transparent; border: 1px solid #3a3a3c;
    border-radius: 6px; color: #0a84ff; cursor: pointer; font-size: 11px;
  }
  .more:hover { background: #2c2c2e; }
  .subtle { color: #8e8e93; }
  .warn { color: #ff453a; font-size: 11px; margin: 8px 0 0; }
</style>
