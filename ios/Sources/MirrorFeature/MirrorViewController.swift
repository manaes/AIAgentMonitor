import BLETransport
import Combine
import DesignSystem
// NetworkTransport(IrohLib)는 17.5+ 전용이다. 이 파일은 두 타깃(MirrorFeature=
// 전체지원, MirrorFeatureBLE=BLE 전용)이 같은 소스로 컴파일하는데, MirrorFeatureBLE
// 는 NETWORK_TRANSPORT 플래그가 꺼져 있어 이 import 자체가 빠진다 — Swift 는 모듈의
// 최소 배포 타깃이 임포트하는 쪽보다 높으면 import 문 자체를 거부하므로
// (`@available`/`@_weakLinked` 로는 못 돌아간다), 플래그로 아예 컴파일에서 빼는 게
// 유일한 방법이다(Project.swift 상단 주석 참고).
#if NETWORK_TRANSPORT
import NetworkTransport
#endif
import SnapKit
import UIKit
import Wire

/// Detail 창을 미러링하는 화면. 연결 상태를 항상 상단에 노출해
/// 화면이 비어 있을 때 원인이 미궁이 되지 않게 한다(스펙 7.3).
///
/// **맥에는 있지만 여기에 의도적으로 옮기지 않은 것** (2단계 범위 밖 — 3단계에서
/// "왜 빠졌지"를 다시 판단하지 않도록 여기에 적어 둔다):
///
/// - **탭 바 (Sessions / Devices)** — `Detail.svelte:36-47`. 맥은 에이전트
///   카드와 세션 목록 사이에 탭 바를 그리지만 미러에는 없다. Devices 는 맥 전용
///   설정(BLE 공유 토글·권한)이라 미러할 대상이 아니다. 남는 탭이 Sessions
///   하나뿐이면 탭 바는 아무 일도 하지 않는 장식이 되므로 탭 바째로 생략하고
///   세션 목록만 직접 그린다.
/// - **🔄 동기화 버튼** — `AgentCard.svelte:73-79`. 미러는 읽기 전용이라 맥의 상태를
///   바꾸는 조작을 두지 않는다.
///
/// 두 항목 모두 `docs/ble-protocol/DEVICE-TEST.md` §6(범위 밖)에도 적혀 있다.
@MainActor
public final class MirrorViewController: UIViewController {

    /// 연결 방식. 우상단 설정 버튼에서 사용자가 고른다 — 켜는 순간 BLE/네트워크
    /// 중 하나를 고르는 macOS `DevicePanel` 의 "공유" 토글과 대칭이다.
    /// BLE 전용 빌드(MirrorFeatureBLE)에는 `.network` 케이스 자체가 없다.
    public enum TransportKind: Equatable {
        case ble
        #if NETWORK_TRANSPORT
        case network
        #endif
    }

    /// 마지막으로 고른 전송 방식. 비밀이 아니라 UI 선호도일 뿐이라 Keychain 이
    /// 아니라 UserDefaults 에 둔다. 앱을 재시작해도 이전에 네트워크로
    /// 페어링했다면 다시 네트워크로 시작해 저장된 정보로 재연결을 시도하게 한다
    /// (사용자 확인 — 기본값 BLE 로 고정돼 있으면 재시도 자체가 안 됨).
    private static let transportPreferenceKey = "mirror.transport.preference"

    public static var preferredTransport: TransportKind {
        #if NETWORK_TRANSPORT
        UserDefaults.standard.string(forKey: transportPreferenceKey) == "network" ? .network : .ble
        #else
        .ble
        #endif
    }

    private static func persistTransportPreference(_ kind: TransportKind) {
        #if NETWORK_TRANSPORT
        UserDefaults.standard.set(kind == .network ? "network" : "ble", forKey: transportPreferenceKey)
        #else
        UserDefaults.standard.set("ble", forKey: transportPreferenceKey)
        #endif
    }

