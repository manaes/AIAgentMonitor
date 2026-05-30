<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { store } from "./lib/store.svelte";
  import Popover from "./routes/Popover.svelte";
  import Detail from "./routes/Detail.svelte";
  import "./app.css";

  let mode = $state<"popover" | "detail" | "loading">("loading");

  // onMount는 동기로 둔다 — async onMount의 cleanup을 Svelte가 무시하기 때문.
  onMount(() => {
    const w = getCurrentWindow();
    mode = w.label === "detail" ? "detail" : "popover";
    store.init();
    return () => store.dispose();
  });
</script>

{#if mode === "popover"}
  <Popover />
{:else if mode === "detail"}
  <Detail />
{/if}
