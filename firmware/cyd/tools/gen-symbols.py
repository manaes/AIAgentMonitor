#!/usr/bin/env python3
"""ko-words.txt 를 읽어 lv_font_conv 의 --symbols 인자를 만든다.

손으로 --symbols 문자열을 옮겨 적는 대신 이 스크립트를 거치게 한 이유
(Task 13 브리프 리뷰, T13-A): 브리프 Step 2 의 손으로 옮긴 --symbols 문자열에
Step 1 단어 목록 어디에도 없는 한자 器(U+5668)가 섞여 있었다 — "기기"를 치다가
IME 가 두 번째 "기"를 한자로 잘못 변환한 것으로 보인다. 사람이 눈으로 대조해서
겨우 잡아냈다. 다음에는 그 대조 자체를 스크립트가 하게 만든다:

  - 한글 음절(U+AC00-U+D7A3) 이외의 문자가 단어 목록에 섞여 들어오면
    "누락"이 아니라 "출력에서 빠졌다는 경고"로 드러난다(자동으로 걸러지고,
    표준 오류로 알려준다) — 조용히 섞여 들어가지 않는다.
  - 다음에 라벨을 추가할 사람은 ko-words.txt 에 단어만 적으면 되고,
    유니코드 문자열을 손으로 맞출 필요가 없다.
"""

import sys
from pathlib import Path

HANGUL_SYLLABLES_START = 0xAC00
HANGUL_SYLLABLES_END = 0xD7A3  # 완성형 한글 음절 블록(가~힣), inclusive


def load_words(path: Path) -> list[str]:
    words = []
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        words.extend(line.split())
    return words


def main() -> int:
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} <ko-words.txt>", file=sys.stderr)
        return 2

    words_path = Path(sys.argv[1])
    words = load_words(words_path)
    if not words:
        print(f"error: {words_path} 에서 단어를 하나도 못 읽었다", file=sys.stderr)
        return 1

    hangul_codepoints: set[int] = set()
    rejected: dict[str, list[str]] = {}

    for word in words:
        for ch in word:
            cp = ord(ch)
            if HANGUL_SYLLABLES_START <= cp <= HANGUL_SYLLABLES_END:
                hangul_codepoints.add(cp)
            else:
                # 한글 음절 블록 밖의 문자(한자 오타, 이모지, 공백 등)는
                # symbols 문자열에 절대 조용히 섞이지 않는다 — 여기서 걸러서
                # stderr 로 알린다. ASCII 는 build-font.sh 가 --range 0x20-0x7F
                # 로 별도로 넣으므로 이 목록에 낄 필요가 없다.
                rejected.setdefault(word, []).append(f"U+{cp:04X}({ch!r})")

    if rejected:
        print("경고: 한글 음절 블록(U+AC00-U+D7A3) 밖의 문자를 건너뛴다:", file=sys.stderr)
        for word, chars in rejected.items():
            print(f"  단어 {word!r}: {', '.join(chars)}", file=sys.stderr)

    if not hangul_codepoints:
        print("error: 한글 음절이 하나도 안 남았다", file=sys.stderr)
        return 1

    # 순서는 lv_font_conv 입장에서 의미 없다(집합) — 재현 가능하도록 정렬만 한다.
    symbols = "".join(chr(cp) for cp in sorted(hangul_codepoints))
    print(symbols, end="")
    print(
        f"{len(hangul_codepoints)}개 고유 한글 음절, {len(words)}개 단어에서 추출",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
