#!/usr/bin/env python3
"""LAN 전송(포트 4320, `GET /mirror`) 종단 검증용 참조 클라이언트.

**이것은 CYD 펌웨어의 대역이다.** 기기가 없는 동안 맥 쪽 전 구간 — v2 페어링,
재연결, 봉인된 스냅샷, 자원 상한 — 이 정말 동작하는지 확인한다. 절차는
`scripts/lan-e2e.md` 에 있다.

## 왜 파이썬이 암호까지 하는가

`websocat` 같은 범용 도구로는 이 프로토콜을 진행할 수 없다. 서버가 보낸 임시
공개키와 논스를 받아 **그 자리에서** 키를 합의하고 HMAC 을 계산해 다음 프레임을
보내야 하기 때문이다. 소켓과 암호를 한 프로세스가 같이 들고 있어야 순서가 선다.

## 왜 Rust 를 가져다 쓰지 않는가

이 클라이언트는 `src-tauri/src/crypto` 를 **부르지도 베끼지도 않는다.** 스펙
(`docs/superpowers/specs/2026-08-25-e2ee-protocol-v2-design.md`)만 보고 따로
구현한 것이다. 같은 코드가 자기 자신과 일치하는 것은 아무것도 증명하지 않는다 —
독립된 두 구현이 일치해야 증거가 된다. iOS 의 Swift 구현이 하는 역할과 같다.

## 골든 벡터 관문

어느 하위 명령을 부르든 `docs/ble-protocol/golden/e2ee-v2-sample.json` 대조를
**먼저** 하고, 하나라도 어긋나면 연결을 시도하지 않고 멈춘다. 이 순서가
뒤집히면 이 도구의 가치가 사라진다: 핸드셰이크가 실패했을 때 맥이 틀린 건지
이 파이썬이 틀린 건지 구분할 수 없게 된다.

## 필요한 것

    python3 -m pip install cryptography websockets

## 사용

    python3 scripts/lan_e2e_client.py selftest
    python3 scripts/lan_e2e_client.py pair          # 6자리를 물어본다
    python3 scripts/lan_e2e_client.py reconnect
    python3 scripts/lan_e2e_client.py watch
    python3 scripts/lan_e2e_client.py wrong-code
    python3 scripts/lan_e2e_client.py observe --seconds 180

## 스냅샷을 몇 장 기대할 수 있나

`--listen` 의 기본값이 1 인 이유가 있다. 맥의 게이트는 **스로틀이자 변경
게이트**다 — 스로틀이 초당 한 장이라는 상한을 주고, 변경 검사가 조용한 시간을
만든다(`GATE_HINT` 의 근거 참고). 유휴 맥에서 2장 이상을 요구하면 정상 동작이
`[FAIL]` 로 나온다.
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import sys
import time

try:
    from cryptography.hazmat.primitives import hashes, hmac
    from cryptography.hazmat.primitives.asymmetric.x25519 import (
        X25519PrivateKey,
        X25519PublicKey,
    )
    from cryptography.hazmat.primitives.ciphers.aead import ChaCha20Poly1305
    from cryptography.hazmat.primitives.kdf.hkdf import HKDF
    from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat
except ImportError as e:  # pragma: no cover - 사람이 읽을 안내
    sys.exit(f"cryptography 가 필요하다: python3 -m pip install cryptography ({e})")


REPO = pathlib.Path(__file__).resolve().parent.parent
GOLDEN = REPO / "docs" / "ble-protocol" / "golden" / "e2ee-v2-sample.json"
TOKEN_FILE = REPO / "scripts" / ".lan-e2e-token.json"

DEFAULT_URL = "ws://127.0.0.1:4320/mirror"


# ---------------------------------------------------------------------------
# 프로토콜 상수 — 스펙 3·4·5·6·7 장에서 그대로 옮긴 것이다.
# ---------------------------------------------------------------------------

AAD = b"aim-v2"
INFO_PAIR = b"aim-pair-v2"
INFO_S2C = b"aim-sess-v2-s2c"
INFO_C2S = b"aim-sess-v2-c2s"

#: 봉인 프레임의 카운터 접두 길이(빅엔디언 u64).
COUNTER_LEN = 8
#: ChaCha20-Poly1305 태그 길이.
TAG_LEN = 16


class ProtocolError(Exception):
    """맥이 프로토콜과 다르게 굴었다.

    **이 도구가 내리는 판정은 전부 이 예외이거나 `[FAIL]` 이다.** 다만 예외
    자체는 이것만 나오지 않는다 — hex 가 아닌 값(`ValueError`), 없는 필드
    (`KeyError`), 끊긴 소켓(`ConnectionClosed`)도 맥이 잘못 굴 때 나온다.
    `main` 이 그것들을 전부 `[FAIL]` 로 옮긴다(트레이스백을 사람에게 보여
    주면 "이 도구가 고장 났다"로 읽힌다).
    """


class Denied(ProtocolError):
    """정상적인 거절이다. 프로토콜 위반이 아니다 — `wrong-code` 가 기대하는 결과다.

    맥은 **두 자리**에서 거절한다. 둘을 구분하지 않으면 §6 이 정상 동작을
    맥의 결함으로 판정한다:

    - `CODE2` 에서 `{"ok":false,"left":N}` — 코드가 틀렸다. 예산이 줄어든다.
    - `HELLO2` 에서 벗은 `{"ok":false}` — **창이 닫혀 있다.** 예산이 소진됐거나
      TTL 이 지났거나 창을 연 적이 없다. `open_window` 가 `attempts_left > 0`
      일 때만 창을 주므로(`ble/pairing.rs:551`), 다섯 번을 틀린 **다음** 시도는
      `CODE2` 까지 가지도 못한다(`:715` 가 곧바로 `Rejected`). 스펙 5.2 표의
      마지막 줄 「창이 닫혀 있음 → `{"ok":false}`」가 바로 이것이다.

    `left` 가 `None` 이면 후자다. **표식은 `v` 필드의 부재**이고, 코드도 그것으로
    가른다 — doc 과 분별자가 같은 것을 가리켜야 한다.

    `await` 부재로 갈라도 정상 맥에서는 결과가 같지만(`Hello2` 팔은
    `AwaitingCode2`/`Rejected` 둘만 돌려준다 — `pairing.rs:710-736`), `v` 쪽이
    더 좁다: 오작동하는 맥이 `HELLO2` 에 `Nonce2` 모양
    (`{"ok":false,"v":2,"epk":…,"nonce":…}`)을 돌려주면 `await` 기준은 그것을
    「창이 닫혔다」로 잘못 부르고, `v` 기준은 프로토콜 위반으로 남긴다.
    """

    def __init__(self, left: int | None, stage: str) -> None:
        self.left = left
        self.stage = stage
        super().__init__(
            f"{stage}: 코드 거절 · 시도 {left}회 남음"
            if left is not None
            else f"{stage}: 창이 닫혀 있다 (벗은 ok:false — 예산 소진이거나 만료)"
        )


# ---------------------------------------------------------------------------
# 암호 원시 연산
# ---------------------------------------------------------------------------


def hkdf32(ikm: bytes, salt: bytes, info: bytes) -> bytes:
    """HKDF-SHA256 으로 32바이트를 뽑는다(스펙 3장)."""
    return HKDF(algorithm=hashes.SHA256(), length=32, salt=salt, info=info).derive(ikm)


def hmac256(key: bytes, msg: bytes) -> bytes:
    """HMAC-SHA256. **키도 메시지도 원시 바이트다** — hex 문자열의 UTF-8 이 아니다.

    골든 벡터의 `note` 가 못박아 둔 규약이고, 세 언어를 통틀어 가장 흔한 구현
    실수라고 스펙 4장이 적어 둔 바로 그 지점이다.
    """
    h = hmac.HMAC(key, hashes.SHA256())
    h.update(msg)
    return h.finalize()


def transcript(cpk: bytes, spk: bytes) -> bytes:
    """`cpk || spk` (64바이트). **항상 클라이언트 키가 먼저다**(스펙 4장)."""
    if len(cpk) != 32 or len(spk) != 32:
        raise ProtocolError("transcript 는 32바이트 공개키 둘로만 만든다")
    return cpk + spk


def derive_pair_key(ss: bytes, nonce: bytes) -> bytes:
    """토큰 한 건을 봉인하는 데만 쓰이는 키(스펙 5장)."""
    return hkdf32(ss, nonce, INFO_PAIR)


def derive_session_keys(ss: bytes, token: bytes, nonce: bytes) -> tuple[bytes, bytes]:
    """`(k_s2c, k_c2s)`. ikm 은 `ss || token_bytes` 다(스펙 6장).

    토큰을 ikm 에 섞는 이유가 이 프로토콜의 요점 중 하나다: X25519 가 깨져도
    토큰이 없으면 키를 못 만들고, 토큰이 새도 임시 개인키가 없으면 녹화된
    트래픽을 못 푼다.
    """
    ikm = ss + token
    return hkdf32(ikm, nonce, INFO_S2C), hkdf32(ikm, nonce, INFO_C2S)


def code_binding(code: str, tr: bytes) -> bytes:
    """`HMAC(utf8(6자리코드), transcript)`. **코드 자체는 링크를 건너지 않는다.**"""
    return hmac256(code.encode("utf-8"), tr)


def session_proof(token: bytes, nonce: bytes, tr: bytes) -> bytes:
    """`HMAC(token_bytes, nonce_bytes || transcript)`(스펙 6장).

    v1 은 `HMAC(token, nonce)` 였다. transcript 가 붙어야 키 합의가 토큰에
    묶이고, 중간자가 임시 키를 바꿔치기했을 때 이 값이 어긋난다.
    """
    return hmac256(token, nonce + tr)


def aead_nonce(counter: int) -> bytes:
    """`[0,0,0,0] || counter.to_be_bytes()` (12바이트, 스펙 7장)."""
    return b"\x00\x00\x00\x00" + counter.to_bytes(8, "big")


def seal(key: bytes, counter: int, plaintext: bytes) -> bytes:
    """`counter(8 BE) || ciphertext || tag`."""
    ct = ChaCha20Poly1305(key).encrypt(aead_nonce(counter), plaintext, AAD)
    return counter.to_bytes(8, "big") + ct


def unseal(key: bytes, frame: bytes, last_counter: int) -> tuple[int, bytes]:
    """봉인 프레임을 연다. 카운터가 전진하지 않으면 거절한다(재전송 방어).

    수신자가 자기 카운터만 세지 않고 프레임에 실린 카운터를 쓴다 — 스펙 7장이
    그렇게 정한 이유는 프레임 하나가 유실돼도 영구히 어긋나지 않게 하기
    위해서다.
    """
    if len(frame) < COUNTER_LEN + TAG_LEN:
        raise ProtocolError(f"봉인 프레임이 너무 짧다: {len(frame)}바이트")
    counter = int.from_bytes(frame[:COUNTER_LEN], "big")
    if counter <= last_counter:
        raise ProtocolError(
            f"카운터가 전진하지 않았다: {counter} <= {last_counter} (재전송이거나 구현 오류)"
        )
    plaintext = ChaCha20Poly1305(key).decrypt(aead_nonce(counter), frame[COUNTER_LEN:], AAD)
    return counter, plaintext


# ---------------------------------------------------------------------------
# 골든 벡터 관문
# ---------------------------------------------------------------------------


def run_selftest(verbose: bool = True) -> bool:
    """골든 벡터의 **모든** 출력을 재현한다. 하나라도 어긋나면 False.

    입력(`shared_secret`, `client_pub`, `server_pub`, `nonce`, `token`, `code`)만
    받아서 `transcript`·`code_binding`·`session_proof`·`pair_key`·`k_s2c`·`k_c2s`
    와 봉인 프레임 두 장을 전부 다시 만든다. 프레임이 두 장인 것이 중요하다 —
    카운터 0 짜리 한 장만으로는 논스 조립(접두/접미, 빅/리틀 엔디언)이 고정되지
    않는다.
    """
    if not GOLDEN.exists():
        print(f"[FAIL] 골든 벡터가 없다: {GOLDEN}", file=sys.stderr)
        return False
    g = json.loads(GOLDEN.read_text())
    i = g["input"]

    ss = bytes.fromhex(i["shared_secret"])
    cpk = bytes.fromhex(i["client_pub"])
    spk = bytes.fromhex(i["server_pub"])
    nonce = bytes.fromhex(i["nonce"])
    token = bytes.fromhex(i["token"])
    code = i["code"]

    tr = transcript(cpk, spk)
    s2c, c2s = derive_session_keys(ss, token, nonce)

    # 서버 방향 채널로 같은 평문을 연속 두 장 봉인한다. 평문 `{"v":2}` 는 골든
    # 파일에 적혀 있지 않아 Rust 테스트에서 읽어 왔다 — 보고서에 남긴 지적이다.
    plaintext = b'{"v":2}'

    checks = [
        ("transcript", tr.hex(), g["transcript"]),
        ("code_binding", code_binding(code, tr).hex(), g["code_binding"]),
        ("session_proof", session_proof(token, nonce, tr).hex(), g["session_proof"]),
        ("pair_key", derive_pair_key(ss, nonce).hex(), g["pair_key"]),
        ("k_s2c", s2c.hex(), g["k_s2c"]),
        ("k_c2s", c2s.hex(), g["k_c2s"]),
        ("sealed_frame_0", seal(s2c, 0, plaintext).hex(), g["sealed_frame_0"]),
        ("sealed_frame_1", seal(s2c, 1, plaintext).hex(), g["sealed_frame_1"]),
    ]

    ok = True
    for name, got, want in checks:
        if got == want:
            if verbose:
                print(f"  [OK]   {name}")
        else:
            ok = False
            print(f"  [FAIL] {name}\n         기대 {want}\n         실제 {got}", file=sys.stderr)

    # 여는 쪽도 확인한다 — 봉인만 맞고 해제가 틀리면 실제 연결에서 스냅샷이
    # 안 열린다.
    counter, opened = unseal(s2c, bytes.fromhex(g["sealed_frame_1"]), last_counter=0)
    if counter != 1 or opened != plaintext:
        ok = False
        print(f"  [FAIL] unseal: counter={counter} plaintext={opened!r}", file=sys.stderr)
    elif verbose:
        print("  [OK]   unseal(sealed_frame_1)")

    # 카운터가 전진하지 않으면 거절해야 한다.
    try:
        unseal(s2c, bytes.fromhex(g["sealed_frame_1"]), last_counter=1)
    except ProtocolError:
        if verbose:
            print("  [OK]   재전송 거절 (counter <= last_counter)")
    else:
        ok = False
        print("  [FAIL] 재전송을 거절하지 않았다", file=sys.stderr)

    return ok


def gate() -> None:
    """관문. 골든 벡터가 어긋나면 **연결을 시도하지 않고** 죽는다."""
    print("골든 벡터 대조:")
    if not run_selftest():
        sys.exit("골든 벡터 대조 실패 — 이 클라이언트가 틀렸다. 맥을 의심하기 전에 여기를 고쳐라.")
    print("골든 벡터 통과. 이제 이 클라이언트는 신뢰할 수 있는 기준이다.\n")


# ---------------------------------------------------------------------------
# 토큰 저장 — CYD 의 NVS 에 해당한다
# ---------------------------------------------------------------------------


def save_token(token_hex: str) -> None:
    """토큰을 0600 으로 **처음부터** 만들어 쓴다.

    `write_text` 로 쓴 뒤 `chmod` 하면 그 사이에 0644 인 창이 열린다. 짧지만
    이 파일의 내용은 재연결 자격 그 자체라 굳이 창을 낼 이유가 없다.
    """
    body = json.dumps({"token": token_hex, "saved_at": int(time.time())}) + "\n"
    fd = os.open(TOKEN_FILE, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        f.write(body)
    print(f"토큰 …{token_hex[-4:]} 를 {TOKEN_FILE} 에 저장했다 (CYD 의 NVS 자리).")


def load_token() -> str:
    if not TOKEN_FILE.exists():
        sys.exit(f"저장된 토큰이 없다: {TOKEN_FILE}\n먼저 `pair` 를 한 번 돌려라.")
    return json.loads(TOKEN_FILE.read_text())["token"]


# ---------------------------------------------------------------------------
# 세션
# ---------------------------------------------------------------------------


class Session:
    """WebSocket 하나 = 세션 하나 = `CentralId` 하나.

    ## 어떤 프레임이 무엇인가 (`src-tauri/src/lan/` 에서 확인한 것)

    - **인증은 양방향 모두 텍스트 프레임**이다. 서버는 `Message::Text` 만
      `offer_frame` 으로 올린다(`server.rs` 의 수신 루프). 바이너리로 보내면
      `last_seen` 만 갱신되고 조용히 버려진다.
    - **스냅샷은 바이너리 프레임**이고 봉인 프레임 **원시 바이트 그대로**다.
      hex 도 아니고 줄바꿈 구분자도 없다 — WebSocket 이 이미 프레임을 나누므로
      LAN 은 청킹하지 않는다(`lan/mod.rs::send_prepared`).

    이 구분이 iroh 와 다르다는 점이 중요하다. iroh 는 hex + `\\n` NDJSON 이다.
    전송마다 다르므로 스펙이 아니라 코드에서 확인해야 한다.
    """

    def __init__(self, ws) -> None:
        self.ws = ws
        self.sk = X25519PrivateKey.generate()
        self.cpk = self.sk.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)
        #: 인증 응답을 기다리는 사이에 도착한 바이너리 프레임.
        self.pending_binary: list[bytes] = []
        self.k_s2c: bytes | None = None
        self.last_counter = -1

    # -- 저수준 입출력 ---------------------------------------------------

    def send_text(self, s: str) -> None:
        print(f"  → {s}")
        self.ws.send(s)

    def recv_json(self, timeout: float = 10.0) -> tuple[dict, str]:
        """텍스트 프레임 하나를 JSON 으로. 그 전에 오는 바이너리는 모아 둔다."""
        while True:
            msg = self.ws.recv(timeout=timeout)
            if isinstance(msg, (bytes, bytearray)):
                self.pending_binary.append(bytes(msg))
                continue
            print(f"  ← {msg}")
            try:
                return json.loads(msg), msg
            except json.JSONDecodeError as e:
                raise ProtocolError(f"JSON 이 아닌 텍스트 프레임: {msg!r} ({e})")

    def agree(self, epk_hex: str) -> tuple[bytes, bytes]:
        """맥의 임시 공개키와 X25519 한다. `(공유 비밀, spk)` 를 돌려준다.

        **저차 점 방어는 여기 한 곳뿐이다.** `cryptography` 는 공유 비밀이
        전부 0 이 되면 `exchange()` 가 `ValueError: Error computing shared key.`
        를 던진다(실측 확인). 그래서 호출부에 `ss == bytes(32)` 같은 이중
        검사를 두지 않는다 — 두면 도달하지 않는 코드가 되고, 페어링 경로에만
        있고 재연결 경로에는 없어서 「어느 쪽이 실제 방어인가」가 코드에서
        읽히지 않게 된다. Rust 쪽 `was_contributory()` 와 같은 자리다.
        """
        spk = bytes.fromhex(epk_hex)
        if len(spk) != 32:
            raise ProtocolError(f"epk 가 32바이트가 아니다: {len(spk)}")
        return self.sk.exchange(X25519PublicKey.from_public_bytes(spk)), spk

    # -- 핸드셰이크 ------------------------------------------------------

    def pair(self, code: str) -> str:
        """`HELLO2` → `CODE2`. 발급된 토큰 hex 를 돌려준다."""
        print("[페어링] HELLO2")
        self.send_text("HELLO2:" + self.cpk.hex())
        reply, raw = self.recv_json()

        # **거절을 프로토콜 위반보다 먼저 본다.** 순서가 반대면 예산이 소진된
        # 뒤의 `HELLO2` 가 `[FAIL]` 로 끝나고, 절차는 그것을 「맥의 결함」으로
        # 읽으라고 지시한다 — 이 도구가 막으려고 존재하는 바로 그 역전이다.
        # `Denied` 의 doc 에 두 거절 자리를 적어 두었다.
        if reply.get("ok") is False and "v" not in reply:
            raise Denied(reply.get("left"), "HELLO2")
        if reply.get("v") != 2 or reply.get("ok") is not False or reply.get("await") != "code":
            raise ProtocolError(f"AwaitingCode2 를 기대했다: {reply}")
        # ── 확인 항목: 6자리 코드가 링크를 건너지 않는다 ──────────────
        if code in raw:
            raise ProtocolError(f"응답에 6자리 코드가 들어 있다! {raw}")
        print("  [OK] 응답에 6자리 코드가 없다 (v2 는 HMAC(code, transcript) 만 보낸다)")
        if len(reply["nonce"]) != 32:
            raise ProtocolError(f"논스가 32 hex 가 아니다: {reply['nonce']}")

        # 저차 점은 `agree()` 안에서 걸린다 — 여기에 이중 검사를 두지 않는다
        # (`agree` 의 doc). 재연결 경로와 방어가 같은 자리에 하나만 있어야
        # 「어느 쪽이 실제 방어인가」가 코드에서 읽힌다.
        ss, spk = self.agree(reply["epk"])
        tr = transcript(self.cpk, spk)
        nonce = bytes.fromhex(reply["nonce"])

        print("[페어링] CODE2 — 코드 대신 HMAC(code, transcript)")
        self.send_text("CODE2:" + code_binding(code, tr).hex())
        reply, raw = self.recv_json()

        if reply.get("ok") is not True:
            # 코드 거절은 프로토콜 위반이 아니다 — 예산이 줄어드는 정상 경로다.
            raise Denied(reply.get("left"), "CODE2")
        if reply.get("v") != 2 or "sealed" not in reply:
            raise ProtocolError(f"Granted2 를 기대했다: {reply}")

        k_pair = derive_pair_key(ss, nonce)
        counter, payload = unseal(k_pair, bytes.fromhex(reply["sealed"]), last_counter=-1)
        if counter != 0:
            raise ProtocolError(f"sealed 의 카운터는 0 이어야 한다: {counter}")
        token_hex = json.loads(payload)["token"]
        if len(token_hex) != 32 or not all(c in "0123456789abcdef" for c in token_hex):
            raise ProtocolError(f"토큰이 128비트 소문자 hex 가 아니다: {token_hex}")

        # ── 확인 항목: 토큰이 평문으로 링크를 건너지 않는다 ────────────
        if token_hex in raw:
            raise ProtocolError("토큰이 응답에 평문으로 들어 있다! (값은 찍지 않는다)")
        # 토큰 전체를 찍지 않는다 — 그 값이 곧 재연결 자격이고, 스크롤백과
        # `| tee` 대상은 `chmod 600` 도 `.gitignore` 도 걸리지 않는다. 끝 4자리면
        # 「방금 받은 그 토큰인가」를 확인하는 진단력은 그대로다.
        print(f"  [OK] 토큰 …{token_hex[-4:]} 는 봉인 프레임 안에서만 왔다")

        # 스펙 6.1 — 왕복 없이 그 자리에서 세션 키로 전환한다.
        token_bytes = bytes.fromhex(token_hex)
        self.k_s2c, _ = derive_session_keys(ss, token_bytes, nonce)
        print("  [OK] 세션 키로 전환 (k_pair 는 토큰 한 건에만 쓰였다)")
        return token_hex

    def reconnect(self, token_hex: str) -> None:
        """`AUTH2` → `PROOF2`. CYD 가 첫 페어링 이후 매 부팅마다 하는 일이다."""
        token_bytes = bytes.fromhex(token_hex)
        print("[재연결] AUTH2")
        self.send_text("AUTH2:" + self.cpk.hex())
        reply, _ = self.recv_json()
        if reply.get("v") != 2 or reply.get("ok") is not False or "epk" not in reply:
            raise ProtocolError(f"Nonce2 를 기대했다: {reply}")

        ss, spk = self.agree(reply["epk"])
        tr = transcript(self.cpk, spk)
        nonce = bytes.fromhex(reply["nonce"])
        self.k_s2c, _ = derive_session_keys(ss, token_bytes, nonce)

        print("[재연결] PROOF2 — HMAC(token, nonce || transcript)")
        self.send_text("PROOF2:" + session_proof(token_bytes, nonce, tr).hex())
        reply, _ = self.recv_json()
        if reply.get("ok") is not True or reply.get("v") != 2:
            raise ProtocolError(f"Authorized2 를 기대했다: {reply}")
        print("  [OK] 재인증 성공 — 토큰은 링크를 건너지 않았다")

    # -- 데이터 단계 -----------------------------------------------------

    def listen(self, count: int, timeout: float = 40.0) -> tuple[int, str]:
        """스냅샷 프레임을 받아 연다. `count < 0` 이면 끊길 때까지.

        `(받은 장수, 끝난 이유)` 를 돌려준다. 이유는 `"count"`(원하는 만큼
        받았다) · `"closed"`(맥이 끊었다) · `"timeout"`(시한이 지났다) 셋이다.

        **끊김과 시한을 구분해 돌려주는 것이 요점이다.** 둘을 뭉뚱그리면
        `watch`(§5, 기기 해제 확인)가 어떤 경우에도 성공으로 끝난다 — 해제
        버튼을 아예 누르지 않아도 시한이 끊김과 같은 값이 되어 「해제하는 즉시
        끊긴다」에 체크가 들어간다. 실패할 수 없는 검사는 검사가 아니다.

        **`timeout` 은 recv 하나당 침묵 시한이 아니라 전체 경과의 총 시한이다.**
        침묵 시한으로 두면 프레임이 그보다 자주 오는 동안 이 함수가 영원히
        돌아오지 않는다. 그리고 그것이 바로 §5 가 잡으려는 결함의 모습이다 —
        **해제했는데 맥이 놓지 않고 계속 보내는 경우.** 침묵 시한이었다면 그때
        `[FAIL]` 을 내지 못하고 그냥 매달리므로, 사람이 Ctrl-C 로 나가고 판정이
        다시 사람에게 넘어간다. §1-1 이 「§5 를 도는 동안 에이전트를 켜 두라」고
        지시하는 이상 프레임이 흐르는 것이 §5 의 **기대 구성**이라, 이 구분이
        없으면 I-2 의 수정이 그 한 갈래에서 성립하지 않는다.
        """
        from websockets.exceptions import ConnectionClosed

        if self.k_s2c is None:
            raise ProtocolError("인증 전에는 들을 것이 없다")
        got = 0
        # 루프 **밖에서** 한 번 잡는다. 프레임이 오든 안 오든 이 시각은 움직이지
        # 않는다 — 맥의 `AUTH_DEADLINE` 이 `sleep_until` 에 절대 시각을 주는 것과
        # 같은 이유다(`lan/server.rs` 의 doc: 「상대가 무엇을 보내든 읽든 이 시각은
        # 움직이지 않는다」).
        deadline = time.monotonic() + timeout
        while count < 0 or got < count:
            if self.pending_binary:
                frame = self.pending_binary.pop(0)
            else:
                left = deadline - time.monotonic()
                if left <= 0:
                    print(f"  총 시한 {timeout}초가 지났다 (받은 프레임 {got}장)")
                    return got, "timeout"
                try:
                    msg = self.ws.recv(timeout=left)
                except TimeoutError:
                    if got:
                        print(f"  총 시한 {timeout}초가 지났다 (받은 프레임 {got}장)")
                    else:
                        print(f"  {timeout}초 동안 아무것도 오지 않았다")
                    return got, "timeout"
                except ConnectionClosed as e:
                    print(f"  맥이 연결을 끊었다: {e} (받은 프레임 {got}장)")
                    return got, "closed"
                if isinstance(msg, str):
                    print(f"  ← (텍스트) {msg}")
                    continue
                frame = bytes(msg)

            # ── 확인 항목: 스냅샷이 평문 JSON 이 아니다 ────────────────
            if frame[:1] == b"{":
                raise ProtocolError(f"스냅샷이 '{{' 로 시작한다 — 평문 JSON 이다! {frame[:64]!r}")
            counter, payload = unseal(self.k_s2c, frame, self.last_counter)
            self.last_counter = counter
            got += 1
            print(f"  ← [봉인 {len(frame)}B, counter={counter}] {payload.decode('utf-8')}")
        return got, "count"


# ---------------------------------------------------------------------------
# 하위 명령
# ---------------------------------------------------------------------------


def connect(url: str):
    try:
        from websockets.sync.client import connect as ws_connect
    except ImportError as e:
        sys.exit(f"websockets 가 필요하다: python3 -m pip install websockets ({e})")
    print(f"연결: {url}")
    # `max_size` 는 맥의 `MAX_FRAME_BYTES`(64 KiB)와 같은 값으로 둔다 — 그보다
    # 큰 것이 오면 이쪽에서도 알아채야 하므로 넓히지 않는다.
    return ws_connect(url, max_size=64 * 1024, open_timeout=10)


#: 맥이 스냅샷을 **언제** 보내는지. 이 문장이 틀리면 사람이 정상 동작을
#: 실패로 읽는다.
#:
#: LAN 게이트는 **스로틀이자 변경 게이트**다 — 둘 다이지 둘 중 하나가 아니다.
#: `EmitGate::should_emit`(`emitter.rs:16-26`)이 검사를 둘 순서대로 한다:
#:
#: 1. **변경 검사** — 내용 해시가 같으면 시간과 무관하게 `false`(`:18-19`).
#:    `hash_snapshot`(`:37-57`)에는 **타임스탬프가 없다**(에이전트 종류·rate·
#:    토큰 합계·쿼터·프로젝트만 넣는다). **조용한 시간을 만드는 것이 이쪽이다.**
#: 2. **스로틀** — 내용이 바뀌었어도 직전 송출로부터 `LAN_THROTTLE`(1000ms,
#:    `lan/mod.rs:26`) 안이면 `false`(`:20-23`). **상한 「최대 1Hz」가 이쪽이다.**
#:    바뀐 스냅샷도 막는다는 것은 전용 테스트가 못박아 뒀다
#:    (`emitter.rs:93` `changed_snapshot_within_throttle_is_suppressed`).
#:
#: `lan/mod.rs:94-96` 이 둘을 한 문장에 적어 뒀다 — 「앱의 틱은 250ms 인데
#: 미러는 1Hz 로 묶는다(`LAN_THROTTLE`). 내용이 그대로면 아예 내보내지 않으므로,
#: 조용한 시간에는 봉인 카운터도 전진하지 않는다」.
#:
#: 결과:
#: - **페어링 직후 첫 한 장은 반드시 나간다.** 게이트가 대상 검사 뒤에 있고
#:   (`lan/mod.rs:481-488`) 아무도 없을 때는 소비되지 않기 때문이다.
#: - **두 번째부터는 스냅샷 내용이 실제로 바뀌어야 나간다.**
#: - **재연결은 첫 장도 보장되지 않는다.** `gate.reset()` 은 `set_enabled(true)`
#:   에서만 불린다(`lan/mod.rs:190`) — 재연결은 게이트를 되돌리지 않으므로
#:   내용이 그대로면 0장이다.
GATE_HINT = (
    "맥은 **내용이 바뀔 때만** 보내고, 바뀌어도 **최대 초당 한 장**이다.\n"
    "  게이트가 스로틀이자 변경 게이트라서다 — 스로틀이 1Hz 상한을 주고,\n"
    "  해시에 타임스탬프가 없는 변경 검사가 조용한 시간을 만든다. 그래서\n"
    "  에이전트가 돌지 않는 맥은 아무것도 보내지 않는다. 이 절을 도는 동안\n"
    "  Claude·Codex 세션을 하나 돌려 토큰 사용량이 실제로 변하게 두라.\n"
    "  그렇게 해 두고도 0장이면 그때가 맥의 결함이다."
)


def listen_and_judge(s: "Session", want: int, label: str) -> int:
    """`--listen` 공통 처리. 부족하면 **왜 부족할 수 있는지**까지 말한다.

    수를 못 채웠다고 곧바로 「맥이 틀렸다」로 읽히게 두지 않는다 — 이 전송에서
    가장 흔한 원인은 결함이 아니라 조용한 맥이다(`GATE_HINT`).
    """
    if want < 1:
        # 음수는 `got >= want` 를 언제나 참으로 만들어 **실패할 수 없는 검사**가
        # 된다. 절차에 나오지 않는 값이지만, 조용히 통과하는 갈래를 남겨 둘
        # 이유가 없다(I-2 와 같은 이야기다).
        print(f"  [FAIL] --listen 은 0 이상이어야 한다: {want}", file=sys.stderr)
        return 1
    print(f"\n[스냅샷] {want}장 기다린다")
    got, _why = s.listen(want)
    if got >= want:
        print("  [OK] 봉인된 스냅샷이 열린다")
        return 0
    print(
        f"  [FAIL] {want}장을 기대했는데 {got}장 받았다 ({label})\n  {GATE_HINT}",
        file=sys.stderr,
    )
    return 1


def cmd_pair(args) -> int:
    with connect(args.url) as ws:
        s = Session(ws)
        token = s.pair(args.code)
        save_token(token)
        if args.listen:
            # 페어링 직후 첫 한 장은 게이트와 무관하게 보장된다(`GATE_HINT`).
            # 그래서 기본값이 1 이다 — 유휴 맥에서도 통과해야 하는 검사다.
            return listen_and_judge(s, args.listen, "페어링 직후")
    return 0


def cmd_reconnect(args) -> int:
    token = load_token()
    with connect(args.url) as ws:
        s = Session(ws)
        s.reconnect(token)
        if args.listen:
            # 재연결은 첫 장도 보장되지 않는다 — `gate.reset()` 을 부르지
            # 않으므로 내용이 그대로면 0장이다(`GATE_HINT`).
            return listen_and_judge(s, args.listen, "재연결 — 첫 장도 보장되지 않는다")
    return 0


def cmd_watch(args) -> int:
    """재인증하고 끊길 때까지 듣는다. 맥에서 기기를 해제하는 순간을 본다.

    **끊김으로 끝나야만 통과다.** 조용히 타임아웃한 것은 「해제하면 끊긴다」의
    증거가 아니라 아무 일도 일어나지 않았다는 뜻이다.
    """
    token = load_token()
    with connect(args.url) as ws:
        s = Session(ws)
        s.reconnect(token)
        print(
            f"\n[감시] 끊길 때까지 듣는다 (최대 {args.timeout}초).\n"
            "  맥 Devices 탭에서 이 기기를 해제하라 — 그 즉시 끊겨야 한다."
        )
        t0 = time.monotonic()
        got, why = s.listen(-1, timeout=args.timeout)
        dt = time.monotonic() - t0
        if why == "closed":
            print(f"  [OK] 맥이 연결을 놓았다 ({got}장 수신, {dt:.1f}초)")
            return 0
        print(
            f"  [FAIL] {dt:.1f}초 동안 끊기지 않았다 ({got}장 수신).\n"
            "  해제를 눌렀는데도 이렇다면 그것이 결함이다. 누르지 않았다면\n"
            "  누르고 다시 돌려라 — 이 검사는 끊김으로 끝나야만 통과다.",
            file=sys.stderr,
        )
        return 1


def cmd_wrong_code(args) -> int:
    """틀린 코드를 반복 제출해 창당 5회 예산이 실제로 닫히는지 본다.

    `CODE2` 는 성공·실패와 무관하게 핸드셰이크를 소비하므로(`pairing.rs:742`
    의 `remove` 가 검증보다 앞에 있다), 매 시도마다 `HELLO2` 부터 다시 한다 —
    연결도 새로 연다(연결 하나 = 세션 하나).

    **그래서 마지막 시도는 `CODE2` 까지 가지 않는다.** 예산이 0 이면
    `open_window` 가 창을 주지 않고(`pairing.rs:551`) `HELLO2` 가 곧바로
    벗은 `{"ok":false}` 를 돌려준다(`:715`). 기대하는 `left` 순서
    `[4,3,2,1,0,None]` 의 마지막 `None` 이 그것이다 — 앞의 다섯은 `CODE2`
    거절, 마지막 하나는 `HELLO2` 에서 닫힌 창이다.
    """
    seen: list[int | None] = []
    stages: list[str] = []
    for attempt in range(1, args.tries + 1):
        print(f"\n[시도 {attempt}] 틀린 코드")
        with connect(args.url) as ws:
            s = Session(ws)
            try:
                s.pair(args.code)
            except Denied as e:
                seen.append(e.left)
                stages.append(e.stage)
                print(f"  거절: {e}")
                continue
            print("  [FAIL] 틀린 코드가 통과했다!", file=sys.stderr)
            return 1

    print(f"\n관측한 `left`: {seen}")
    print(f"거절이 나온 단계: {stages}")
    # 창을 방금 연 상태에서 시작했다면 4,3,2,1,0 뒤로는 전부 닫힘이어야 한다.
    want = [4, 3, 2, 1, 0][: args.tries] + [None] * max(0, args.tries - 5)
    if seen == want:
        print("  [OK] 예산이 5회에서 정확히 소진되고 그 뒤로는 창이 닫혔다")
        return 0
    print(
        f"  [주의] 기대한 순서는 {want} 였다.\n"
        "  창을 방금 열지 않았거나(남은 예산이 5회가 아니었다) 다른 기기가\n"
        "  같은 창에서 시도했다면 어긋날 수 있다 — 창은 소유자가 없고 예산을\n"
        "  공유한다. [페어링 시작] 을 새로 누르고 다시 돌려라.",
        file=sys.stderr,
    )
    return 1


def cmd_observe(args) -> int:
    """인증하지 않은 채 붙어만 있는다. 두 가지를 동시에 본다.

    1. **한 바이트도 오지 않는가** — 인가되지 않은 상대는 스냅샷을 0바이트 받는다.
    2. **인증 시한이 정말 연결을 놓는가** — `AUTH_DEADLINE` 은 150초다.

    Ping/Pong 은 라이브러리가 알아서 주고받으므로 `recv()` 에 올라오지 않는다.
    여기서 세는 것은 애플리케이션 프레임뿐이다.
    """
    from websockets.exceptions import ConnectionClosed

    t0 = time.monotonic()
    total = 0
    frames = 0
    with connect(args.url) as ws:
        print(f"아무것도 보내지 않고 최대 {args.seconds}초 기다린다.")
        while True:
            left = args.seconds - (time.monotonic() - t0)
            if left <= 0:
                print(f"  {args.seconds}초가 지났는데 맥이 끊지 않았다.")
                break
            try:
                msg = ws.recv(timeout=min(left, 5.0))
            except TimeoutError:
                continue
            except ConnectionClosed as e:
                dt = time.monotonic() - t0
                print(f"\n맥이 {dt:.1f}초 만에 연결을 끊었다: {e}")
                print(f"  받은 애플리케이션 프레임 {frames}장 / {total}바이트")
                if frames or total:
                    print("  [FAIL] 인가되지 않았는데 무언가 받았다", file=sys.stderr)
                    return 1
                print("  [OK] 인가 전에는 한 바이트도 오지 않았다")
                if 140 <= dt <= 175:
                    print("  [OK] AUTH_DEADLINE(150초)이 실제로 연결을 놓았다")
                else:
                    print(f"  [주의] 150초 언저리가 아니다: {dt:.1f}초")
                return 0
            frames += 1
            total += len(msg)
            print(f"  [FAIL] 받았다: {msg!r}", file=sys.stderr)
    print(f"  받은 애플리케이션 프레임 {frames}장 / {total}바이트")
    # 끊기지 않은 것 자체가 실패다 — 인가되지 않은 연결은 시한을 받아야 한다.
    return 1


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--url", default=DEFAULT_URL, help=f"기본값 {DEFAULT_URL}")
    sub = p.add_subparsers(dest="cmd", required=True)

    sp = sub.add_parser("selftest", help="골든 벡터 대조만 한다 (연결하지 않는다)")
    sp.set_defaults(func=lambda a: 0)

    sp = sub.add_parser("pair", help="HELLO2/CODE2 로 페어링하고 토큰을 저장한다")
    # 생략하면 물어본다. `--code` 는 argv 라 `ps` 로 같은 기계의 다른 사용자에게
    # 보이고 셸 히스토리에도 남는다 — 코드는 120초 · 1회용이라 영향이 작지만,
    # 굳이 남길 이유도 없다.
    sp.add_argument("--code", help="맥 화면의 6자리. 생략하면 물어본다(권장)")
    # 기본 1장. 페어링 직후 첫 한 장만 게이트와 무관하게 보장된다(`GATE_HINT`).
    sp.add_argument("--listen", type=int, default=1, help="이어서 받을 스냅샷 장수 (기본 1)")
    sp.set_defaults(func=cmd_pair)

    sp = sub.add_parser("reconnect", help="저장된 토큰으로 AUTH2/PROOF2 재인증")
    # 재연결은 첫 장도 보장되지 않는다 — 내용이 바뀌는 동안에만 온다.
    sp.add_argument("--listen", type=int, default=1, help="이어서 받을 스냅샷 장수 (기본 1)")
    sp.set_defaults(func=cmd_reconnect)

    sp = sub.add_parser("watch", help="재인증하고 끊길 때까지 듣는다 (기기 해제 확인)")
    sp.add_argument("--timeout", type=float, default=120.0)
    sp.set_defaults(func=cmd_watch)

    sp = sub.add_parser("wrong-code", help="틀린 코드로 창당 5회 예산을 확인한다")
    sp.add_argument("--code", default="000000")
    sp.add_argument("--tries", type=int, default=6)
    sp.set_defaults(func=cmd_wrong_code)

    sp = sub.add_parser("observe", help="인증하지 않고 붙어만 있는다 (0바이트 · AUTH_DEADLINE)")
    sp.add_argument("--seconds", type=float, default=180.0)
    sp.set_defaults(func=cmd_observe)

    # **파이프로 넘길 때도 순서가 맞아야 한다.** stdout 은 리다이렉트되면
    # 블록 버퍼가 되는데 stderr 은 아니라서, `| tee` 로 받으면 `[FAIL]` 이
    # 로그 맨 앞에 찍힌다 — OK 들이 그 아래 늘어서 있으니 「시작하자마자
    # 실패했다」로 읽힌다. 이 절차는 사람이 로그를 남겨 가며 도는 것이므로
    # 줄 단위 버퍼로 고정한다.
    sys.stdout.reconfigure(line_buffering=True)

    args = p.parse_args()

    # 관문. 어느 명령이든 여기를 먼저 지난다.
    gate()
    if args.cmd == "selftest":
        return 0

    if getattr(args, "code", None) is None and args.cmd == "pair":
        args.code = input("맥 화면의 6자리를 입력하라: ").strip()

    from websockets.exceptions import WebSocketException

    try:
        return args.func(args)
    except ProtocolError as e:
        print(f"\n[FAIL] {e}", file=sys.stderr)
        return 1
    # **맥이 잘못 굴 때 사람에게 트레이스백을 보여주지 않는다.** 그러면
    # 「이 도구가 고장 났다」로 읽혀서, 정작 판정해야 할 맥의 결함이 묻힌다.
    # 실제로 나오는 것들: hex 가 아닌 `epk`/`nonce`/`sealed`(`ValueError` —
    # `binascii.Error` 는 그 하위다), 저차 점에서 `cryptography` 가 던지는
    # `ValueError`, 없는 필드(`KeyError`), 핸드셰이크 중 끊김
    # (`ConnectionClosed` ⊂ `WebSocketException`), 소켓·파일(`OSError`).
    except (ValueError, KeyError, OSError, WebSocketException) as e:
        print(f"\n[FAIL] 맥의 응답을 해석할 수 없다 — {type(e).__name__}: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
