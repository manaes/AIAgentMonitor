// AI Agent Monitor — CYD 펌웨어: 페어링 6자리 코드 키패드 화면 (Task 14b).
//
// Task 14a 가 켠 디스플레이/터치(LVGL + 저항막) 위에 처음으로 진짜 UI
// 로직을 얹는다. 이 화면이 하는 일은 하나뿐이다 — `Transport::authStep()`
// 을 읽어 사람이 코드를 입력해야 하는 상태(NeedsPairing/SendHello2/
// SendCode2/Failed)인지 판단해서 3×4 키패드를 그리거나, 이미 페어링된
// 흐름(SendAuth2/SendProof2/Subscribed)이면 그 대신 짧은 상태 문구를
// 보여준다.
//
// "코드가 입력됐을 때 다음에 무엇을 보낼지" 를 정하는 결정은 여기 없다 —
// `Transport::submitCode()` 를 부르면 `lib/authfsm` 의 순수 함수
// (`authInitialStep`/`authOnReply`, 둘 다 Task 11 산출물 그대로, 이
// 태스크가 고치지 않았다)가 그 결정을 대신 내린다. 이 화면은 그 결과
// (`authStep()`, `attemptsLeft()`)를 읽기만 한다 — Task 11 브리프가 남긴
// 교훈("연결 코드에 상태 기계를 녹여 넣지 않는다, 아이폰 쪽이 그렇게 하다
// 두 번 버그가 났다")을 UI 쪽에서도 그대로 지킨다.
//
// 이 파일 자체는 순수하지 않다(LVGL 위젯·millis() 를 직접 다룬다) —
// `transport.cpp` 와 같은 처지라 `lib/` 가 아니라 `src/` 에 둔다.
#pragma once

#include "transport.h"

/// 페어링 화면 위젯을 만든다. 한 번만 부른다(보통 `setup()` 에서,
/// `transport.begin()` 이후 — 초기 `authStep()`/`attemptsLeft()` 를
/// 읽어 첫 프레임을 그리기 때문이다).
void uiPairingCreate(Transport &transport);

/// 매 `loop()` 마다 부른다. 위젯을 다시 만들거나 지우지 않고 텍스트/버튼
/// 상태만 갱신한다 — Task 12 가 T12-D 로 남긴 "화면이 깜빡이지 않아야
/// 한다" 요구를 지키기 위해서다.
void uiPairingUpdate(Transport &transport);
