import DesignSystem
import SnapKit
import UIKit
import Wire

/// `SessionList.svelte` 이식. 모든 에이전트의 프로젝트를 한 줄씩 펼쳐
/// 최근 활동순으로 정렬한다.
public final class SessionListView: UIView {

    /// 행 하나 + 그 위의 구분선을 함께 관리하는 슬롯. `SessionRowView` 자체는
    /// 구분선을 모른다 — 구분선은 목록 안에서 "이 행이 두 번째 이후인가" 라는
    /// 목록 차원의 정보이기 때문에 여기서 감싼다.
    private final class RowSlot {
        let container = UIView()
        let separator = UIView()
        let row = SessionRowView()
        private var separatorHeight: Constraint?

        init() {
            separator.backgroundColor = Palette.separator
            [separator, row].forEach(container.addSubview)

            // SessionList.svelte:49 `.row + .row { border-top: 1px solid #3a3a3c }`.
            // CSS px 는 기기 독립 단위다 — Mac 의 Retina 디스플레이에서도 1px 는
            // 사용자 눈에 1pt 로 보인다. 이 포트의 다른 모든 값(패딩·간격·점 크기)이
            // CSS 1px 를 1pt 로 옮겼으므로 여기만 물리 픽셀(1/scale)로 예외를 두지
            // 않는다 — 3배 기기에서는 0.33pt 로 Mac 보다 눈에 띄게 얇아진다.
            separator.snp.makeConstraints { make in
                make.leading.trailing.top.equalToSuperview()
                separatorHeight = make.height.equalTo(1).constraint
            }
            row.snp.makeConstraints { make in
                make.leading.trailing.bottom.equalToSuperview()
                make.top.equalTo(separator.snp.bottom)
            }
        }

        /// 첫 번째로 보이는 행 위에는 구분선이 없어야 한다. `isHidden` 만으로는
        /// 부족하다 — row.top 이 separator.bottom 에 고정돼 있어서, 숨겨도 제약
        /// 자체는 그대로 남아 row 가 1pt 만큼 밀려난다(유령 여백). 높이 제약의
        /// 상수 자체를 0 으로 낮춰야 실제로 공간이 사라진다.
        func setSeparatorVisible(_ visible: Bool) {
            separator.isHidden = !visible
            separatorHeight?.update(offset: visible ? 1 : 0)
        }
    }

    private let titleLabel = UILabel()
    private let emptyLabel = UILabel()
    private let stack = UIStackView()
    private var slots: [RowSlot] = []

    public var rowCount: Int { slots.filter { !$0.container.isHidden }.count }
    public var isEmptyMessageVisible: Bool { !emptyLabel.isHidden }
    public func rowText(at index: Int) -> String? {
        let visible = slots.filter { !$0.container.isHidden }
        guard index < visible.count else { return nil }
        return visible[index].row.leftText
    }
    /// 테스트 전용 창구: 보이는 행들 중 index 번째 위 구분선이 레이아웃에서 실제로
    /// 차지하는 높이. 0 이면 구분선이 없는 것이다(첫 행이어야 한다).
    public func separatorHeight(at index: Int) -> CGFloat? {
        let visible = slots.filter { !$0.container.isHidden }
        guard index < visible.count else { return nil }
        return visible[index].separator.bounds.height
    }
    /// 테스트 전용 창구: 재사용 풀에 만들어진 슬롯의 총 개수(보이는 것 + 숨은 것).
    public var pooledSlotCount: Int { slots.count }

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
        // emptyLabel 을 stack 의 arranged subview 로 넣어야 isHidden 이 실제로
        // 공간을 접는다. 별도 서브뷰로 두고 제약만 붙이면(이전 구현) isHidden 이어도
        // 프레임은 그대로 남아, 행이 있을 때도 라벨 높이만큼 빈 공간이 생긴다.
        stack.addArrangedSubview(emptyLabel)

        [titleLabel, stack].forEach(addSubview)

        titleLabel.snp.makeConstraints { make in
            make.top.equalToSuperview().offset(10)
            make.leading.trailing.equalToSuperview().inset(12)
        }
        // SessionList.svelte:47 `.label { margin: 0 0 6px }` — 라벨과 첫 줄 사이 6px.
        stack.snp.makeConstraints { make in
            make.top.equalTo(titleLabel.snp.bottom).offset(6)
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

        // 스냅샷이 1Hz 로 들어오므로 슬롯을 재사용한다. 매번 새로 만들면 뷰가 쌓인다.
        while slots.count < entries.count {
            let slot = RowSlot()
            slots.append(slot)
            stack.addArrangedSubview(slot.container)
        }

        for (i, slot) in slots.enumerated() {
            if i < entries.count {
                slot.container.isHidden = false
                // 매번 새로 계산한다: 첫 번째로 보이는 행 위에는 구분선이 없어야 하고,
                // 재사용 때문에 이전 프레임에 "두 번째"였던 슬롯이 이번엔 "첫 번째"가
                // 될 수도 있다.
                slot.setSeparatorVisible(i != 0)
                slot.row.configure(project: entries[i].project, kind: entries[i].kind, now: now)
            } else {
                // 컨테이너 전체가 숨으므로 안의 구분선도 함께 사라진다 — 남는 슬롯이
                // 유령 구분선을 남기지 않는다.
                slot.container.isHidden = true
            }
        }
    }
}
