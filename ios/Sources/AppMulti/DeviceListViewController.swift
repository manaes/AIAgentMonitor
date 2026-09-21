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

    /// 마지막으로 그린 행. 내용이 그대로인 셀까지 reconfigure 하지 않기 위한 비교 기준이다 —
    /// 16대가 초당 한 장씩 흘리면 갱신 하나마다 보이는 셀 전부를 다시 그리게 된다.
    private var renderedRows: [String: DeviceRowModel] = [:]
    /// 오프라인 장치의 디스크 캐시 읽기 결과. 메인 스레드 동기 I/O 라 장치당 한 번만 읽는다.
    /// 값이 없다는 사실도 기억해야 하므로 옵셔널을 두 겹으로 둔다.
    private var cachedSnapshots: [String: CachedSnapshot?] = [:]
    /// onChange 코얼레싱 상태.
    private var needsApply = false
    private var applyScheduled = false
    /// 레지스트리 손상 알림은 한 번만 띄운다(`makeFleet()` 은 삭제할 때마다 다시 불린다).
    private var hasShownRegistryError = false

    init(environment: AppEnvironment) {
        self.environment = environment
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) 는 쓰지 않는다") }

    deinit {
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
        observeAppLifecycle()
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        // 콜드런치에서는 willEnterForeground 알림이 이 화면이 뜨기 전에 지나가므로
        // 여기서 타이머를 건다. startReprobeTimer 가 중복 생성을 막는다.
        startReprobeTimer()
        // viewDidLoad 에서는 안 된다 — 아직 윈도우에 붙기 전이라 present 가 먹지 않고,
        // registryError 는 startFleet() 안의 makeFleet() 이 세팅한다.
        presentRegistryErrorIfNeeded()
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
        // 스펙 §6.1 — 그룹 안에서는 사용자가 끌어다 놓은 순서(sortIndex)를 따른다.
        collectionView.dragDelegate = self
        collectionView.dropDelegate = self
        collectionView.dragInteractionEnabled = true
        collectionView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(collectionView)

        let registration = UICollectionView.CellRegistration<DeviceCell, String> { [weak self] cell, _, id in
            guard let self, let model = self.renderedRows[id] ?? self.rowModel(for: id) else { return }
            cell.configure(model)
        }
        dataSource = UICollectionViewDiffableDataSource(collectionView: collectionView) { view, indexPath, id in
            view.dequeueConfiguredReusableCell(using: registration, for: indexPath, item: id)
        }
        dataSource.reorderingHandlers.canReorderItem = { _ in true }
        // 놓은 뒤에만 저장한다 — 드래그 도중의 중간 순서를 Keychain 에 쓰면 손가락을 뗄
        // 때까지 쓰기가 계속 일어난다.
        dataSource.reorderingHandlers.didReorder = { [weak self] transaction in
            self?.persistOrder(transaction.finalSnapshot.itemIdentifiers)
        }

        let refresh = UIRefreshControl()
        refresh.addTarget(self, action: #selector(pulledToRefresh), for: .valueChanged)
        collectionView.refreshControl = refresh
    }

    private func observeAppLifecycle() {
        NotificationCenter.default.addObserver(
            self, selector: #selector(startReprobeTimer),
            name: UIApplication.willEnterForegroundNotification, object: nil
        )
        NotificationCenter.default.addObserver(
            self, selector: #selector(stopReprobeTimer),
            name: UIApplication.didEnterBackgroundNotification, object: nil
        )
    }

    private func startFleet() {
        // 버리기 전에 캐시를 흘려 넣는다. 쓰기가 30초 스로틀이라 마지막 스냅샷이 아직
        // 디스크에 없을 수 있고, stopAll() 이 세션의 latest 를 비우면 그대로 사라진다.
        fleet?.flushCache()
        // 콜백을 먼저 끊는다 — stopAll() 은 세션마다 onChange 를 발사하므로, 그대로 두면
        // 삭제된 행이 아직 들어 있는 스냅샷을 세션 수만큼 적용하게 된다.
        fleet?.onChange = nil
        // 옛 fleet 의 스트림을 끊는다 — 안 그러면 삭제된 장치의 연결이 누수되고 남은
        // 장치는 이중 연결이 된다.
        fleet?.stopAll()
        cachedSnapshots.removeAll()
        renderedRows.removeAll()

        let fleet = environment.makeFleet()
        fleet.onChange = { [weak self] in self?.scheduleApply() }
        self.fleet = fleet
        applySnapshot()
        Task { await fleet.startInitialRound() }
    }

    /// 포어그라운드에서만 돈다. 오프라인 장치만 다시 확인한다.
    @objc private func startReprobeTimer() {
        // 이미 돌고 있으면 그대로 둔다 — viewDidAppear 와 willEnterForeground 양쪽에서
        // 불리는데, 매번 다시 만들면 3분 카운트가 0으로 되돌아간다. 상세 화면을 오갈 때마다
        // viewDidAppear 가 불리므로 타이머는 영원히 발사되지 않는다.
        guard reprobeTimer == nil else { return }
        reprobeTimer = Timer.scheduledTimer(
            withTimeInterval: DeviceFleet.reprobeInterval, repeats: true
        ) { [weak self] _ in
            Task { @MainActor in
                guard let fleet = self?.fleet else { return }
                await fleet.retriggerOffline()
            }
        }
    }

    @objc private func stopReprobeTimer() {
        reprobeTimer?.invalidate()
        reprobeTimer = nil
    }

    // MARK: - 데이터

    /// onChange 는 스냅샷마다 온다. 런루프 한 틱에 한 번만 적용해 초당 수십 번의
    /// diffable 적용을 막는다.
    private func scheduleApply() {
        needsApply = true
        guard !applyScheduled else { return }
        applyScheduled = true
        Task { @MainActor [weak self] in
            guard let self else { return }
            self.applyScheduled = false
            guard self.needsApply else { return }
            self.needsApply = false
            self.applySnapshot()
        }
    }

    private func rowModel(for id: String) -> DeviceRowModel? {
        guard let session = fleet?.sessions.first(where: { $0.device.endpointIdHex == id }) else {
            return nil
        }
        // 신선도는 **스냅샷을 받은 시각**으로 계산한다. 지금 시각을 쓰면 오프라인으로 떨어진
        // 장치가 들고 있는 낡은 스냅샷이 영원히 "방금 전"으로 보인다.
        let live = session.latest.flatMap { snapshot in
            session.latestAt.map { CachedSnapshot(snapshot: snapshot, fetchedAt: $0) }
        }
        let cached = live ?? diskSnapshot(for: id)
        return DeviceListPresentation.row(
            device: session.device, status: session.status, cached: cached, now: Date()
        )
    }

    /// 디스크 캐시는 장치당 한 번만 읽는다 — `load` 는 메인 스레드에서 파일 읽기 + JSON
    /// 디코드를 하므로 갱신마다 부르면 안 된다. fleet 을 다시 만들 때 비운다.
    private func diskSnapshot(for id: String) -> CachedSnapshot? {
        if let memo = cachedSnapshots[id] { return memo }
        let loaded = environment.cache.load(endpointIdHex: id)
        cachedSnapshots[id] = loaded
        return loaded
    }

    private func applySnapshot() {
        guard let fleet, let dataSource else { return }
        let ordered = DeviceListPresentation.sorted(
            fleet.sessions.map { ($0.device, $0.status) }
        )
        let ids = ordered.map(\.endpointIdHex)
        let previous = Set(dataSource.snapshot().itemIdentifiers)

        var models: [String: DeviceRowModel] = [:]
        var changed: [String] = []
        for id in ids {
            guard let model = rowModel(for: id) else { continue }
            models[id] = model
            // 이미 화면에 있고 내용까지 그대로면 건드리지 않는다. 새로 삽입되는 id 를
            // 여기 넣으면 삽입과 갱신이 같은 스냅샷에서 겹쳐 적용이 거부된다.
            if previous.contains(id), renderedRows[id] != model { changed.append(id) }
        }
        renderedRows = models

        var snapshot = NSDiffableDataSourceSnapshot<Section, String>()
        snapshot.appendSections([.main])
        snapshot.appendItems(ids)
        snapshot.reconfigureItems(changed)
        // 구성이 그대로면 애니메이션할 것이 없다 — 스트리밍 중 매 갱신마다 셀이 깜빡인다.
        dataSource.apply(snapshot, animatingDifferences: previous != Set(ids))
    }

    // MARK: - 동작

    func reconnectAll() {
        guard let fleet else { return }
        Task { await fleet.startInitialRound() }
    }

    /// 캐시 쓰기는 스로틀돼 있으므로 백그라운드 전환 시 마지막 상태를 흘려 넣는다.
    func flushCache() {
        fleet?.flushCache()
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
        // 스펙 6.3 은 "스캐너를 열기 전에 막으라" 하고 8 은 "중복 스캔은 병합" 이라 한다. 16대가
        // 찼을 때 그냥 막으면 재스캔(= 이름·연결정보를 고치는 유일한 경로)까지 닫혀버리므로,
        // 새 장치는 못 받는다는 사실을 먼저 알리고 기존 장치 갱신 경로는 열어둔다.
        if let count = try? environment.registry.load().count, count >= DeviceRegistry.maxDevices {
            let full = UIAlertController(
                title: nil,
                message: "장치는 최대 \(DeviceRegistry.maxDevices)대까지 추가할 수 있습니다. "
                    + "이미 등록된 Mac 을 다시 스캔하면 이름과 연결 정보를 갱신할 수 있습니다.",
                preferredStyle: .alert
            )
            full.addAction(UIAlertAction(title: "취소", style: .cancel))
            full.addAction(UIAlertAction(title: "스캔", style: .default) { [weak self] _ in
                self?.presentAddDevice()
            })
            present(full, animated: true)
            return
        }
        presentAddDevice()
    }

    /// `repairing` 이 있으면 그 장치를 고치러 온 것이다 — 스캐너가 그 장치의 QR 만 받는다.
    /// `+` 버튼 경로는 nil 로 둬서 지금까지와 같은 범용 스캐너가 된다.
    private func presentAddDevice(repairing device: Device? = nil) {
        let add = AddDeviceViewController(registry: environment.registry)
        add.expectedEndpointIdHex = device?.endpointIdHex
        add.expectedDeviceName = device?.displayName
        add.onAdded = { [weak self] _ in
            // 레지스트리가 바뀌었으므로 fleet 을 다시 만든다(startFleet 이 옛 fleet 을
            // flushCache 한 뒤 stopAll 한다 — Ruling 17).
            self?.startFleet()
        }
        // 다크 강제는 SceneDelegate 의 루트 윈도우에서 한 번만 한다 — 모달에 따로 걸면
        // 화면이 늘 때마다 빠뜨린다.
        present(UINavigationController(rootViewController: add), animated: true)
    }

    private func removeDevice(endpointIdHex: String) {
        do {
            _ = try environment.registry.remove(endpointIdHex: endpointIdHex)
            // startFleet() 이 옛 fleet 전체를 stopAll() 하므로 지운 장치의 세션도 여기서 멈춘다.
            // 이 함수는 메인 액터에서 await 없이 이어지므로 그 사이에 늦은 결과가 끼어들 틈이 없다.
            startFleet()
            // 캐시 파일 삭제는 **반드시 startFleet() 뒤**여야 한다(Ruling 34).
            // startFleet() 첫 줄이 옛 fleet 의 flushCache() 인데, 그 옛 fleet 은 방금 지운
            // 장치의 세션을 아직 들고 있고 그 세션의 latest 도 살아 있다 — 먼저 지우면
            // 그 flush 가 파일을 그대로 되살린다. 여기서 지우면 새 fleet 에는 그 장치가
            // 없으므로 이후 어떤 flush 도 파일을 다시 만들지 못한다.
            // 남기면 고아 파일이 쌓이고, 같은 Mac 을 다시 페어링했을 때 연결되기도 전에
            // 낡은 스냅샷이 먼저 보인다.
            environment.cache.remove(endpointIdHex: endpointIdHex)
        } catch {
            let alert = UIAlertController(
                title: nil, message: "삭제하지 못했습니다", preferredStyle: .alert
            )
            alert.addAction(UIAlertAction(title: "확인", style: .default))
            present(alert, animated: true)
        }
    }

    /// 레지스트리를 읽지 못했으면 빈 목록으로 조용히 시작하지 않는다 — 사용자가
    /// "페어링된 장치 없음"으로 오해하고 다시 스캔하면, 읽지 못했을 뿐 살아 있던
    /// 목록을 덮어쓴다.
    private func presentRegistryErrorIfNeeded() {
        guard !hasShownRegistryError, let error = environment.registryError else { return }
        hasShownRegistryError = true
        let alert = UIAlertController(
            title: DeviceRegistryError.corruptedTitle,
            message: "\(error.localizedDescription)\n\n\(DeviceRegistryError.corruptedAdvice)",
            preferredStyle: .alert
        )
        alert.addAction(UIAlertAction(title: "확인", style: .default))
        present(alert, animated: true)
    }
}

