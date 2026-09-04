#include "snapshot.h"

#include <ArduinoJson.h>

namespace {

SnapshotAgentKind mapAgentKind(int raw) {
    switch (raw) {
        case 0: return SnapshotAgentKind::Claude;
        case 1: return SnapshotAgentKind::Codex;
        case 2: return SnapshotAgentKind::Antigravity;
        default: return SnapshotAgentKind::Unknown;  // snapshot.h 의 fail-safe 문서 참고.
    }
}

SnapshotProjectStatus mapProjectStatus(int raw) {
    switch (raw) {
        case 0: return SnapshotProjectStatus::Active;
        case 1: return SnapshotProjectStatus::Idle;
        default: return SnapshotProjectStatus::Dormant;  // 2 도, 범위 밖도 여기로(snapshot.h 문서).
    }
}

/// 프로젝트 객체 하나. 필수 필드(`id`/`n`/`m`/`r`/`t`/`s`) 중 하나라도 없거나
/// 선언된 타입이 아니면 false — 개별 프로젝트를 조용히 스킵하지 않는다
/// (snapshotParse 문서: 실패는 스냅샷 전체로 전파된다).
bool parseProject(JsonObjectConst obj, SnapshotProject &out) {
    if (!obj["id"].is<uint32_t>() || !obj["n"].is<const char *>() ||
        !obj["m"].is<const char *>() || !obj["r"].is<float>() ||
        !obj["t"].is<uint64_t>() || !obj["s"].is<int>()) {
        return false;
    }
    out.id = obj["id"].as<uint32_t>();
    out.name = obj["n"].as<const char *>();
    out.model = obj["m"].as<const char *>();
    out.rateTokPerSec = obj["r"].as<float>();
    out.lastActivityEpochSec = obj["t"].as<uint64_t>();
    out.status = mapProjectStatus(obj["s"].as<int>());
    return true;
}

/// 에이전트 객체 하나. 필수 필드(`k`/`r`/`t5`/`pj`)는 프로젝트와 같은 규칙 —
/// 없거나 타입이 다르면 false. optional 필드(`p5`/`r5`/`pw`/`rw`)는 `.is<T>()`
/// 로 "키가 있고 선언된 타입이다" 를 확인한다 — 키가 아예 없는 경우와 키는
/// 있지만 타입이 다른 경우 둘 다 false 로 나와 "값 없음" 취급이 되는데, 이
/// 둘을 굳이 가르지 않는다: 맥은 이 필드를 절대 다른 타입으로 보내지 않으므로
/// (wire.rs 의 `Option<f32>`/`Option<u64>`, `serde` 직렬화가 타입을 보장한다)
/// "타입이 다른 optional 필드" 는 실제로 도달 불가능한 경로다.
/// 에이전트 객체 하나. 필수 필드(`k`/`r`/`t5`)만 빠르게 파싱.
bool parseAgent(JsonObjectConst obj, SnapshotAgent &out) {
    if (!obj["k"].is<int>() || !obj["r"].is<float>() || !obj["t5"].is<uint32_t>()) {
        return false;
    }
    out.kind = mapAgentKind(obj["k"].as<int>());
    out.rateTokPerSec = obj["r"].as<float>();
    out.tokens5hCumulative = obj["t5"].as<uint32_t>();

    out.has5hUsagePct = obj["p5"].is<float>();
    out.usage5hPct = out.has5hUsagePct ? obj["p5"].as<float>() : 0.0f;

    out.has5hResetAt = obj["r5"].is<uint64_t>();
    out.reset5hEpochSec = out.has5hResetAt ? obj["r5"].as<uint64_t>() : 0;

    out.hasWeeklyUsagePct = obj["pw"].is<float>();
    out.usageWeeklyPct = out.hasWeeklyUsagePct ? obj["pw"].as<float>() : 0.0f;

    out.hasWeeklyResetAt = obj["rw"].is<uint64_t>();
    out.resetWeeklyEpochSec = out.hasWeeklyResetAt ? obj["rw"].as<uint64_t>() : 0;

    // 정상이면 맥이 키 자체를 생략한다(wire.rs 의 skip_serializing_if).
    out.hasQuotaError = obj["e"].is<uint8_t>();
    out.quotaErrorCode = out.hasQuotaError ? obj["e"].as<uint8_t>() : 0;

    return true;
}

}  // namespace

bool snapshotParse(const uint8_t *json, size_t len, Snapshot &out) {
    JsonDocument doc;
    if (deserializeJson(doc, json, len) != DeserializationError::Ok) {
        return false;  // 잘린 JSON, 문법 오류 등 — snapshot.h 문서의 첫 실패 조건.
    }

    if (!doc["v"].is<uint8_t>() || !doc["t"].is<uint64_t>() || !doc["a"].is<JsonArray>()) {
        return false;
    }

    Snapshot result;
    result.protocolVersion = doc["v"].as<uint8_t>();
    result.emittedAtEpochSec = doc["t"].as<uint64_t>();

    JsonArrayConst agents = doc["a"];
    result.agentCount = 0;
    result.agentsTruncated = false;
    for (JsonObjectConst ao : agents) {
        if (result.agentCount >= SNAPSHOT_MAX_AGENTS) {
            result.agentsTruncated = true;
            break;
        }
        SnapshotAgent a;
        if (!parseAgent(ao, a)) {
            return false;  // out 을 건드리지 않은 채 그대로 반환.
        }
        result.agents[result.agentCount++] = a;
    }

    out = result;
    return true;
}

// **영문만 쓴다.** 이 화면의 라벨은 전부 `lv_font_montserrat_14/16`
// (ui_cards.cpp)이고 이 펌웨어에는 한글 글리프가 들어 있는 폰트가 없다 —
// 한국어를 넣으면 렌더가 안 되고 네모로 깨진다. 맥·iOS 는 각자 한국어 문구를
// 고르고(코드만 공유한다), 여기서는 짧은 영문으로 320px 카드 폭에 맞춘다.
const char *snapshotQuotaErrorText(uint8_t code) {
    switch (code) {
        case 1: return "LOGIN REQUIRED";
        case 2: return "CLI ERROR";
        case 3: return "TIMEOUT";
        default: return "QUOTA ERROR";  // 4(기타) 및 이 펌웨어가 모르는 코드
    }
}
