import XCTest
@testable import BLETransport

final class FrameReassemblerTests: XCTestCase {

    private func packet(_ bytes: [UInt8]) -> Data { Data(bytes) }

    func testSingleChunkFrame() {
        var r = FrameReassembler()
        XCTAssertEqual(r.push(packet([0, 0, 1, 0x41, 0x42])), Data([0x41, 0x42]))
    }

    func testMultiChunkFrame() {
        var r = FrameReassembler()
        XCTAssertNil(r.push(packet([7, 0, 2, 0x41])))
        XCTAssertEqual(r.push(packet([7, 1, 2, 0x42])), Data([0x41, 0x42]))
    }

    func testDiscardsFrameWhenSubscribedMidStream() {
        var r = FrameReassembler()
        XCTAssertNil(r.push(packet([7, 1, 3, 0x42])), "0번을 못 봤으면 완성되면 안 된다")
        XCTAssertNil(r.push(packet([7, 2, 3, 0x43])))
    }

    func testNewFrameIdDiscardsIncompletePrevious() {
        var r = FrameReassembler()
        XCTAssertNil(r.push(packet([1, 0, 3, 0xAA])))
        XCTAssertNil(r.push(packet([1, 1, 3, 0xAA])))
        XCTAssertEqual(r.push(packet([2, 0, 1, 0xBB])), Data([0xBB]))
    }

    func testOutOfOrderChunkDiscardsFrame() {
        var r = FrameReassembler()
        XCTAssertNil(r.push(packet([5, 0, 3, 0xC1])))
        XCTAssertNil(r.push(packet([5, 2, 3, 0xC3])), "순서 이탈이면 폐기")
        XCTAssertNil(r.push(packet([5, 3, 3, 0xC4])), "폐기 후에는 계속 무시")
    }

    func testTooShortPacketIsIgnored() {
        var r = FrameReassembler()
        XCTAssertNil(r.push(packet([1, 2])))
    }

    /// chunk_count 0 은 송신 측 청킹이 255 경계를 넘겨 u8 로 잘렸을 때 나오는 값이다.
    /// 이걸 받아들이면 절대 완성되지 않는 프레임을 무한히 붙잡는다.
    func testZeroChunkCountIsRejected() {
        var r = FrameReassembler()
        XCTAssertNil(r.push(packet([1, 0, 0, 0x41])), "chunk_count 0 은 유효하지 않다")
        // 정상 프레임은 그대로 받아야 한다(폐기 상태가 남으면 안 된다)
        XCTAssertEqual(r.push(packet([1, 0, 1, 0x42])), Data([0x42]))
    }

    /// Rust framing.rs 가 생성한 프레임을 그대로 재조립할 수 있어야 한다.
    /// 이 테스트가 언어 간 프레이밍 불일치를 잡는 유일한 안전장치다.
    func testGoldenVectorsRoundTrip() throws {
        struct Golden: Decodable {
            let chunk_size: Int
            let frame_id: UInt8
            let message: String
            let frames: [String]
        }
        let url = try XCTUnwrap(
            Bundle(for: Self.self).url(forResource: "frames-sample", withExtension: "json"),
            "골든 벡터가 테스트 번들에 없다"
        )
        let golden = try JSONDecoder().decode(Golden.self, from: Data(contentsOf: url))

        var r = FrameReassembler()
        var out: Data?
        for hex in golden.frames {
            var bytes = [UInt8]()
            var idx = hex.startIndex
            while idx < hex.endIndex {
                let next = hex.index(idx, offsetBy: 2)
                bytes.append(UInt8(hex[idx..<next], radix: 16)!)
                idx = next
            }
            XCTAssertLessThanOrEqual(bytes.count, golden.chunk_size, "청크가 한계를 넘었다")
            XCTAssertEqual(bytes[0], golden.frame_id)
            if let msg = r.push(Data(bytes)) { out = msg }
        }
        let decoded = try XCTUnwrap(out.flatMap { String(data: $0, encoding: .utf8) })
        XCTAssertEqual(decoded, golden.message, "Rust 가 만든 프레임을 Swift 가 복원하지 못했다")
    }
}