extension DeviceListViewController: UICollectionViewDelegate {
    func collectionView(_ collectionView: UICollectionView, didSelectItemAt indexPath: IndexPath) {
        collectionView.deselectItem(at: indexPath, animated: true)
        guard let id = dataSource.itemIdentifier(for: indexPath),
              let fleet,
              let session = fleet.sessions.first(where: { $0.device.endpointIdHex == id }) else {
            return
        }
        switch DeviceListPresentation.tapDestination(for: session.status) {
        case .rePair:
            // 재페어링 필요는 상세로 보내봐야 할 수 있는 게 없다 — 토큰이 폐기된 상태라 재시도도
            // 막혀 있다(종단 상태). 스펙 §7 대로 그 장치를 다시 스캔하는 경로로 보낸다.
            // 재스캔은 endpointIdHex 로 병합되므로(Ruling 25) 이름·정렬순서는 보존되고 토큰만 갱신된다.
            // 16대 사전 안내(Ruling 28)는 여기서 하지 않는다 — 기존 장치 갱신이라 한도와 무관하다.
            presentAddDevice(repairing: session.device)
        case .detail:
            // 다른 세션은 끊지 않는다 — 목록으로 돌아왔을 때 즉시 최신값이 보여야 하고,
            // 끊었다 다시 붙이면 probe 승격으로 아끼려던 비용을 그대로 다시 낸다(스펙 §4.4).
            let detail = DeviceDetailViewController(
                session: session, fleet: fleet, cache: environment.cache
            )
            navigationController?.pushViewController(detail, animated: true)
        }
    }
}

