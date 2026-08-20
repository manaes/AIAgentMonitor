import DesignSystem
import SnapKit
import UIKit

/// 페어링 코드 입력 화면. Mac Devices 탭에 뜬 6자리 코드를 사용자가 그대로 옮겨 적는다.
///
/// 코드 자체는 여기서 만들거나 검증하지 않는다 — 이미 열려 있는 Mac 쪽 창에 제출할
/// 뿐이다. 페어링 창을 여는 것은 여전히 Mac 사용자 제스처(Devices 탭 [페어링 시작])
/// 전용이다(스펙 5.1) — 이 화면은 그 창에 코드를 제출하는 역할만 한다.
@MainActor
public final class PairingViewController: UIViewController {

    /// 사용자가 6자리를 다 입력하고 확인을 눌렀을 때 호출한다.
    public var onSubmit: ((String) -> Void)?

    private let titleLabel = UILabel()
    private let codeField = UITextField()
    private let attemptsLabel = UILabel()
    private let confirmButton = UIButton(type: .system)

    /// 테스트 전용 창구 — 다른 뷰들과 같은 규약(화면에 실제로 나간 값만 노출).
    public var confirmEnabled: Bool { confirmButton.isEnabled }
    public var attemptsText: String? { attemptsLabel.text }
    public var enteredCode: String? { codeField.text }

    public override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.windowBackground

        titleLabel.font = Typography.name
        titleLabel.textColor = Palette.primaryText
        titleLabel.textAlignment = .center
        titleLabel.numberOfLines = 0
        titleLabel.text = "Mac 화면의 6자리 코드를 입력하세요"

        codeField.font = Typography.bigRate
        codeField.textColor = Palette.primaryText
        codeField.textAlignment = .center
        codeField.keyboardType = .numberPad
        codeField.placeholder = "000000"
        codeField.addTarget(self, action: #selector(codeFieldChanged), for: .editingChanged)

        attemptsLabel.font = Typography.body
        attemptsLabel.textColor = Palette.countdown
        attemptsLabel.textAlignment = .center
        attemptsLabel.numberOfLines = 0

        confirmButton.setTitle("확인", for: .normal)
        confirmButton.isEnabled = false
        confirmButton.addTarget(self, action: #selector(confirmTapped), for: .touchUpInside)

        [titleLabel, codeField, attemptsLabel, confirmButton].forEach(view.addSubview)

        titleLabel.snp.makeConstraints { make in
            make.top.equalTo(view.safeAreaLayoutGuide).offset(40)
            make.leading.trailing.equalToSuperview().inset(24)
        }
        codeField.snp.makeConstraints { make in
            make.top.equalTo(titleLabel.snp.bottom).offset(24)
            make.leading.trailing.equalToSuperview().inset(24)
            make.height.equalTo(48)
        }
        attemptsLabel.snp.makeConstraints { make in
            make.top.equalTo(codeField.snp.bottom).offset(8)
            make.leading.trailing.equalToSuperview().inset(24)
        }
        confirmButton.snp.makeConstraints { make in
            make.top.equalTo(attemptsLabel.snp.bottom).offset(24)
            make.centerX.equalToSuperview()
        }
    }

    /// 남은 시도 횟수를 반영한다. `nil` 이면 아직 틀린 적이 없다는 뜻이라 문구를 비운다
    /// — 처음 페어링을 시작하는 사용자에게 "실패"를 암시하지 않기 위함이다.
    public func setAttemptsRemaining(_ left: Int?) {
        guard let left else {
            attemptsLabel.text = nil
            return
        }
        attemptsLabel.text = "코드가 틀렸습니다 · \(left)회 남음"
    }

    @objc private func codeFieldChanged() {
        confirmButton.isEnabled = (codeField.text?.count ?? 0) == 6
    }

    @objc private func confirmTapped() {
        guard let code = codeField.text, code.count == 6 else { return }
        onSubmit?(code)
    }

    // MARK: - 테스트 전용 구동부

    /// 사용자가 코드를 입력하는 것을 흉내낸다 — `UITextField` 는 프로그램적으로 값을
    /// 넣어도 `.editingChanged` 액션을 스스로 발동하지 않으므로 여기서 대신 불러준다.
    public func simulateCodeEntry(_ code: String) {
        codeField.text = code
        codeFieldChanged()
    }

    /// 확인 버튼 탭을 흉내낸다.
    public func simulateConfirmTap() {
        confirmTapped()
    }
}
