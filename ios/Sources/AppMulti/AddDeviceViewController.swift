import DesignSystem
import Fleet
import Foundation
import NetworkTransport
import UIKit

/// QR 스캔 → 이름 확인 → **코드로 연결해 토큰 발급** → 레지스트리 저장. 저장까지 끝나면
/// `onAdded` 로 알린다.
///
/// 스펙 6.3 의 "저장 → 즉시 연결" 을 "연결 → 저장" 으로 한 칸 당긴 것 — 저장해야 할 토큰이
/// 연결의 결과물이기 때문이다(Ruling 16). QR 에 실려오는 `code` 는 6자리 페어링 코드일 뿐
/// 토큰이 아니라서, 그걸 `Device.token` 에 넣으면 재연결이 항상 needsPairing 으로 거부된다.
final class AddDeviceViewController: UIViewController {
    private let registry: DeviceRegistry
    var onAdded: ((Device) -> Void)?

    private let scanner = QRScannerViewController()
    /// 카메라는 같은 코드를 초당 여러 번 던진다. 한 번 처리를 시작하면 흐름이 끝나거나
    /// 실패로 되돌아올 때까지 잠근다.
    private var hasHandledScan = false

    init(registry: DeviceRegistry) {
        self.registry = registry
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "장치 추가"
        view.backgroundColor = Palette.windowBackground
        navigationItem.leftBarButtonItem = UIBarButtonItem(
            title: "취소", style: .plain, target: self, action: #selector(cancelTapped)
        )

        // addChild 를 view 접근보다 먼저 한다 — 스캐너는 viewDidLoad 에서 parent 로
        // "embed 됐는지" 를 판별해 닫기 버튼과 자체 dismiss 를 끈다.
        addChild(scanner)
        view.addSubview(scanner.view)
        scanner.view.frame = view.bounds
        scanner.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        scanner.didMove(toParent: self)

        scanner.onScan = { [weak self] payload in
            guard let self, !self.hasHandledScan else { return }
            self.hasHandledScan = true
            self.handle(payload)
        }
    }

    @objc private func cancelTapped() {
        dismiss(animated: true)
    }

    /// 실패해서 같은 화면에서 다시 스캔할 수 있게 되돌린다. 스캐너 자체의 잠금도 같이
    /// 풀지 않으면 콜백이 영영 다시 오지 않는다.
    private func resumeScanning() {
        hasHandledScan = false
        scanner.unlockScanResult()
    }

    private func handle(_ payload: String) {
        guard let parsed = NetworkClient.parseQrPayload(payload) else {
            present(alert("QR 코드를 인식하지 못했습니다") { [weak self] in
                self?.resumeScanning()
            }, animated: true)
            return
        }
        askForName(parsed: parsed)
    }

    private func askForName(parsed: NetworkClient.ParsedPairingPayload) {
        // 이미 등록된 장치를 재스캔한 경우엔 사용자가 붙여 둔 이름을 먼저 보여준다.
        // Mac 호스트명을 채우면 "내가 지은 이름이 사라졌다"로 보이고, 그대로 확인을 누르면
        // 그 호스트명이 이름이 된다(upsert 는 들어온 이름이 이긴다).
        let existing = (try? registry.load())?.first { $0.endpointIdHex == parsed.endpointIdHex }
        let defaultName = existing?.userLabel ?? parsed.macName ?? ""
        let sheet = UIAlertController(
            title: "장치 이름", message: "목록에 표시할 이름입니다.", preferredStyle: .alert
        )
        sheet.addTextField { field in
            field.text = defaultName
            field.placeholder = "예: 작업실 맥"
        }
        sheet.addAction(UIAlertAction(title: "취소", style: .cancel) { [weak self] _ in
            self?.resumeScanning()
        })
        sheet.addAction(UIAlertAction(title: "추가", style: .default) { [weak self] _ in
            let typed = sheet.textFields?.first?.text?.trimmingCharacters(in: .whitespaces)
            self?.pairAndSave(parsed: parsed, userLabel: (typed?.isEmpty == false) ? typed : nil)
        })
        present(sheet, animated: true)
    }

    /// 페어링 타임아웃. probe 의 3초는 "이미 아는 장치가 켜져 있나" 용이고, 페어링은 사용자가
    /// Mac 앞에서 기다리는 1회성 작업이라 hole-punch 가 느린 네트워크를 더 참아준다.
    private static let pairingTimeoutSeconds: Double = 10

