import BLETransport
import Combine
import SnapKit
import UIKit

/// 1단계 확인용 화면. 2단계에서 실제 미러 UI 로 교체된다.
/// 목적은 단 하나 — 실기기에서 스냅샷 JSON 이 실제로 흐르는지 눈으로 보는 것.
final class RawDumpViewController: UIViewController {
    private let client = BLEClient()
    private var cancellables = Set<AnyCancellable>()
    private var received = 0

    private let statusLabel = UILabel()
    private let counterLabel = UILabel()
    private let textView = UITextView()

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "BLE 미러 (raw)"
        view.backgroundColor = .systemBackground

        statusLabel.font = .preferredFont(forTextStyle: .headline)
        statusLabel.text = ConnectionState.idle.label
        counterLabel.font = .preferredFont(forTextStyle: .caption1)
        counterLabel.textColor = .secondaryLabel
        counterLabel.text = "수신 0건"

        textView.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        textView.isEditable = false
        textView.backgroundColor = .secondarySystemBackground
        textView.layer.cornerRadius = 8

        [statusLabel, counterLabel, textView].forEach(view.addSubview)

        statusLabel.snp.makeConstraints { make in
            make.top.equalTo(view.safeAreaLayoutGuide).offset(16)
            make.leading.trailing.equalToSuperview().inset(16)
        }
        counterLabel.snp.makeConstraints { make in
            make.top.equalTo(statusLabel.snp.bottom).offset(4)
            make.leading.trailing.equalTo(statusLabel)
        }
        textView.snp.makeConstraints { make in
            make.top.equalTo(counterLabel.snp.bottom).offset(12)
            make.leading.trailing.equalToSuperview().inset(16)
            make.bottom.equalTo(view.safeAreaLayoutGuide).offset(-16)
        }

        client.state
            .receive(on: DispatchQueue.main)
            .sink { [weak self] in self?.statusLabel.text = $0.label }
            .store(in: &cancellables)

        client.rawMessages
            .receive(on: DispatchQueue.main)
            .sink { [weak self] json in
                guard let self else { return }
                self.received += 1
                self.counterLabel.text = "수신 \(self.received)건 · \(json.utf8.count) bytes"
                self.textView.text = json
            }
            .store(in: &cancellables)

        client.start()
    }
}