// MARK: - 드래그 재정렬 (스펙 §6.1)

extension DeviceListViewController: UICollectionViewDragDelegate {
    func collectionView(
        _ collectionView: UICollectionView, itemsForBeginning session: UIDragSession,
        at indexPath: IndexPath
    ) -> [UIDragItem] {
        guard let id = dataSource.itemIdentifier(for: indexPath) else { return [] }
        // 앱 밖으로 끌어낼 일이 없으므로 페이로드는 비워 두고 localObject 로만 식별한다.
        let item = UIDragItem(itemProvider: NSItemProvider())
        item.localObject = id
        return [item]
    }
}

extension DeviceListViewController: UICollectionViewDropDelegate {
    /// 상태 그룹을 가로지르는 이동은 막는다. 표시 순서는 상태 그룹이 1차 키라, 그룹을
    /// 넘겨 놓아봐야 `DeviceListPresentation.sorted` 가 곧바로 원래 그룹으로 돌려보낸다 —
    /// 사용자에게는 "드래그가 먹지 않는" 것으로 보이므로 아예 못 놓게 한다.
    func collectionView(
        _ collectionView: UICollectionView, dropSessionDidUpdate session: UIDropSession,
        withDestinationIndexPath destinationIndexPath: IndexPath?
    ) -> UICollectionViewDropProposal {
        guard session.localDragSession != nil else {
            return UICollectionViewDropProposal(operation: .forbidden)
        }
        guard let destinationIndexPath,
              let draggedId = session.localDragSession?.items.first?.localObject as? String,
              let destinationId = dataSource.itemIdentifier(for: destinationIndexPath),
              let from = status(of: draggedId), let to = status(of: destinationId),
              DeviceListPresentation.allowsReorder(from: from, to: to) else {
            return UICollectionViewDropProposal(operation: .cancel)
        }
        return UICollectionViewDropProposal(
            operation: .move, intent: .insertAtDestinationIndexPath
        )
    }