    private func pairAndSave(parsed: NetworkClient.ParsedPairingPayload, userLabel: String?) {
        // 최대 10초 떠 있는 알림이다. 스피너가 없으면 멈춘 화면과 구별되지 않는다.
        let waiting = UIAlertController(
            title: nil, message: "\nMac 에 연결하는 중…\n\n", preferredStyle: .alert
        )
        let spinner = UIActivityIndicatorView(style: .medium)
        spinner.translatesAutoresizingMaskIntoConstraints = false
        spinner.startAnimating()
        waiting.view.addSubview(spinner)
        NSLayoutConstraint.activate([
            spinner.centerXAnchor.constraint(equalTo: waiting.view.centerXAnchor),
            spinner.bottomAnchor.constraint(equalTo: waiting.view.bottomAnchor, constant: -20),
        ])
        present(waiting, animated: true)
        Task { [weak self] in
            guard let self else { return }
            let outcome: Result<String, Error>
            do {
                let client = NetworkClient(endpointProvider: .shared)
                let result = try await client.probe(
                    endpointIdHex: parsed.endpointIdHex,
                    relayUrl: parsed.relayUrl,
                    addresses: parsed.addresses,
                    timeoutSeconds: Self.pairingTimeoutSeconds,
                    code: parsed.code
                )
                // 페어링용 연결은 여기서 닫는다. fleet 이 저장된 토큰으로 다시 붙는다 —
                // hole-punch 가 한 번 더 들지만 페어링은 장치당 한 번이다.
                try? result.connection.close(errorCode: 0, reason: Data())
                guard let token = result.issuedToken else {
                    // 프로토콜상 도달 불가능한 방어 가드다. 코드 페어링은 `.ephemeral` 슬롯을
                    // 쓰고 그 슬롯은 load 가 항상 nil 이라 언제나 HELLO2 로 시작하는데,
                    // issuedToken 이 nil 인 `.openSession` 은 PROOF2 를 보낸 뒤에만 나온다.
                    // 그래도 남겨 둔다 — 비용이 0 이고, 토큰 없이 저장하면 그 장치는 영영
                    // needsPairing 으로만 거부된다.
                    throw NetworkClientError.needsPairing
                }
                outcome = .success(token)
            } catch {
                outcome = .failure(error)
            }
            waiting.dismiss(animated: true) { [weak self] in
                guard let self else { return }
                switch outcome {
                case .success(let token):
                    self.save(parsed: parsed, token: token, userLabel: userLabel)
                case .failure:
                    self.present(
                        self.alert("Mac 화면의 코드가 만료됐거나 연결할 수 없습니다. QR 을 다시 스캔하세요") {
                            [weak self] in self?.resumeScanning()
                        },
                        animated: true
                    )
                }
            }
        }
    }

    private func save(parsed: NetworkClient.ParsedPairingPayload, token: String, userLabel: String?) {
        let device = Device(
            endpointIdHex: parsed.endpointIdHex,
            token: token,   // Mac 이 발급한 토큰. QR 의 code 는 여기 오기 전에 소비됐다.
            relayUrl: parsed.relayUrl,
            addresses: parsed.addresses,
            macHostname: parsed.macName,
            userLabel: userLabel,
            sortIndex: 0    // upsert 가 max+1 로 덮어쓴다
        )
        do {
            let saved = try registry.upsert(device)
            // 로컬 device 가 아니라 **저장된** 값을 넘긴다 — sortIndex 는 upsert 가 매기고
            // userLabel 도 병합을 거친 뒤라야 목록과 같은 값이 된다.
            onAdded?(saved.first { $0.endpointIdHex == device.endpointIdHex } ?? device)
            dismiss(animated: true)
        } catch DeviceRegistryError.deviceLimitReached {
            present(alert("장치는 최대 \(DeviceRegistry.maxDevices)대까지 추가할 수 있습니다") { [weak self] in
                self?.resumeScanning()
            }, animated: true)
        } catch DeviceRegistryError.corrupted {
            // 목록 화면(Ruling 18)과 같은 말을 해야 한다 — 같은 사고를 두 가지로 설명하면
            // 사용자는 둘 중 하나를 다른 문제로 읽는다.
            present(
                alert("\(DeviceRegistryError.corruptedTitle)\n\n\(DeviceRegistryError.corruptedAdvice)") {
                    [weak self] in self?.resumeScanning()
                },
                animated: true
            )
        } catch {
            present(alert("저장하지 못했습니다: \(error.localizedDescription)") { [weak self] in
                self?.resumeScanning()
            }, animated: true)
        }
    }

    private func alert(_ message: String, onDismiss: @escaping () -> Void) -> UIAlertController {
        let controller = UIAlertController(title: nil, message: message, preferredStyle: .alert)
        controller.addAction(UIAlertAction(title: "확인", style: .default) { _ in onDismiss() })
        return controller
    }
}
