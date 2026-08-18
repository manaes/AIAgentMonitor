import DesignSystem
import SnapKit
import UIKit
import Wire

/// `SessionList.svelte` 이식. 모든 에이전트의 프로젝트를 한 줄씩 펼쳐
/// 최근 활동순으로 정렬한다.
public final class SessionListView: UIView {

    private let titleLabel = UILabel()
    private let emptyLabel = UILabel()
    private let stack = UIStackView()
    private var rows: [SessionRowView] = []

    public var rowCount: Int { rows.filter { !$0.isHidden }.count }
    public var isEmptyMessageVisible: Bool { !emptyLabel.isHidden }
    public func rowText(at index: Int) -> String? {
        let visible = rows.filter { !$0.isHidden }
        guard index < visible.count else { return nil }
        return visible[index].leftText
    }

    public init() {
        super.init(frame: .zero)
        backgroundColor = Palette.cardBackground
        layer.cornerRadius = 8

        // 원본은 소문자 텍스트에 CSS 로 text-transform: uppercase 와 letter-spacing: 0.4px 를
        // 적용한다(SessionList.svelte:47). iOS 에는 text-transform 이 없으므로 대문자화는
        // 여기서 하고, 자간은 attributedText 의 .kern 으로 재현한다.
        titleLabel.attributedText = NSAttributedString(
            string: "Active sessions · sorted by recent activity".uppercased(),
            attributes: [
                .kern: 0.4,
                .font: Typography.sectionLabel,
                .foregroundColor: Palette.subtle,
            ]
        )

        emptyLabel.text = "No sessions yet."
        emptyLabel.font = Typography.body
        emptyLabel.textColor = Palette.subtle

        stack.axis = .vertical
        stack.spacing = 0

        [titleLabel, emptyLabel, stack].forEach(addSubview)

        titleLabel.snp.makeConstraints { make in
            make.top.equalToSuperview().offset(10)
            make.leading.trailing.equalToSuperview().inset(12)
        }
        emptyLabel.snp.makeConstraints { make in
            make.top.equalTo(titleLabel.snp.bottom).offset(6)
            make.leading.trailing.equalToSuperview().inset(12)
        }
        stack.snp.makeConstraints { make in
            make.top.equalTo(emptyLabel.snp.bottom)
            make.leading.trailing.equalToSuperview().inset(12)
            make.bottom.equalToSuperview().offset(-10)
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    public func configure(snapshot: MirrorSnapshot, now: Date) {
        // 에이전트별 프로젝트를 한 줄로 펼치고 최근 활동순 정렬 — 원본과 동일.
        let entries = snapshot.agents
            .flatMap { agent in agent.projects.map { (project: $0, kind: agent.kind) } }
            .sorted { $0.project.t > $1.project.t }

        emptyLabel.isHidden = !entries.isEmpty

        // 스냅샷이 1Hz 로 들어오므로 행을 재사용한다. 매번 새로 만들면 뷰가 쌓인다.
        while rows.count < entries.count {
            let row = SessionRowView()
            rows.append(row)
            stack.addArrangedSubview(row)
        }

        for (i, row) in rows.enumerated() {
            if i < entries.count {
                row.isHidden = false
                row.configure(project: entries[i].project, kind: entries[i].kind, now: now)
            } else {
                row.isHidden = true
            }
        }
    }
}
