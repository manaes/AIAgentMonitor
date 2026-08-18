import BLETransport
import Combine
import DesignSystem
import SnapKit
import UIKit
import Wire

/// Detail 창을 미러링하는 화면. 연결 상태를 항상 상단에 노출해
/// 화면이 비어 있을 때 원인이 미궁이 되지 않게 한다(스펙 7.3).
///
/// **맥에는 있지만 여기에 의도적으로 옮기지 않은 것** (2단계 범위 밖 — 3단계에서
/// "왜 빠졌지"를 다시 판단하지 않도록 여기에 적어 둔다):
///
/// - **탭 바 (Sessions / Triggers / Devices)** — `Detail.svelte:36-48`. 맥은 에이전트
///   카드와 세션 목록 사이에 탭 바를 그리지만 미러에는 없다. Triggers 는 BLE 전송
///   경로 자체가 아직 없고(3단계), Devices 는 맥 전용 설정(BLE 공유 토글·권한)이라
///   미러할 대상이 아니다. 남는 탭이 Sessions 하나뿐이면 탭 바는 아무 일도 하지
///   않는 장식이 되므로 탭 바째로 생략하고 세션 목록만 직접 그린다.
/// - **🔄 동기화 버튼** — `AgentCard.svelte:73-79`. 미러는 읽기 전용이라 맥의 상태를
///   바꾸는 조작을 두지 않는다.
///
/// 두 항목 모두 `docs/ble-protocol/DEVICE-TEST.md` §6(범위 밖)에도 적혀 있다.
@MainActor
public final class MirrorViewController: UIViewController {

    private let client: BLEClient
    private var cancellables = Set<AnyCancellable>()
    private var tick: Timer?
    private var latest: MirrorSnapshot?

    private let statusLabel = UILabel()
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

    public init(client: BLEClient) {
        self.client = client
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    public override func viewDidLoad() {
        super.viewDidLoad()
        title = "AI Agent Monitor"
        // app.css:17 `.window-root { background: #1c1c1e }`.
        view.backgroundColor = Palette.windowBackground

        statusLabel.font = Typography.body
        statusLabel.textColor = Palette.subtle
        statusLabel.text = ConnectionState.idle.label

        // Detail.svelte:70 `.window-root { gap: 12px }` — 에이전트 블록과 세션 목록
        // 사이 간격. 카드끼리의 간격(agentsStack)은 Detail.svelte:71 `.agents { gap: 8px }`
        // 로 서로 다른 값이다 — 하나로 합치면 둘 중 하나가 원본과 어긋난다.
        contentStack.axis = .vertical
        contentStack.spacing = 12

        agentsStack.axis = .vertical
        agentsStack.spacing = 8

        view.addSubview(statusLabel)
        view.addSubview(scrollView)
        scrollView.addSubview(contentStack)
        [agentsStack, sessionList].forEach(contentStack.addArrangedSubview)

        statusLabel.snp.makeConstraints { make in
            make.top.equalTo(view.safeAreaLayoutGuide).offset(12)
            make.leading.trailing.equalToSuperview().inset(16)
        }
        scrollView.snp.makeConstraints { make in
            make.top.equalTo(statusLabel.snp.bottom).offset(12)
            make.leading.trailing.equalToSuperview()
            make.bottom.equalTo(view.safeAreaLayoutGuide)
        }
        contentStack.snp.makeConstraints { make in
            make.edges.equalToSuperview().inset(UIEdgeInsets(top: 0, left: 16, bottom: 16, right: 16))
            make.width.equalTo(scrollView).offset(-32)
        }

        client.state
            .receive(on: DispatchQueue.main)
            .sink { [weak self] in self?.statusLabel.text = $0.label }
            .store(in: &cancellables)

        client.snapshots
            .receive(on: DispatchQueue.main)
            .sink { [weak self] snap in
                self?.configure(snapshot: snap, now: Date())
            }
            .store(in: &cancellables)

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
}
