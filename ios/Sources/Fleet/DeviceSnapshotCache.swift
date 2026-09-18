import Foundation
import Wire
import os

public struct CachedSnapshot: Codable, Equatable, Sendable {
    public let snapshot: MirrorSnapshot
    public let fetchedAt: Date

    public init(snapshot: MirrorSnapshot, fetchedAt: Date) {
        self.snapshot = snapshot
        self.fetchedAt = fetchedAt
    }
}

/// 장치별 마지막 스냅샷. 앱 시작 시 probe 가 끝나기 전에도 목록을 채우는 용도라
/// 비밀이 아니고, 스트리밍 중 초당 갱신이라 Keychain 이 아닌 파일에 둔다.
public final class DeviceSnapshotCache {
    private static let logger = Logger(
        subsystem: "co.kr.wannypark.aiagentmonitor.multi", category: "DeviceSnapshotCache"
    )

    private let directory: URL

    public init(directory: URL) {
        self.directory = directory
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    }

    /// Application Support 아래 기본 위치.
    public static func defaultDirectory() -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return base.appendingPathComponent("DeviceSnapshots", isDirectory: true)
    }

    public func load(endpointIdHex: String) -> CachedSnapshot? {
        guard let url = fileURL(endpointIdHex) else { return nil }
        // 파일이 아직 없는 건 정상 상태라 여기서는 로그를 남기지 않는다.
        guard let data = try? Data(contentsOf: url) else { return nil }
        do {
            return try JSONDecoder().decode(CachedSnapshot.self, from: data)
        } catch {
            // 디코딩 실패는 스키마 변경/파일 손상을 뜻하므로 진단 흔적을 남긴다.
            Self.logger.error(
                "캐시 디코딩 실패 endpointIdHex=\(endpointIdHex, privacy: .public) error=\(String(describing: error), privacy: .public)"
            )
            return nil
        }
    }

    public func save(_ snapshot: MirrorSnapshot, fetchedAt: Date, endpointIdHex: String) {
        guard let url = fileURL(endpointIdHex) else {
            Self.logger.error("캐시 파일명으로 쓸 수 없는 식별자")
            return
        }
        let cached = CachedSnapshot(snapshot: snapshot, fetchedAt: fetchedAt)
        guard let data = try? JSONEncoder().encode(cached) else {
            Self.logger.error("캐시 인코딩 실패 endpointIdHex=\(endpointIdHex, privacy: .public)")
            return
        }
        do {
            try data.write(to: url, options: .atomic)
        } catch {
            // 디스크 쓰기 실패(권한/용량 부족 등)가 조용히 사라지면 원인 추적이 불가능해진다.
            Self.logger.error(
                "캐시 파일 쓰기 실패 endpointIdHex=\(endpointIdHex, privacy: .public) error=\(String(describing: error), privacy: .public)"
            )
        }
    }

    /// `endpointIdHex` 가 그대로 파일명이 되므로 hex 문자만 허용한다 — 경로 조작 차단.
    private func fileURL(_ endpointIdHex: String) -> URL? {
        let allowed = CharacterSet(charactersIn: "0123456789abcdefABCDEF")
        guard !endpointIdHex.isEmpty,
              endpointIdHex.unicodeScalars.allSatisfy(allowed.contains) else { return nil }
        return directory.appendingPathComponent("\(endpointIdHex).json")
    }
}
