# 문서 안내

## 현재 기준 문서

- [프로젝트 개요](../README.md)
- [아키텍처와 데이터 흐름](ARCHITECTURE.md)
- [iOS·CYD 데이터 송수신](DATA_TRANSPORT.md)
- [페어링과 E2EE 보안 모델](SECURITY.md)
- [ESP32 CYD 설치 및 문제 해결](CYD_SETUP_GUIDE.md)
- [Codex 저장 형식 호환성](codex-schema.md)
- [릴리즈 및 배포](RELEASE.md)

## 검증·역사 문서

- `ble-protocol/DEVICE-TEST.md`: BLE/iOS/CYD 프로토콜 실기기 검증 절차. 일반 설치 가이드가 아니다.
- `release-notes/`: 각 버전 출시 당시 변경 기록. 현재 사용법은 위 기준 문서를 따른다.
- `superpowers/specs/`: 구현 전 설계와 결정 근거. 이후 구현으로 세부가 바뀔 수 있다.
- `superpowers/plans/`: 작업 순서와 조사 기록. 완료된 계획의 체크리스트를 현재 기능 목록으로 해석하지 않는다.

현재 동작이 문서와 다르면 코드·테스트를 먼저 확인하고 기준 문서를 함께 갱신한다. 과거 릴리즈 노트와 계획서는 당시 기록 보존을 위해 소급 수정하지 않는다.
