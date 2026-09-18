import Fleet
import UIKit

/// Task 14 에서 컬렉션 뷰로 채운다. 여기서는 타겟이 빌드·실행되는 것만 확인한다.
final class DeviceListViewController: UIViewController {
    private let environment: AppEnvironment
    private var fleet: DeviceFleet?

    init(environment: AppEnvironment) {
        self.environment = environment
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "장치"
        view.backgroundColor = .systemBackground
        fleet = environment.makeFleet()
    }

    func reconnectAll() {
        guard let fleet else { return }
        Task { await fleet.startInitialRound() }
    }
}
