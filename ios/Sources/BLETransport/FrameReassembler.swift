import Foundation

/// BLE notify 청크 재조립기.
///
/// Rust `src-tauri/src/ble/framing.rs` 의 `Reassembler` 와 **동작이 완전히 같아야 한다**.
/// 두 구현의 어긋남은 실기기에서만 드러나는 난해한 버그가 되므로
/// 골든 벡터(docs/ble-protocol/golden/frames-sample.json)로 양쪽을 묶어둔다.
///
/// 패킷 = [frame_id][chunk_idx][chunk_count][payload…]
public struct FrameReassembler {
    public static let headerLength = 3

    private var frameID: UInt8?
    private var expectedIndex: UInt8 = 0
    private var count: UInt8 = 0
    private var buffer = Data()
    /// 이 프레임을 이미 버렸는지(중간 구독·순서 이탈)
    private var aborted = false

    public init() {}

    /// 완성된 메시지를 만들면 반환한다.
    public mutating func push(_ packet: Data) -> Data? {
        guard packet.count >= Self.headerLength else { return nil }
        let bytes = [UInt8](packet)
        let (id, idx, total) = (bytes[0], bytes[1], bytes[2])
        guard total > 0 else { return nil }

        // 새 frame_id 이거나 0번 청크면 새로 시작한다.
        if frameID != id || idx == 0 {
            guard idx == 0 else {
                // 0번을 못 본 프레임은 통째로 버린다.
                frameID = id
                aborted = true
                return nil
            }
            frameID = id
            expectedIndex = 0
            count = total
            buffer.removeAll(keepingCapacity: true)
            aborted = false
        }

        guard !aborted, idx == expectedIndex, total == count else {
            aborted = true
            return nil
        }

        buffer.append(contentsOf: bytes[Self.headerLength...])
        expectedIndex &+= 1

        if expectedIndex == count {
            let done = buffer
            buffer = Data()
            frameID = nil
            return done
        }
        return nil
    }
}