    private let bleClient: BLEClient
    #if NETWORK_TRANSPORT
    private let networkClient: NetworkClient
    #endif
    private var activeKind: TransportKind
    private var client: any MirrorTransport {
        switch activeKind {
        case .ble: return bleClient
        #if NETWORK_TRANSPORT
        case .network: return networkClient
        #endif
        }
    }
    private var cancellables = Set<AnyCancellable>()
    private var tick: Timer?
    private var latest: MirrorSnapshot?
    /// 페어링이 필요한 동안(needsPairing/pairingFailed) modal 로 띄워둔 화면.
    /// nil 이 아니면 이미 떠 있다는 뜻 — 상태가 반복해서 같은 갈래로 와도 다시
    /// present 하지 않고 남은 시도 문구만 갱신한다. 전송마다 다른 화면을 띄우므로
    /// (BLE=코드 입력, 네트워크=QR 스캔) 각각 따로 추적한다.
    private weak var pairingViewController: PairingViewController?
    private weak var qrScannerViewController: QRScannerViewController?

    private let statusLabel = UILabel()
    private let settingsButton = UIButton(type: .system)
    private let scrollView = UIScrollView()
    private let contentStack = UIStackView()
    /// 카드 개수는 스냅샷에 실린 에이전트 수만큼이라 고정이 아니다. 세션 목록과
    /// 마찬가지로 1Hz 로 계속 들어오므로 매번 새로 만들지 않고 풀에서 재사용한다.
    private let agentsStack = UIStackView()
    private var agentCards: [AgentCardView] = []
    private let sessionList = SessionListView()

    /// 테스트 전용 창구. 다른 뷰들과 같은 규약을 따른다 — 전부 읽기 전용이고,
    /// 하위 뷰 자체가 아니라 화면에 실제로 나간 값만 노출한다.
    public var statusText: String? { statusLabel.text }
    /// 지금 화면에 보이는 카드 수(숨겨진 풀 슬롯 제외).
    public var visibleAgentCardCount: Int { agentCards.filter { !$0.isHidden }.count }
    /// 재사용 풀에 만들어진 카드의 총 개수(보이는 것 + 숨은 것).
    public var pooledAgentCardCount: Int { agentCards.count }
    /// 보이는 카드들 중 index 번째의 에이전트 이름. 표시 순서 검증용.
    public func visibleAgentCardName(at index: Int) -> String? {
        let visible = agentCards.filter { !$0.isHidden }
        guard index < visible.count else { return nil }
        return visible[index].nameText
    }
    /// 보이는 카드들 중 index 번째의 카운트다운 문구(없으면 nil). 주입한 `now` 가
    /// 카드 안쪽까지 전달되는지 확인하는 용도.
    public func visibleAgentCardCountdown(at index: Int) -> String? {
        let visible = agentCards.filter { !$0.isHidden }
        guard index < visible.count else { return nil }
        return visible[index].countdownText
    }
    /// 세션 목록에 실제로 그려진 행 수와 내용.
    public var sessionRowCount: Int { sessionList.rowCount }
    public func sessionRowText(at index: Int) -> String? { sessionList.rowText(at: index) }

