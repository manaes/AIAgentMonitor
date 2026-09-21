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

    public override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        setUpCamera()
        if isStandalone { setUpCloseButton() }
    }

    public override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        guard !captureSession.isRunning else { return }
        DispatchQueue.global(qos: .userInitiated).async { [captureSession] in
            captureSession.startRunning()
        }
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

    /// 스캔 잠금을 푼다. 한 번 성공하면 콜백을 더 이상 내지 않는데(카메라는 같은 코드를
    /// 초당 여러 번 던지므로 필요한 잠금이다), 페어링이 실패해 **같은 화면에서** 다시
    /// 스캔해야 하는 경우에는 호출부가 직접 풀어줘야 한다.
    public func resumeScanning() {
        didScan = false
    }

    private func setUpCamera() {
        guard let device = AVCaptureDevice.default(for: .video),
              let input = try? AVCaptureDeviceInput(device: device),
              captureSession.canAddInput(input) else { return }
        captureSession.addInput(input)

        let output = AVCaptureMetadataOutput()
        guard captureSession.canAddOutput(output) else { return }
        captureSession.addOutput(output)
        output.setMetadataObjectsDelegate(self, queue: .main)
        output.metadataObjectTypes = [.qr]

        let layer = AVCaptureVideoPreviewLayer(session: captureSession)
        layer.videoGravity = .resizeAspectFill
        layer.frame = view.bounds
        view.layer.addSublayer(layer)
        previewLayer = layer
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
