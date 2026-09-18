import DesignSystem
import Fleet
import UIKit

/// 페어링된 Mac 목록. 레지스트리가 바뀌면 fleet 을 통째로 다시 만든다.
final class DeviceListViewController: UIViewController {
    private enum Section { case main }

    private let environment: AppEnvironment
    private var fleet: DeviceFleet?
    private var collectionView: UICollectionView!
    private var dataSource: UICollectionViewDiffableDataSource<Section, String>!
    private var reprobeTimer: Timer?

    init(environment: AppEnvironment) {
        self.environment = environment
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    deinit {
        // 타이머는 target 을 강하게 잡으므로(여기서는 블록) 화면이 사라질 때 직접 멈춘다.
        reprobeTimer?.invalidate()
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "장치"
        view.backgroundColor = Palette.windowBackground

        navigationItem.rightBarButtonItem = UIBarButtonItem(
            barButtonSystemItem: .add, target: self, action: #selector(addDeviceTapped)
        )

        configureCollectionView()
        startFleet()
        startReprobeTimer()
    }

    // MARK: - 구성

    private func configureCollectionView() {
        var config = UICollectionLayoutListConfiguration(appearance: .insetGrouped)
        config.backgroundColor = Palette.windowBackground
        // 목록에서 지울 수 있어야 16대 상한에 걸렸을 때 자리를 비울 수 있다.
        config.trailingSwipeActionsConfigurationProvider = { [weak self] indexPath in
            guard let self, let id = self.dataSource.itemIdentifier(for: indexPath) else { return nil }
            let delete = UIContextualAction(style: .destructive, title: "삭제") { [weak self] _, _, done in
                self?.removeDevice(endpointIdHex: id)
                done(true)
            }
            return UISwipeActionsConfiguration(actions: [delete])
        }
        let layout = UICollectionViewCompositionalLayout.list(using: config)

        collectionView = UICollectionView(frame: view.bounds, collectionViewLayout: layout)
        collectionView.backgroundColor = Palette.windowBackground
        collectionView.delegate = self
        collectionView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(collectionView)

        let registration = UICollectionView.CellRegistration<DeviceCell, String> { [weak self] cell, _, id in
            guard let self, let model = self.rowModel(for: id) else { return }
            cell.configure(model)
        }
        dataSource = UICollectionViewDiffableDataSource(collectionView: collectionView) { view, indexPath, id in
            view.dequeueConfiguredReusableCell(using: registration, for: indexPath, item: id)
        }

        let refresh = UIRefreshControl()
        refresh.addTarget(self, action: #selector(pulledToRefresh), for: .valueChanged)
        collectionView.refreshControl = refresh
    }

    private func startFleet() {
        // 옛 fleet 의 스트림을 먼저 끊는다 — 안 그러면 삭제된 장치의 연결이 누수되고 남은
        // 장치는 이중 연결이 된다.
        fleet?.stopAll()
        let fleet = environment.makeFleet()
        fleet.onChange = { [weak self] in self?.applySnapshot() }
        self.fleet = fleet
        applySnapshot()
        Task { await fleet.startInitialRound() }
    }

    /// 포어그라운드 상태에서만 돈다(suspend 되면 타이머도 멈춘다). 오프라인 장치만 다시 확인한다.
    private func startReprobeTimer() {
        reprobeTimer = Timer.scheduledTimer(
            withTimeInterval: DeviceFleet.reprobeInterval, repeats: true
        ) { [weak self] _ in
            Task { @MainActor in
                guard let fleet = self?.fleet else { return }
                await fleet.retriggerOffline()
            }
        }
    }

    // MARK: - 데이터

    private func rowModel(for id: String) -> DeviceRowModel? {
        guard let session = fleet?.sessions.first(where: { $0.device.endpointIdHex == id }) else {
            return nil
        }
        // 신선도는 **스냅샷을 받은 시각**으로 계산한다. 지금 시각을 쓰면 오프라인으로 떨어진
        // 장치가 들고 있는 낡은 스냅샷이 영원히 "방금 전"으로 보인다.
        let live = session.latest.flatMap { snapshot in
            session.latestAt.map { CachedSnapshot(snapshot: snapshot, fetchedAt: $0) }
        }
        let cached = live ?? environment.cache.load(endpointIdHex: id)
        return DeviceListPresentation.row(
            device: session.device, status: session.status, cached: cached, now: Date()
        )
    }

    private func applySnapshot() {
        guard let fleet, let dataSource else { return }
        let ordered = DeviceListPresentation.sorted(
            fleet.sessions.map { ($0.device, $0.status) }
        )
        let ids = ordered.map(\.endpointIdHex)
        var snapshot = NSDiffableDataSourceSnapshot<Section, String>()
        snapshot.appendSections([.main])
        snapshot.appendItems(ids)
        // 식별자는 그대로인데 내용(상태·tok/s·사용량)만 바뀌는 게 대부분이라 diff 만으로는
        // 셀이 갱신되지 않는다. 이미 있던 항목만 reconfigure 한다 — 새로 추가되는 항목까지
        // 넣으면 삽입과 갱신이 겹쳐 적용이 거부된다.
        let existing = Set(dataSource.snapshot().itemIdentifiers)
        snapshot.reconfigureItems(ids.filter(existing.contains))
        dataSource.apply(snapshot, animatingDifferences: true)
    }

    // MARK: - 동작

    func reconnectAll() {
        guard let fleet else { return }
        Task { await fleet.startInitialRound() }
    }

    @objc private func pulledToRefresh() {
        guard let fleet else {
            collectionView.refreshControl?.endRefreshing()
            return
        }
        Task { [weak self] in
            await fleet.retriggerOffline()
            self?.collectionView.refreshControl?.endRefreshing()
        }
    }

    @objc private func addDeviceTapped() {
        // Task 15 에서 채운다.
    }

    private func removeDevice(endpointIdHex: String) {
        do {
            _ = try environment.registry.remove(endpointIdHex: endpointIdHex)
            // startFleet() 이 옛 fleet 전체를 stopAll() 하므로 지운 장치의 세션도 여기서 멈춘다.
            // 이 함수는 메인 액터에서 await 없이 이어지므로 그 사이에 늦은 결과가 끼어들 틈이 없다.
            startFleet()
        } catch {
            let alert = UIAlertController(
                title: nil, message: "삭제하지 못했습니다", preferredStyle: .alert
            )
            alert.addAction(UIAlertAction(title: "확인", style: .default))
            present(alert, animated: true)
        }
    }
}

extension DeviceListViewController: UICollectionViewDelegate {
    func collectionView(_ collectionView: UICollectionView, didSelectItemAt indexPath: IndexPath) {
        collectionView.deselectItem(at: indexPath, animated: true)
        // Task 16 에서 채운다.
    }
}