    /// 실제 항목 이동은 diffable 데이터소스의 `reorderingHandlers` 가 처리한다 —
    /// 여기서 또 옮기면 이중 적용이 된다.
    func collectionView(
        _ collectionView: UICollectionView, performDropWith coordinator: UICollectionViewDropCoordinator
    ) {}

    private func status(of id: String) -> DeviceStatus? {
        fleet?.sessions.first(where: { $0.device.endpointIdHex == id })?.status
    }

    /// 놓인 순서를 레지스트리에 저장하고, 살아 있는 세션에 그대로 얹는다.
    ///
    /// fleet 을 다시 만들지 않는 게 핵심이다 — 순서만 바뀐 건데 다시 만들면 16대의 QUIC
    /// 연결이 전부 끊긴다. 세션에 반영하지 않으면 다음 갱신(스트리밍 중에는 매초)에서
    /// 옛 sortIndex 로 다시 정렬돼 방금 옮긴 자리가 튕겨 돌아간다.
    private func persistOrder(_ orderedIds: [String]) {
        do {
            let devices = try environment.registry.reorder(orderedIds)
            fleet?.applySortOrder(devices)
            // renderedRows 는 그대로 쓸 수 있다 — 내용은 안 바뀌고 순서만 바뀐다.
            applySnapshot()
        } catch {
            // 저장이 실패했는데 화면만 새 순서로 두면 다음 실행에서 조용히 되돌아간다.
            let alert = UIAlertController(
                title: nil, message: "순서를 저장하지 못했습니다", preferredStyle: .alert
            )
            alert.addAction(UIAlertAction(title: "확인", style: .default))
            present(alert, animated: true)
            // 저장된 순서(옛 sortIndex)로 다시 그려 화면과 저장 상태를 맞춘다.
            applySnapshot()
        }
    }
}
