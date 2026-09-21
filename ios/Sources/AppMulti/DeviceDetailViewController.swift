import DesignSystem
import Fleet
import SnapKit
import UIKit
import Wire

/// 장치 한 대의 세부 화면. 기존 1:1 앱의 세부 화면과 같은 구성이라 카드 렌더링은
/// `AgentCardView`, 세션 목록은 `SessionListView` 를 그대로 쓴다.
///
/// 이 화면에 들어와도 **다른 세션은 끊지 않는다**(스펙 §4.4) — 목록으로 돌아왔을 때
/// 즉시 최신값이 보여야 하고, 끊었다 다시 붙이면 probe 승격으로 아끼려던 비용을
/// 그대로 다시 낸다.
final class DeviceDetailViewController: UIViewController {
    private let session: DeviceSession
    private let fleet: DeviceFleet
    private let cache: DeviceSnapshotCache

    private let statusLabel = UILabel()
    private let scrollView = UIScrollView()
    private let cardStack = UIStackView()
    private let sessionList = SessionListView()
    /// 스냅샷은 스트리밍 중 초당 들어온다. 갱신마다 카드를 새로 만들면 뷰가 쌓이므로
    /// 1:1 앱 `MirrorViewController` 와 같이 풀에서 재사용하고 남는 건 숨긴다.
    private var agentCards: [AgentCardView] = []

    /// 디스크 캐시는 장치당 한 번만 읽는다 — `load` 가 메인 스레드에서 파일 읽기 + JSON
    /// 디코드를 하므로 갱신마다 부르면 안 된다. "읽었는데 없더라"도 기억해야 해서
    /// 옵셔널을 두 겹으로 둔다(목록 화면과 같은 방식).
    private var diskSnapshot: CachedSnapshot??
    /// 화면이 떠 있는 동안만 도는 1초 틱. 아래 viewWillAppear 의 주석 참고.
    private var tick: Timer?

