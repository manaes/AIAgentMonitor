// MirrorFormat(Swift)와 src/lib/format.ts(JS)가 항상 같은 문자열을 내야 하므로,
// 두 런타임의 실제 반올림 동작을 직접 비교할 골든 벡터 테이블을 생성한다.
//
// 아래 두 함수는 ../../../src/lib/format.ts 의 formatTokensPerSec / formatTokensTotal
// 을 그대로 복제한 것이다(원본을 import 하지 않는 이유: 이 스크립트는 Node 에서
// 한 번 실행해 format-parity.json 을 만들기 위한 것일 뿐이고, TS 빌드 파이프라인에
// 얹지 않기 위해서다). format.ts 가 바뀌면 이 복제본도 같이 고쳐야 한다.
//
// 실행: node generate-format-parity.mjs > format-parity.json

function formatTokensPerSec(v) {
  if (v < 1) return "0";
  if (v < 1000) return v.toFixed(0);
  return (v / 1000).toFixed(1) + "k";
}

function formatTokensTotal(n) {
  if (n < 1000) return n.toString();
  if (n < 1_000_000) return (n / 1000).toFixed(1) + "k";
  return (n / 1_000_000).toFixed(2) + "M";
}

const tokensTotal = [];
const seenTotal = new Set();
function addTotal(n) {
  if (seenTotal.has(n)) return;
  seenTotal.add(n);
  tokensTotal.push({ n, expected: formatTokensTotal(n) });
}

// 0...5000 전수 — k 단위로 넘어가는 경계(1000) 주변을 촘촘히 덮는다.
for (let n = 0; n <= 5000; n++) addTotal(n);

// 5000...4_000_000 을 997(소수) 간격으로 스캔 — 파일 크기를 적당히 유지하면서
// M 단위 경계(1_000_000)를 포함해 넓은 범위를 표본추출한다.
for (let n = 5000; n <= 4_000_000; n += 997) addTotal(n);

// 4_000_000...UInt32.max(4_294_967_295) — wire 타입이 UInt32 라 이론상 여기까지
// 올 수 있는데, 위 스윕은 4_000_000 에서 멈춘다. 파일 크기가 다시 커지지 않도록
// 훨씬 성긴 간격으로 상위 구간도 표본추출한다.
const UINT32_MAX = 4_294_967_295;
for (let n = 4_000_000; n <= UINT32_MAX; n += 2_860_643) addTotal(n);
addTotal(UINT32_MAX);

// 리뷰에서 지목된 값 — 동점 근처(진짜 동점은 아님) 및 M 단위 경계
for (const n of [
  1150, 1450, 1650, 1950, 2050, 2150, 2550,
  1_045_000, 1_250_000, 1_255_000, 999_999, 1_000_000,
]) {
  addTotal(n);
}

// M 경로(소수 2자리)의 "진짜" 동점 — 위 두 스윕은 성긴 간격이라 M 경로의 동점을
// 우연히도 하나도 못 맞혔다(1e6, 1_250_000, 1_255_000, 2_500_000 은 전부 동점이
// 아니다). n/1_000_000 이 1.125 처럼 8분의 홀수(=이진으로 정확히 표현되는 값)일
// 때만 진짜 동점이 된다. Node 로 직접 실행해 확인한 값들이다:
//   (1125000/1e6).toFixed(2)    === "1.13"
//   (1625000/1e6).toFixed(2)    === "1.63"
//   (4125000/1e6).toFixed(2)    === "4.13"
//   (4294625000/1e6).toFixed(2) === "4294.63"
for (const n of [1_125_000, 1_625_000, 4_125_000, 4_294_625_000]) addTotal(n);

// k 경로(소수 1자리)의 "진짜" 동점 12개 — n=0...5000 스윕에는 이미 여럿 들어있지만
// (250, 750, 1250, ...) 5000 을 넘는 구간은 997 간격 스윕이 우연히 스치지 않는 한
// 동점을 하나도 검증하지 못한다. n/1000 이 250 mod 500 형태(=8분의 홀수)일 때
// 진짜 동점이며, 아래는 5000 초과 범위에서 고르게 뽑은 것들이다.
for (const n of [
  5250, 87750, 170250, 252750, 335250, 417750,
  500250, 582750, 665250, 747750, 830250, 912750,
]) {
  addTotal(n);
}

const tokensPerSec = [];
function addRate(v) {
  // Swift 쪽 파라미터는 Float(f32). Mac 쪽도 동일한 f32 값을 JSON 으로 파싱해
  // JS double 로 다루므로, Math.fround 로 f32 폭을 먼저 재현한 뒤 그 값으로
  // formatTokensPerSec 를 계산해야 두 런타임이 같은 입력을 비교하게 된다.
  const f = Math.fround(v);
  tokensPerSec.push({ v: f, expected: formatTokensPerSec(f) });
}

// 0...2000 을 0.5 간격으로 스캔 — 정수/반정수 경계와 동점 후보를 촘촘히 덮는다.
for (let i = 0; i <= 4000; i++) addRate(i / 2);

// 리뷰에서 지목된 값
for (const v of [0.5, 2.5, 122.5, 123.5, 999.4, 1000, 1234, 15678]) addRate(v);

const table = { tokensTotal, tokensPerSec };
process.stdout.write(JSON.stringify(table));