    #if NETWORK_TRANSPORT
    public init(bleClient: BLEClient, networkClient: NetworkClient, initialTransport: TransportKind = .ble) {
        self.bleClient = bleClient
        self.networkClient = networkClient
        self.activeKind = initialTransport
        super.init(nibName: nil, bundle: nil)
    }
    #else
    public init(bleClient: BLEClient, initialTransport: TransportKind = .ble) {
        self.bleClient = bleClient
        self.activeKind = initialTransport
        super.init(nibName: nil, bundle: nil)
    }
    #endif

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    public override func viewDidLoad() {
        super.viewDidLoad()
        // 이 화면 하나뿐이라 네비게이션바(뒤로가기·타이틀 모두 안 씀)는 그냥 공간만
        // 차지하는 장식이다 — 통째로 숨기고, 연결 상태는 예전처럼 화면 좌상단에
        // 작은 라벨로 직접 그린다.
        navigationController?.setNavigationBarHidden(true, animated: false)

        statusLabel.font = Typography.body
        statusLabel.textColor = Palette.subtle
        statusLabel.text = ConnectionState.idle.label

        settingsButton.setImage(UIImage(systemName: "gearshape"), for: .normal)
        settingsButton.tintColor = Palette.subtle
        settingsButton.addTarget(self, action: #selector(settingsTapped), for: .touchUpInside)

        // app.css:17 `.window-root { background: #1c1c1e }`.
        view.backgroundColor = Palette.windowBackground

        // Detail.svelte:70 `.window-root { gap: 12px }` — 에이전트 블록과 세션 목록
        // 사이 간격. 카드끼리의 간격(agentsStack)은 Detail.svelte:71 `.agents { gap: 8px }`
        // 로 서로 다른 값이다 — 하나로 합치면 둘 중 하나가 원본과 어긋난다.
        contentStack.axis = .vertical
        contentStack.spacing = 12

        agentsStack.axis = .vertical
        agentsStack.spacing = 8

        view.addSubview(statusLabel)
        view.addSubview(settingsButton)
        view.addSubview(scrollView)
        scrollView.addSubview(contentStack)
        [agentsStack, sessionList].forEach(contentStack.addArrangedSubview)

        statusLabel.snp.makeConstraints { make in
            make.top.equalTo(view.safeAreaLayoutGuide).offset(12)
            make.leading.equalTo(view.safeAreaLayoutGuide).offset(16)
        }
        settingsButton.snp.makeConstraints { make in
            make.centerY.equalTo(statusLabel)
            make.trailing.equalTo(view.safeAreaLayoutGuide).offset(-16)
        }
        scrollView.snp.makeConstraints { make in
            make.top.equalTo(statusLabel.snp.bottom).offset(12)
            make.leading.trailing.equalTo(view.safeAreaLayoutGuide)
            make.bottom.equalTo(view.safeAreaLayoutGuide)
        }
        contentStack.snp.makeConstraints { make in
            make.edges.equalToSuperview().inset(UIEdgeInsets(top: 0, left: 16, bottom: 16, right: 16))
            make.width.equalTo(scrollView).offset(-32)
        }

        bind(to: client)

        // 카운트다운과 상대 시각은 클라이언트가 계산하므로 추가 전송 없이 1초마다 다시 그린다.
        // 클로저가 self 를 약하게만 잡으므로 타이머가 컨트롤러의 수명을 늘리지 않는다.
        tick = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.render(now: Date()) }
        }

