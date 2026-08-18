import BLETransport
import Combine
import DesignSystem
import SnapKit
import UIKit
import Wire

/// Detail 창을 미러링하는 화면. 연결 상태를 항상 상단에 노출해
/// 화면이 비어 있을 때 원인이 미궁이 되지 않게 한다(스펙 7.3).
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

        contentStack.axis = .vertical
        contentStack.spacing = 8

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
                self?.latest = snap
                self?.render()
            }
            .store(in: &cancellables)

        // 카운트다운과 상대 시각은 클라이언트가 계산하므로 추가 전송 없이 1초마다 다시 그린다.
        // 클로저가 self 를 약하게만 잡으므로 타이머가 컨트롤러의 수명을 늘리지 않는다.
        tick = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.render() }
        }

        client.start()
    }

    deinit {
        tick?.invalidate()
    }

    private func render() {
        guard let snap = latest else { return }
        let now = Date()

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
