<script lang="ts">
  import { store } from "../lib/store.svelte";
  import AgentCard from "../components/AgentCard.svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";

  let rootEl = $state<HTMLElement | null>(null);

  async function openDetail() {
    // popover 닫기는 백엔드(open_detail_window)가 처리한다
    await invoke("open_detail_window");
  }

  // 콘텐츠 크기에 맞춰 popover 창을 리사이즈 → 스크롤 없이 전체가 보이도록 한다.
  $effect(() => {
    const el = rootEl;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      const r = el.getBoundingClientRect();
      getCurrentWindow().setSize(new LogicalSize(Math.ceil(r.width), Math.ceil(r.height)));
    });
    ro.observe(el);
    return () => ro.disconnect();
  });
</script>

<div class="popover-root" bind:this={rootEl}>
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