        client.start()
    }

    deinit {
        tick?.invalidate()
    }

    /// 현재 활성 전송의 퍼블리셔를 구독한다. 전송을 바꿀 때(`switchTransport`)
    /// 이전 구독을 전부 걷어내고 다시 부른다 — `cancellables` 를 비우지 않으면
    /// 옛 전송의 콜백이 새 전송과 함께 계속 화면을 건드린다.
    private func bind(to transport: any MirrorTransport) {
        cancellables.removeAll()
        transport.state
            .receive(on: DispatchQueue.main)
            .sink { [weak self] state in
                self?.statusLabel.text = state.label
                self?.updatePairingPresentation(for: state)
            }
            .store(in: &cancellables)

        transport.snapshots
            .receive(on: DispatchQueue.main)
            .sink { [weak self] snap in
                self?.configure(snapshot: snap, now: Date())
            }
            .store(in: &cancellables)
    }

    @objc private func settingsTapped() {
        let sheet = UIAlertController(title: "연결 방식", message: nil, preferredStyle: .actionSheet)
        sheet.addAction(UIAlertAction(title: "BLE" + (activeKind == .ble ? " ✓" : ""), style: .default) { [weak self] _ in
            self?.switchTransport(to: .ble)
        })
        #if NETWORK_TRANSPORT
        sheet.addAction(UIAlertAction(title: "네트워크" + (activeKind == .network ? " ✓" : ""), style: .default) { [weak self] _ in
            self?.switchTransport(to: .network)
        })
        #endif
        sheet.addAction(UIAlertAction(title: "취소", style: .cancel))
        // iPad 는 액션시트를 팝오버로 띄우므로 앵커가 없으면 그 자리에서 크래시한다.
        sheet.popoverPresentationController?.sourceView = settingsButton
        sheet.popoverPresentationController?.sourceRect = settingsButton.bounds
        present(sheet, animated: true)
    }

    private func switchTransport(to kind: TransportKind) {
        guard kind != activeKind else { return }
        client.stop()
        dismissPairingIfPresented()
        activeKind = kind
        Self.persistTransportPreference(kind)
        bind(to: client)
        switch kind {
        case .ble:
            bleClient.start()
        #if NETWORK_TRANSPORT
        case .network:
            // start() 는 저장된 페어링 정보로 조용히 재연결을 시도해 카메라
            // 화면이 아예 안 뜰 수 있다 — 설정에서 명시적으로 "네트워크" 를
            // 고르는 건 항상 새로 페어링하겠다는 뜻이므로 정보를 지우고
            // QR 스캐너를 다시 띄운다(사용자 확인).
            networkClient.resetPairing()
        #endif
        }
    }

    public override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        updateAgentsLayout(for: view.bounds.size)
    }

    /// 가로모드에서는 카드가 한 줄에 나란히 보이도록 `agentsStack` 을 가로 축으로
    /// 바꾼다 — 세로모드처럼 쌓으면 화면 높이가 좁아진 가로모드에서 스크롤 없이는
    /// 첫 카드조차 다 안 보인다. `scrollView` 가 이미 safeAreaLayoutGuide 에 물려
    /// 있어(노치·홈 인디케이터) 나뉜 카드도 safe area 를 벗어나지 않는다.
    ///
    /// 카드 수는 고정이 아니다 — 맥의 설정 탭에서 표시할 에이전트를 고르면 스냅샷에
    /// 그만큼만 실려 온다. `.fillEqually` 는 숨겨진 arranged subview 를 레이아웃에서
    /// 제외하므로 보이는 카드끼리만 폭을 나눠 갖는다(오른쪽 여백이 남지 않는다).
    private func updateAgentsLayout(for size: CGSize) {
        let isLandscape = size.width > size.height
        let axis: NSLayoutConstraint.Axis = isLandscape ? .horizontal : .vertical
        guard agentsStack.axis != axis else { return }
        agentsStack.axis = axis
        agentsStack.distribution = isLandscape ? .fillEqually : .fill
    }

    /// 스냅샷 하나를 화면에 반영한다. 하위 세 뷰(`AgentCardView`·`SessionRowView`·
    /// `SessionListView`)와 같은 `configure(…, now:)` 규약을 쓴다 — `now` 를 주입해
    /// 카운트다운·상대 시각이 걸린 조립 전체를 BLE 없이 그대로 구동할 수 있다.
    public func configure(snapshot: MirrorSnapshot, now: Date) {
        latest = snapshot
        render(now: now)
    }

    private func render(now: Date) {
        guard let snap = latest else { return }

        // Detail.svelte:27-29 는 스냅샷에 실린 에이전트마다 카드를 하나씩 그린다 —
        // claude/codex 두 종류로 못박지 않는다. 다만 매 프레임 순서가 흔들리면
        // 카드가 화면에서 자리를 바꾸므로, claude → codex → 그 외(스냅샷 순서)
        // 순으로 고정한다.
        let ordered = orderedForDisplay(snap.agents)

        while agentCards.count < ordered.count {
            let card = AgentCardView()
            agentCards.append(card)
            agentsStack.addArrangedSubview(card)
        }

        for (i, card) in agentCards.enumerated() {
            if i < ordered.count {
                card.isHidden = false
                card.configure(agent: ordered[i], now: now)
            } else {
                card.isHidden = true
            }
        }

        sessionList.configure(snapshot: snap, now: now)
    }

    private func orderedForDisplay(_ agents: [MirrorAgent]) -> [MirrorAgent] {
        let claude = agents.filter { $0.kind == .claude }
        let codex = agents.filter { $0.kind == .codex }
        let others = agents.filter { $0.kind != .claude && $0.kind != .codex }
        return claude + codex + others
    }

    /// `updatePairingPresentation` 이 실제로 UIKit 을 부르기 전에 거치는 순수 결정.
    /// `present`/`dismiss` 는 호스트 없는 로직 테스트 번들에서 신뢰성 있게 검증하기
    /// 어렵지만(macOS `pump()`/`targets_for` 때와 같은 사각지대), 어떤 상태가 present/
    /// dismiss/무시로 이어지는지는 CoreBluetooth·UIKit 없이 그대로 테스트할 수 있다.
    enum PairingPresentationAction: Equatable {
        case present(attemptsRemaining: Int?)
        case dismiss
        case none
    }

    /// 페어링이 필요한 상태(코드 대기·시도 실패)면 띄우고, 연결이 완성돼도(streaming),
    /// 그 사이 연결이 끊겨도(연결 끊김·블루투스 꺼짐·대기·버전 불일치) 닫는다 —
    /// 전체 브랜치 리뷰 I-3: 시트가 떠 있는 동안 Mac 이 공유를 끄거나 범위를
    /// 벗어나면, 시트는 `isModalInPresentation = true` 라 스와이프로도 못 닫히고
    /// 뒤에 가려진 상태 라벨도 안 보여서 사용자가 이유를 알 방법이 없었다.
    /// 재연결되면 필요할 때 다시 뜬다(`.needsPairing`/`.pairingFailed` 재도달).
    static func pairingAction(for state: ConnectionState) -> PairingPresentationAction {
        switch state {
        case .needsPairing:
            return .present(attemptsRemaining: nil)
        case .pairingFailed(let left):
            return .present(attemptsRemaining: left)
        case .streaming, .disconnected, .bluetoothOff, .idle, .versionMismatch:
            return .dismiss
        case .scanning, .connecting:
            return .none
        }
    }

    private func updatePairingPresentation(for state: ConnectionState) {
        switch Self.pairingAction(for: state) {
        case .present(let attemptsRemaining):
            presentPairingIfNeeded(attemptsRemaining: attemptsRemaining)
        case .dismiss:
            dismissPairingIfPresented()
        case .none:
            break
        }
    }

    private func presentPairingIfNeeded(attemptsRemaining: Int?) {
        switch activeKind {
        case .ble:
            if let existing = pairingViewController {
                existing.setAttemptsRemaining(attemptsRemaining)
                return
            }
            guard qrScannerViewController == nil else { return }
            let vc = PairingViewController()
            vc.setAttemptsRemaining(attemptsRemaining)
            vc.onSubmit = { [weak bleClient] code in bleClient?.submitPairingCode(code) }
            vc.modalPresentationStyle = .formSheet
            vc.isModalInPresentation = true
            pairingViewController = vc
            present(vc, animated: true)
        #if NETWORK_TRANSPORT
        case .network:
            // QR 은 스캔 한 번으로 코드까지 자동 제출하므로(NetworkClient.pair),
            // 시도 소진(attemptsRemaining)이 와도 새 QR 을 다시 스캔하는 것 외에
            // 화면에서 딱히 더 보여줄 게 없다 — 스캐너를 다시 띄운다.
            guard pairingViewController == nil, qrScannerViewController == nil else { return }
            let vc = QRScannerViewController()
            vc.onScan = { [weak networkClient] payload in networkClient?.pair(qrPayload: payload) }
            vc.modalPresentationStyle = .fullScreen
            qrScannerViewController = vc
            present(vc, animated: true)
        #endif
        }
    }

    private func dismissPairingIfPresented() {
        if pairingViewController != nil {
            pairingViewController = nil
            dismiss(animated: true)
        }
        if qrScannerViewController != nil {
            qrScannerViewController = nil
            dismiss(animated: true)
        }
    }
}
