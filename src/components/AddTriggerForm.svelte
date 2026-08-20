<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import { store } from "../lib/store.svelte";

  let agent = $state<"claude" | "codex" | "antigravity">("claude");
  let timeValue = $state("08:00");
  let workingDir = $state("");
  let prompt = $state("");
  let submitting = $state(false);
  let errorMsg = $state("");

  async function pickFolder() {
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected === "string") {
      workingDir = selected;
    }
  }

  async function handleSubmit(e: SubmitEvent) {
    e.preventDefault();
    errorMsg = "";

    const parts = timeValue.split(":");
    const hour = parseInt(parts[0], 10);
    const minute = parseInt(parts[1], 10);

    if (isNaN(hour) || isNaN(minute)) {
      errorMsg = "시각 형식이 올바르지 않습니다.";
      return;
    }
    if (!workingDir.trim()) {
      errorMsg = "작업 디렉토리를 선택하거나 입력하세요.";
      return;
    }
    if (!prompt.trim()) {
      errorMsg = "프롬프트를 입력하세요.";
      return;
    }

    submitting = true;
    try {
      await store.addTrigger(agent, hour, minute, workingDir.trim(), prompt.trim());
      workingDir = "";
      prompt = "";
    } catch (err) {
      errorMsg = String(err);
    } finally {
      submitting = false;
    }
  }
</script>

<form class="form" onsubmit={handleSubmit}>
  <p class="label">새 트리거 추가</p>

  <!-- 1행: 에이전트 · 시간 · 경로 · 폴더 선택 -->
  <div class="row">
    <select class="sel" bind:value={agent}>
      <option value="claude">Claude</option>
      <option value="codex">Codex</option>
      <option value="antigravity">Antigravity</option>
    </select>

    <input
      class="input time-input"
      type="time"
      bind:value={timeValue}
      required
    />

    <input
      class="input dir-input"
      type="text"
      bind:value={workingDir}
      placeholder="~/workspace 또는 절대경로"
    />

    <button class="btn-dir" type="button" onclick={pickFolder} title="폴더 선택">
      📁
    </button>
  </div>

  <!-- 2행: 프롬프트 textarea + 추가 버튼 -->
  <div class="row row-prompt">
    <textarea
      class="input prompt-area"
      bind:value={prompt}
      placeholder="실행할 프롬프트 입력 (예: ping, summarize recent changes...)"
      rows={3}
      required
    ></textarea>
    <button class="btn-add" type="submit" disabled={submitting}>
      {submitting ? "…" : "추가"}
    </button>
  </div>

  {#if errorMsg}
    <p class="error">{errorMsg}</p>
  {/if}
  <p class="hint subtle">앱 실행 중일 때만 트리거가 동작합니다.</p>
</form>

<style>
  .form {
    background: #2c2c2e;
    border-radius: 8px;
    padding: 10px 12px;
    margin-top: 8px;
  }
  .label {
    font-size: 9px;
    color: #8e8e93;
    text-transform: uppercase;
    letter-spacing: 0.4px;
    margin: 0 0 6px;
  }
  .row {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .row-prompt {
    margin-top: 6px;
    align-items: flex-end;
  }
  .sel,
  .input {
    background: #1c1c1e;
    border: 1px solid #3a3a3c;
    border-radius: 5px;
    color: #f2f2f7;
    font-size: 11px;
    padding: 4px 7px;
    outline: none;
    font-family: inherit;
  }
  .sel:focus,
  .input:focus {
    border-color: #0a84ff;
  }
  .sel { flex-shrink: 0; }
  .time-input { width: 90px; flex-shrink: 0; }
  .dir-input { flex: 1; min-width: 0; }
  .prompt-area {
    flex: 1;
    resize: vertical;
    min-height: 52px;
    line-height: 1.5;
  }
  .btn-dir {
    flex-shrink: 0;
    background: #3a3a3c;
    border: none;
    border-radius: 5px;
    font-size: 14px;
    padding: 3px 8px;
    cursor: pointer;
    line-height: 1;
  }
  .btn-dir:hover { background: #48484a; }
  .btn-add {
    flex-shrink: 0;
    align-self: flex-end;
    background: #0a84ff;
    border: none;
    border-radius: 5px;
    color: #fff;
    font-size: 11px;
    font-weight: 600;
    padding: 6px 14px;
    cursor: pointer;
    height: 28px;
  }
  .btn-add:hover:not(:disabled) { background: #0070d8; }
  .btn-add:disabled { opacity: 0.5; cursor: default; }
  .error { font-size: 10px; color: #ff453a; margin: 4px 0 0; }
  .hint { font-size: 9px; margin: 6px 0 0; }
  .subtle { color: #8e8e93; }
</style>
