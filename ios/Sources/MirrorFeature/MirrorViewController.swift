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
    private let claudeCard = AgentCardView()
    private let codexCard = AgentCardView()
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
        view.backgroundColor = .black

        statusLabel.font = Typography.body
        statusLabel.textColor = Palette.subtle
        statusLabel.text = ConnectionState.idle.label

        contentStack.axis = .vertical
        contentStack.spacing = 8

        view.addSubview(statusLabel)
        view.addSubview(scrollView)
        scrollView.addSubview(contentStack)
        [claudeCard, codexCard, sessionList].forEach(contentStack.addArrangedSubview)

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

        // 스냅샷이 오기 전에는 카드를 감춰 빈 껍데기를 보여주지 않는다.
        claudeCard.isHidden = true
        codexCard.isHidden = true

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

        if let claude = snap.agents.first(where: { $0.kind == .claude }) {
            claudeCard.isHidden = false
            claudeCard.configure(agent: claude, now: now)
        } else {
            claudeCard.isHidden = true
        }

        if let codex = snap.agents.first(where: { $0.kind == .codex }) {
            codexCard.isHidden = false
            codexCard.configure(agent: codex, now: now)
        } else {
            codexCard.isHidden = true
        }

        sessionList.configure(snapshot: snap, now: now)
    }
}
