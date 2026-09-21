import AVFoundation
import UIKit

/// Mac 화면의 페어링 QR을 스캔해 원문 문자열을 콜백으로 넘긴다. 이 화면은 순수
/// 카메라 UI일 뿐 — 페이로드 파싱/인증/연결은 `NetworkClient.pair(qrPayload:)`
/// 책임이다(BLE `PairingViewController`가 코드 검증을 하지 않는 것과 같은 분리).
public final class QRScannerViewController: UIViewController {
    public var onScan: ((String) -> Void)?

    /// 모달로 직접 떠 있을 때만 스스로 닫고 닫기 버튼을 그린다. 자식으로 embed 된
    /// 경우(AppMulti 의 장치 추가 화면)에 `dismiss` 를 부르면 스캐너가 아니라 그걸 담고
    /// 있는 모달 전체가 닫혀, 스캔 직후 화면이 통째로 사라진다.
    private var isStandalone: Bool { parent == nil }

    private let captureSession = AVCaptureSession()
    private var previewLayer: AVCaptureVideoPreviewLayer?
    private var didScan = false
    /// 카메라를 못 얻었을 때 대신 보여주는 안내. 두 번 붙이지 않으려고 참조를 들고 있는다.
    private var guidanceView: UIView?

    public override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        setUpCamera()
        if isStandalone { setUpCloseButton() }
    }

    public override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        startCaptureSession()
    }

    public override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        guard captureSession.isRunning else { return }
        DispatchQueue.global(qos: .userInitiated).async { [captureSession] in
            captureSession.stopRunning()
        }
    }

    public override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        previewLayer?.frame = view.bounds
    }

    private func setUpCloseButton() {
        let button = UIButton(type: .system)
        button.setTitle("닫기", for: .normal)
        button.setTitleColor(.white, for: .normal)
        button.addTarget(self, action: #selector(closeTapped), for: .touchUpInside)
        view.addSubview(button)
        button.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            button.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 12),
            button.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -16),
        ])
    }

    @objc private func closeTapped() {
        dismiss(animated: true)
    }

    /// 스캔 결과 잠금만 푼다 — **캡처 세션을 재시작하지는 않는다.** 한 번 성공하면 콜백을
    /// 더 이상 내지 않는데(카메라는 같은 코드를 초당 여러 번 던지므로 필요한 잠금이다),
    /// 페어링이 실패해 **같은 화면에서** 다시 스캔해야 하는 경우에는 호출부가 직접 풀어준다.
    /// 세션이 멈춘 상태(뷰가 사라졌다 돌아온 경우)의 재시작은 `viewDidAppear` 담당이다.
    public func unlockScanResult() {
        didScan = false
    }

    private func setUpCamera() {
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .denied, .restricted:
            // AppMulti 에서는 이 화면이 장치를 추가하는 **유일한** 경로라, 검은 화면은
            // 막다른 길이 된다. 권한은 앱 안에서 되돌릴 수 없으니 설정으로 보낸다.
            showGuidance(message: "카메라 권한이 필요합니다", showsSettingsButton: true)
        case .notDetermined:
            // 여기서 직접 묻지 않으면 세션이 시작될 때 시스템이 묻고, 거부당하면
            // 안내 없이 검은 화면만 남는다.
            AVCaptureDevice.requestAccess(for: .video) { [weak self] granted in
                DispatchQueue.main.async {
                    guard let self else { return }
                    guard granted else {
                        self.showGuidance(message: "카메라 권한이 필요합니다", showsSettingsButton: true)
                        return
                    }
                    self.configureCaptureSession()
                    // 권한 대화상자를 닫은 시점엔 viewDidAppear 가 이미 지나갔다.
                    if self.view.window != nil { self.startCaptureSession() }
                }
            }
        default:
            configureCaptureSession()
        }
    }

    /// 입력·출력·프리뷰를 붙인다. 한 단계라도 실패하면 안내를 띄운다 — 조용히 돌아가면
    /// 사용자는 카메라가 준비되기를 영원히 기다린다.
    private func configureCaptureSession() {
        guard let device = AVCaptureDevice.default(for: .video),
              let input = try? AVCaptureDeviceInput(device: device),
              captureSession.canAddInput(input) else {
            showGuidance(message: "카메라를 사용할 수 없습니다", showsSettingsButton: false)
            return
        }
        captureSession.addInput(input)

        let output = AVCaptureMetadataOutput()
        guard captureSession.canAddOutput(output) else {
            showGuidance(message: "카메라를 사용할 수 없습니다", showsSettingsButton: false)
            return
        }
        captureSession.addOutput(output)
        output.setMetadataObjectsDelegate(self, queue: .main)
        output.metadataObjectTypes = [.qr]

        let layer = AVCaptureVideoPreviewLayer(session: captureSession)
        layer.videoGravity = .resizeAspectFill
        layer.frame = view.bounds
        view.layer.addSublayer(layer)
        previewLayer = layer
    }

    private func startCaptureSession() {
        // 입력이 없으면(권한 거부·카메라 없음) 시작할 것이 없다.
        guard !captureSession.inputs.isEmpty, !captureSession.isRunning else { return }
        DispatchQueue.global(qos: .userInitiated).async { [captureSession] in
            captureSession.startRunning()
        }
    }

    private func showGuidance(message: String, showsSettingsButton: Bool) {
        guard guidanceView == nil else { return }
        let stack = UIStackView()
        stack.axis = .vertical
        stack.alignment = .center
        stack.spacing = 12
        stack.translatesAutoresizingMaskIntoConstraints = false

        let label = UILabel()
        label.text = message
        label.textColor = .white
        label.textAlignment = .center
        label.numberOfLines = 0
        stack.addArrangedSubview(label)

        if showsSettingsButton {
            let button = UIButton(type: .system)
            button.setTitle("설정 열기", for: .normal)
            button.addTarget(self, action: #selector(openSettingsTapped), for: .touchUpInside)
            stack.addArrangedSubview(button)
        }

        view.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            stack.centerYAnchor.constraint(equalTo: view.centerYAnchor),
            stack.leadingAnchor.constraint(greaterThanOrEqualTo: view.leadingAnchor, constant: 24),
            stack.trailingAnchor.constraint(lessThanOrEqualTo: view.trailingAnchor, constant: -24),
        ])
        guidanceView = stack
    }

    @objc private func openSettingsTapped() {
        guard let url = URL(string: UIApplication.openSettingsURLString) else { return }
        UIApplication.shared.open(url)
    }
}

extension QRScannerViewController: AVCaptureMetadataOutputObjectsDelegate {
    public func metadataOutput(
        _ output: AVCaptureMetadataOutput,
        didOutput metadataObjects: [AVMetadataObject],
        from connection: AVCaptureConnection
    ) {
        guard !didScan,
              let object = metadataObjects.first as? AVMetadataMachineReadableCodeObject,
              object.type == .qr,
              let value = object.stringValue else { return }
        didScan = true
        onScan?(value)
        // embed 된 경우엔 닫지 않는다 — 호출부가 같은 화면에서 이름 입력·페어링을 이어간다.
        if isStandalone { dismiss(animated: true) }
    }
}
