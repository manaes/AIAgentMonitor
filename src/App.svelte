<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { store } from "./lib/store.svelte";
  import Popover from "./routes/Popover.svelte";
  import Detail from "./routes/Detail.svelte";
  import "./app.css";

  let mode = $state<"popover" | "detail" | "loading">("loading");

  onMount(async () => {
    const w = getCurrentWindow();
    mode = w.label === "detail" ? "detail" : "popover";
    await store.init();
  });
</script>

{#if mode === "popover"}
  <Popover />
{:else if mode === "detail"}
  <Detail />
{/if}
