// 이 파일의 표시 규칙은 iPhone 미러(Swift `MirrorFormat`)로도 포팅돼 있다.
// 두 화면을 나란히 놓았을 때 같은 숫자가 나와야 하므로, 아래 함수를 고치면
// **반드시** 다음을 돌려서 골든 표와 복제본이 따라오게 할 것:
//
//   docs/ble-protocol/golden/check-parity-drift.sh
//
// 그 스크립트가 (1) docs/ble-protocol/golden/generate-format-parity.mjs 안의
// 복제본이 이 파일과 같은지, (2) 골든 표 format-parity.json 이 최신인지를 본다.
// 돌리지 않으면 Swift 쪽 테스트는 낡은 표를 기준으로 그대로 통과한다.

export function formatTokensPerSec(v: number): string {
  if (v < 1) return "0";
  if (v < 1000) return v.toFixed(0);
  return (v / 1000).toFixed(1) + "k";
}

export function formatTokensTotal(n: number): string {
  if (n < 1000) return n.toString();
  if (n < 1_000_000) return (n / 1000).toFixed(1) + "k";
  return (n / 1_000_000).toFixed(2) + "M";
}

export function relativeTime(secsSinceEpoch: number): string {
  const elapsed = Math.floor(Date.now() / 1000) - secsSinceEpoch;
  if (elapsed < 5) return "방금 전";
  if (elapsed < 60) return `${elapsed}초 전`;
  if (elapsed < 3600) return `${Math.floor(elapsed / 60)}분 전`;
  return `${Math.floor(elapsed / 3600)}시간 전`;
}

export function formatResetClock(secsSinceEpoch: number): string {
  const d = new Date(secsSinceEpoch * 1000);
  return d.toTimeString().slice(0, 5);
}