    init(session: DeviceSession, fleet: DeviceFleet, cache: DeviceSnapshotCache) {
        self.session = session
        self.fleet = fleet
        self.cache = cache
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    deinit {
        tick?.invalidate()
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = session.device.displayName
        view.backgroundColor = Palette.windowBackground

        statusLabel.font = Typography.label
        statusLabel.textColor = Palette.subtle
        statusLabel.numberOfLines = 0

        cardStack.axis = .vertical
        cardStack.spacing = 12

        let root = UIStackView(arrangedSubviews: [statusLabel, cardStack, sessionList])
        root.axis = .vertical
        root.spacing = 12

        view.addSubview(scrollView)
        scrollView.addSubview(root)
        scrollView.snp.makeConstraints { $0.edges.equalTo(view.safeAreaLayoutGuide) }
        root.snp.makeConstraints { make in
            make.edges.equalToSuperview().inset(16)
            make.width.equalTo(scrollView).offset(-32)
        }

        // session.onChange 는 건드리지 않는다 — fleet 소유(캐시 쓰기·목록 갱신). 구독은 아래
        // viewWillAppear/viewWillDisappear 의 fleet.onSessionChange 로 한다(Ruling 23).
        // 첫 그리기는 viewWillAppear 가 한다 — push 되는 화면은 여기 다음에 반드시 그게
        // 불리므로, 여기서도 부르면 첫 표시에 render 가 두 번 돈다.

        // 오프라인 장치에 들어왔다면 사용자 의도가 명확하므로 즉시 1회 재시도한다(스펙 §6.2).
        // self 가 아니라 session 만 캡처한다 — 재시도는 화면을 닫아도 끝까지 도는 게 맞지만,
        // 그 3초 동안 화면을 붙들고 있을 이유는 없다.
        if session.status == .offline || session.status == .idle {
            Task { [session] in await session.retrigger() }
        }
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        fleet.onSessionChange = { [weak self] changed in
            // fleet 은 16대 전부의 변화를 이 훅으로 흘린다. 우리 세션만 골라낸다.
            guard let self, changed === self.session else { return }
            self.render()
        }
        render()

        // 카운트다운과 상대 시각("N분 전")은 스냅샷이 아니라 now 로 계산되므로, 추가 전송
        // 없이 1초마다 다시 그린다 — 1:1 앱 MirrorViewController 와 같은 이유·같은 패턴이다.
        // 이게 없으면 오프라인 장치의 신선도가 다음 재탐색(3분)까지 그대로 굳어서, 사용자는
        // "0분 전"이 몇 분째 안 움직이는 화면을 본다. 클로저가 self 를 약하게만 잡으므로
        // 타이머가 화면의 수명을 늘리지 않는다.
        //
        // 다시 걸기 전에 먼저 끊는다 — 스와이프 뒤로가기를 중간에 취소하면 viewWillDisappear
        // 다음에 viewWillAppear 가 한 번 더 오는데, 그냥 대입하면 옛 타이머가 무효화되지 않은
        // 채 런루프에 남아 초당 두 번씩 그리게 된다.
        tick?.invalidate()
        tick = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.render() }
        }
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        fleet.onSessionChange = nil
        tick?.invalidate()
        tick = nil
    }

    private func render() {
        let now = Date()
        let isLive = (session.status == .online)

        // 신선도는 **스냅샷을 받은 시각**으로 계산한다. 지금 시각을 쓰면 오프라인으로 떨어진
        // 장치가 들고 있는 낡은 스냅샷이 영원히 "방금 전"으로 보인다.
        let live = session.latest.flatMap { snapshot in
            session.latestAt.map { CachedSnapshot(snapshot: snapshot, fetchedAt: $0) }
        }
        let cached = live ?? loadDiskSnapshot()

        // 신선도 문구는 목록과 같은 함수에서 뽑는다 — 두 화면이 같은 장치를 다르게 말하면 안 된다.
        let row = DeviceListPresentation.row(
            device: session.device, status: session.status, cached: cached, now: now
        )
        statusLabel.text = [statusText(), row.freshnessText]
            .compactMap { $0 }
            .joined(separator: " · ")

        // 목록과 같은 순서로 쌓는다 — 두 화면에서 에이전트 자리가 뒤바뀌면 안 된다.
        let agents = DeviceListPresentation.orderedForDisplay(cached?.snapshot.agents ?? [])
        while agentCards.count < agents.count {
            let card = AgentCardView()
            agentCards.append(card)
            cardStack.addArrangedSubview(card)
        }
        for (index, card) in agentCards.enumerated() {
            if index < agents.count {
                card.isHidden = false
                // 오프라인이면 tok/s 같은 순간값은 감추고 한도 %와 막대는 남긴다(스펙 §7).
                card.configure(agent: agents[index], now: now, isLive: isLive)
            } else {
                card.isHidden = true
            }
        }

        if let snapshot = cached?.snapshot {
            sessionList.isHidden = false
            sessionList.configure(snapshot: snapshot, now: now)
        } else {
            // 받아본 스냅샷이 한 장도 없으면 빈 목록 대신 아무것도 보여주지 않는다.
            sessionList.isHidden = true
        }
    }

    private func loadDiskSnapshot() -> CachedSnapshot? {
        if let memo = diskSnapshot { return memo }
        let loaded = cache.load(endpointIdHex: session.device.endpointIdHex)
        diskSnapshot = loaded
        return loaded
    }

    /// 목록의 짧은 배지와 달리 화면 전체를 쓸 수 있으므로, 사용자가 다음에 뭘 해야 하는지까지 적는다.
    private func statusText() -> String {
        switch session.status {
        case .idle: return "대기"
        case .probing: return "확인 중"
        case .online: return "연결됨"
        case .unstable: return "재연결 중"
        case .offline: return "오프라인 — 마지막으로 받은 값"
        case .needsRepairing: return "재페어링이 필요합니다. QR 을 다시 스캔하세요."
        case .versionMismatch: return "Mac 앱 버전이 맞지 않습니다."
        }
    }
}
