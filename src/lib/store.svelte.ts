import { listenSnapshot, type Snapshot } from "./tauri";

class SnapshotStore {
  snap = $state<Snapshot | null>(null);
  lastReceived = $state<number>(0);
  staleSeconds = $state<number>(0);

  async init() {
    const unlisten = await listenSnapshot((s) => {
      this.snap = s;
      this.lastReceived = Date.now();
    });
    setInterval(() => {
      this.staleSeconds = Math.floor((Date.now() - this.lastReceived) / 1000);
    }, 1000);
    return unlisten;
  }
}

export const store = new SnapshotStore();
