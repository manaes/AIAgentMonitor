<script lang="ts">
  import { store } from "../lib/store.svelte";
  import AgentCard from "../components/AgentCard.svelte";
  import SessionList from "../components/SessionList.svelte";
</script>

<div class="window-root">
  {#if store.snap}
    <div class="agents">
      {#each store.snap.agents as agent (agent.kind)}
        <AgentCard {agent} />
      {/each}
    </div>
    <div class="sessions">
      <SessionList snap={store.snap} />
    </div>
  {:else}
    <p class="subtle">Waiting for snapshot…</p>
  {/if}
</div>

<style>
  .window-root { display: flex; flex-direction: column; gap: 12px; }
  .agents { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
  .agents :global(.card + .card) { margin-top: 0; }
</style>
