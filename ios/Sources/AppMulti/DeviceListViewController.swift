import DesignSystem
import Fleet
import SnapKit
import UIKit

/// 페어링된 Mac 목록. 레지스트리가 바뀌면 fleet 을 통째로 다시 만든다.
final class DeviceListViewController: UIViewController {
    /// 보기 모드. 같은 계정의 Claude/Codex 를 여러 Mac 에서 쓰면 한도 %가 장치마다
    /// 똑같이 반복되므로, 한도는 위에서 한 번만 합쳐 보고 아래에서 장치별 tok/s 만 본다.
    private enum ViewMode: String {
        case individual
        case unified
    }

    private enum Section: Hashable {
        /// 개별 모드의 유일한 섹션.
        case main
        /// 통합 모드 상단 — 에이전트 종류별 한도.
        case quota
        /// 통합 모드 하단 — 장치별 tok/s.
        case rates
        /// 첫 확인이 끝나기 전. 두 모드 공통이며 헤더가 없다.
        case loading

        var headerTitle: String? {
            switch self {
            case .main: return nil
            case .quota: return "한도 (통합)"
            case .rates: return "장치별 속도"
            case .loading: return nil
            }
        }
    }

    /// 항목 식별자는 스냅샷 전체에서 유일해야 한다 — 통합 모드는 두 섹션이 서로 다른
    /// 종류의 키(에이전트 이름 / endpointIdHex)를 쓰므로 그냥 String 으로 두면 섞인다.
    private enum Item: Hashable {
        case device(String)
        case quotaAgent(String)
        case loading

        var deviceId: String? {
            if case .device(let id) = self { return id }
            return nil
        }
    }

    /// 다음 실행에도 유지한다. 저장 실패는 무시하고 기본값(개별)으로 시작한다.
    private static let viewModeDefaultsKey = "AppMulti.deviceListViewMode"

    private let environment: AppEnvironment
    private var fleet: DeviceFleet?
    private var collectionView: UICollectionView!
    private var segmentedControl: UISegmentedControl!
    private var dataSource: UICollectionViewDiffableDataSource<Section, Item>!
    private var reprobeTimer: Timer?
    private var viewMode: ViewMode = .individual

    /// 마지막으로 그린 행. 내용이 그대로인 셀까지 reconfigure 하지 않기 위한 비교 기준이다 —
    /// 16대가 초당 한 장씩 흘리면 갱신 하나마다 보이는 셀 전부를 다시 그리게 된다.
    private var renderedRows: [String: DeviceRowModel] = [:]
    /// 통합 모드용 비교 기준. 위와 같은 이유로 둔다(키는 각각 에이전트 이름·endpointIdHex).
    private var renderedAgentRows: [String: UnifiedAgentRow] = [:]
    private var renderedRateRows: [String: UnifiedDeviceRow] = [:]
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

        restoreViewMode()
        configureSegmentedControl()
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

    private func restoreViewMode() {
        let stored = UserDefaults.standard.string(forKey: Self.viewModeDefaultsKey)
        viewMode = stored.flatMap(ViewMode.init(rawValue:)) ?? .individual
    }

    private func configureSegmentedControl() {
        segmentedControl = UISegmentedControl(items: ["개별", "통합"])
        segmentedControl.selectedSegmentIndex = (viewMode == .unified) ? 1 : 0
        segmentedControl.addTarget(self, action: #selector(viewModeChanged), for: .valueChanged)
        view.addSubview(segmentedControl)
        segmentedControl.snp.makeConstraints { make in
            make.top.equalTo(view.safeAreaLayoutGuide.snp.top).offset(8)
            make.leading.trailing.equalToSuperview().inset(16)
        }
    }

    private func configureCollectionView() {
        collectionView = UICollectionView(frame: view.bounds, collectionViewLayout: makeLayout(for: viewMode))
        collectionView.backgroundColor = Palette.windowBackground
        collectionView.delegate = self
        // 스펙 §6.1 — 그룹 안에서는 사용자가 끌어다 놓은 순서(sortIndex)를 따른다.
        collectionView.dragDelegate = self
        collectionView.dropDelegate = self
        collectionView.dragInteractionEnabled = (viewMode == .individual)
        view.addSubview(collectionView)
        collectionView.snp.makeConstraints { make in
            make.top.equalTo(segmentedControl.snp.bottom).offset(8)
            make.leading.trailing.bottom.equalToSuperview()
        }

        let deviceRegistration = UICollectionView.CellRegistration<DeviceCell, String> { [weak self] cell, _, id in
            guard let self, let model = self.renderedRows[id] ?? self.rowModel(for: id) else { return }
            cell.configure(model)
        }
        let quotaRegistration = UICollectionView.CellRegistration<UnifiedQuotaCell, String> { [weak self] cell, _, name in
            guard let self, let model = self.renderedAgentRows[name] else { return }
            cell.configure(model)
        }
        let rateRegistration = UICollectionView.CellRegistration<UnifiedRateCell, String> { [weak self] cell, _, id in
            guard let self, let model = self.renderedRateRows[id] else { return }
            cell.configure(model)
        }

        let loadingRegistration = UICollectionView.CellRegistration<LoadingCell, Int> { _, _, _ in }

        dataSource = UICollectionViewDiffableDataSource(collectionView: collectionView) { [weak self] view, indexPath, item in
            switch item {
            case .loading:
                return view.dequeueConfiguredReusableCell(using: loadingRegistration, for: indexPath, item: 0)
            case .quotaAgent(let name):
                return view.dequeueConfiguredReusableCell(using: quotaRegistration, for: indexPath, item: name)
            case .device(let id):
                // 같은 식별자라도 통합 모드 하단에서는 한도 막대 없이 tok/s 만 그린다.
                if self?.viewMode == .unified {
                    return view.dequeueConfiguredReusableCell(using: rateRegistration, for: indexPath, item: id)
                }
                return view.dequeueConfiguredReusableCell(using: deviceRegistration, for: indexPath, item: id)
            }
        }

        let headerRegistration = UICollectionView.SupplementaryRegistration<UICollectionViewListCell>(
            elementKind: UICollectionView.elementKindSectionHeader
        ) { [weak self] header, _, indexPath in
            guard let self,
                  let section = self.dataSource.sectionIdentifier(for: indexPath.section) else { return }
            var content = UIListContentConfiguration.groupedHeader()
            content.text = section.headerTitle
            content.textProperties.color = Palette.subtle
            header.contentConfiguration = content
        }
        dataSource.supplementaryViewProvider = { view, _, indexPath in
            view.dequeueConfiguredReusableSupplementary(using: headerRegistration, for: indexPath)
        }

        // 통합 모드의 하단 섹션은 순서를 바꾸는 화면이 아니다 — 거기서 끌면 개별 보기의
        // sortIndex 만 소리 없이 바뀐다.
        dataSource.reorderingHandlers.canReorderItem = { [weak self] _ in self?.viewMode == .individual }
        // 놓은 뒤에만 저장한다 — 드래그 도중의 중간 순서를 Keychain 에 쓰면 손가락을 뗄
        // 때까지 쓰기가 계속 일어난다.
        dataSource.reorderingHandlers.didReorder = { [weak self] transaction in
            self?.persistOrder(transaction.finalSnapshot.itemIdentifiers.compactMap(\.deviceId))
        }

        let refresh = UIRefreshControl()
        refresh.addTarget(self, action: #selector(pulledToRefresh), for: .valueChanged)
        collectionView.refreshControl = refresh
    }

    /// 스와이프 삭제는 개별 모드에서만 단다. 섹션 헤더도 모드에 따라 달라지므로 레이아웃
    /// 자체를 모드마다 새로 만든다 — 개별 모드는 섹션이 하나뿐이라 헤더가 필요 없다.
    private func makeLayout(for mode: ViewMode) -> UICollectionViewCompositionalLayout {
        var config = UICollectionLayoutListConfiguration(appearance: .insetGrouped)
        config.backgroundColor = Palette.windowBackground
        switch mode {
        case .individual:
            // 목록에서 지울 수 있어야 16대 상한에 걸렸을 때 자리를 비울 수 있다.
            config.trailingSwipeActionsConfigurationProvider = { [weak self] indexPath in
                guard let self, let id = self.dataSource.itemIdentifier(for: indexPath)?.deviceId else { return nil }
                let delete = UIContextualAction(style: .destructive, title: "삭제") { [weak self] _, _, done in
                    self?.removeDevice(endpointIdHex: id)
                    done(true)
                }
                return UISwipeActionsConfiguration(actions: [delete])
            }
        case .unified:
            config.headerMode = .supplementary
            // 에이전트·장치 카드 사이의 구분선을 뺀다 — 셀마다 카드 배경이 있어 경계가
            // 이미 드러나는데 선까지 그으면 촘촘해 보인다(사용자 요청).
            config.showsSeparators = false
        }
        return UICollectionViewCompositionalLayout.list(using: config)
    }

    @objc private func viewModeChanged(_ sender: UISegmentedControl) {
        let mode: ViewMode = (sender.selectedSegmentIndex == 1) ? .unified : .individual
        guard mode != viewMode else { return }
        viewMode = mode
        UserDefaults.standard.set(mode.rawValue, forKey: Self.viewModeDefaultsKey)
        applyViewMode()
    }

    /// **fleet 은 다시 만들지 않는다.** 모드 전환은 표시 방식만 바꾸는 것이라 연결을 끊으면
    /// 안 된다 — `startFleet()` 을 부르면 16대의 QUIC 연결이 전부 끊긴다. 드래그 재정렬이
    /// 같은 이유로 `applySortOrder` 를 따로 두는 것과 같다(Ruling 38).
    private func applyViewMode() {
        renderedRows.removeAll()
        renderedAgentRows.removeAll()
        renderedRateRows.removeAll()
        collectionView.setCollectionViewLayout(makeLayout(for: viewMode), animated: false)
        collectionView.dragInteractionEnabled = (viewMode == .individual)
        // 섹션 구성 자체가 바뀌고 같은 식별자가 다른 셀 종류로 넘어가므로, 한 번 비우고
        // 새로 쌓는다. 모드 전환은 사용자가 탭할 때만 일어나므로 비용은 무시할 수 있다.
        dataSource.applySnapshotUsingReloadData(NSDiffableDataSourceSnapshot<Section, Item>())
        applySnapshot()
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
        renderedAgentRows.removeAll()
        renderedRateRows.removeAll()

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
        guard let session = session(for: id) else { return nil }
        return DeviceListPresentation.row(
            device: session.device, status: session.status, cached: cachedSnapshot(of: session), now: Date()
        )
    }

    private func session(for id: String) -> DeviceSession? {
        fleet?.sessions.first(where: { $0.device.endpointIdHex == id })
    }

    /// 세션이 들고 있는 값이 있으면 그걸 쓰고, 없으면 디스크 캐시로 떨어진다.
    ///
    /// 신선도는 **스냅샷을 받은 시각**으로 계산한다. 지금 시각을 쓰면 오프라인으로 떨어진
    /// 장치가 들고 있는 낡은 스냅샷이 영원히 "방금 전"으로 보인다.
    private func cachedSnapshot(of session: DeviceSession) -> CachedSnapshot? {
        let live = session.latest.flatMap { snapshot in
            session.latestAt.map { CachedSnapshot(snapshot: snapshot, fetchedAt: $0) }
        }
        return live ?? diskSnapshot(for: session.device.endpointIdHex)
    }

    /// 디스크 캐시는 장치당 한 번만 읽는다 — `load` 는 메인 스레드에서 파일 읽기 + JSON
    /// 디코드를 하므로 갱신마다 부르면 안 된다. fleet 을 다시 만들 때 비운다.
    private func diskSnapshot(for id: String) -> CachedSnapshot? {
        if let memo = cachedSnapshots[id] { return memo }
        let loaded = environment.cache.load(endpointIdHex: id)
        cachedSnapshots[id] = loaded
        return loaded
    }

    /// 표시 순서대로 정렬된 세션. 두 모드가 같은 정렬(`DeviceListPresentation.sorted`)을 쓴다.
    private func orderedSessions(_ fleet: DeviceFleet) -> [DeviceSession] {
        DeviceListPresentation.sorted(fleet.sessions.map { ($0.device, $0.status) })
            .compactMap { device in session(for: device.endpointIdHex) }
    }

    private func applySnapshot() {
        guard let fleet, let dataSource else { return }
        switch viewMode {
        case .individual: applyIndividualSnapshot(fleet: fleet, dataSource: dataSource)
        case .unified: applyUnifiedSnapshot(fleet: fleet, dataSource: dataSource)
        }
    }

    /// 첫 확인이 끝나기 전이면 로딩 카드 한 장만 깔고 true 를 돌려준다. 판정은
    /// `DeviceListPresentation.isAwaitingFirstResult` 한 곳에만 둔다 — 같은 규칙을
    /// 두 모드에 따로 적으면 어긋난다(이 저장소가 여러 번 겪은 실수다).
    private func applyLoadingSnapshotIfAwaiting(
        sessions: [DeviceSession], dataSource: UICollectionViewDiffableDataSource<Section, Item>
    ) -> Bool {
        let sources = sessions.map { (status: $0.status, cached: cachedSnapshot(of: $0)) }
        guard DeviceListPresentation.isAwaitingFirstResult(sources: sources) else { return false }
        let previous = Set(dataSource.snapshot().itemIdentifiers)
        guard previous != [.loading] else { return true }   // 이미 로딩 중이면 다시 적용하지 않는다
        var snapshot = NSDiffableDataSourceSnapshot<Section, Item>()
        snapshot.appendSections([.loading])
        snapshot.appendItems([.loading])
        dataSource.apply(snapshot, animatingDifferences: false)
        return true
    }

    private func applyIndividualSnapshot(
        fleet: DeviceFleet, dataSource: UICollectionViewDiffableDataSource<Section, Item>
    ) {
        let sessions = orderedSessions(fleet)
        if applyLoadingSnapshotIfAwaiting(sessions: sessions, dataSource: dataSource) { return }
        let ids = sessions.map(\.device.endpointIdHex)
        let previous = Set(dataSource.snapshot().itemIdentifiers)

        var models: [String: DeviceRowModel] = [:]
        var changed: [Item] = []
        for id in ids {
            guard let model = rowModel(for: id) else { continue }
            models[id] = model
            // 이미 화면에 있고 내용까지 그대로면 건드리지 않는다. 새로 삽입되는 id 를
            // 여기 넣으면 삽입과 갱신이 같은 스냅샷에서 겹쳐 적용이 거부된다.
            if previous.contains(.device(id)), renderedRows[id] != model { changed.append(.device(id)) }
        }
        renderedRows = models

        var snapshot = NSDiffableDataSourceSnapshot<Section, Item>()
        snapshot.appendSections([.main])
        snapshot.appendItems(ids.map(Item.device))
        snapshot.reconfigureItems(changed)
        // 구성이 그대로면 애니메이션할 것이 없다 — 스트리밍 중 매 갱신마다 셀이 깜빡인다.
        dataSource.apply(snapshot, animatingDifferences: previous != Set(ids.map(Item.device)))
    }

    /// 상단은 에이전트 종류별 한도(합산 규칙은 `UnifiedPresentation` — Ruling 40),
    /// 하단은 장치별 tok/s.
    private func applyUnifiedSnapshot(
        fleet: DeviceFleet, dataSource: UICollectionViewDiffableDataSource<Section, Item>
    ) {
        let sessions = orderedSessions(fleet)
        if applyLoadingSnapshotIfAwaiting(sessions: sessions, dataSource: dataSource) { return }
        let sources = sessions.map { (status: $0.status, cached: cachedSnapshot(of: $0)) }
        let agentRows = UnifiedPresentation.agentRows(sources: sources, now: Date())
        let deviceRows = sessions.map { session in
            UnifiedPresentation.deviceRow(
                device: session.device, status: session.status, cached: cachedSnapshot(of: session)
            )
        }

        let previous = Set(dataSource.snapshot().itemIdentifiers)
        var changed: [Item] = []
        for row in agentRows where previous.contains(.quotaAgent(row.name)) && renderedAgentRows[row.name] != row {
            changed.append(.quotaAgent(row.name))
        }
        for row in deviceRows where previous.contains(.device(row.id)) && renderedRateRows[row.id] != row {
            changed.append(.device(row.id))
        }
        renderedAgentRows = Dictionary(uniqueKeysWithValues: agentRows.map { ($0.name, $0) })
        renderedRateRows = Dictionary(uniqueKeysWithValues: deviceRows.map { ($0.id, $0) })

        var snapshot = NSDiffableDataSourceSnapshot<Section, Item>()
        snapshot.appendSections([.quota, .rates])
        snapshot.appendItems(agentRows.map { Item.quotaAgent($0.name) }, toSection: .quota)
        snapshot.appendItems(deviceRows.map { Item.device($0.id) }, toSection: .rates)
        snapshot.reconfigureItems(changed)
        dataSource.apply(snapshot, animatingDifferences: previous != Set(snapshot.itemIdentifiers))
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
    /// 통합 상단(한도)은 장치 하나에 대응하지 않으므로 선택할 수 없다.
    func collectionView(_ collectionView: UICollectionView, shouldSelectItemAt indexPath: IndexPath) -> Bool {
        dataSource.itemIdentifier(for: indexPath)?.deviceId != nil
    }

    func collectionView(_ collectionView: UICollectionView, didSelectItemAt indexPath: IndexPath) {
        collectionView.deselectItem(at: indexPath, animated: true)
        // 통합 하단의 장치 셀도 개별 보기와 같은 목적지 판정을 쓴다.
        guard let id = dataSource.itemIdentifier(for: indexPath)?.deviceId,
              let fleet,
              let session = session(for: id) else {
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
        // 통합 모드는 순서를 바꾸는 화면이 아니다 — 들어올려 봐야 놓을 때 취소된다.
        guard viewMode == .individual,
              let id = dataSource.itemIdentifier(for: indexPath)?.deviceId else { return [] }
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
        guard viewMode == .individual, session.localDragSession != nil else {
            return UICollectionViewDropProposal(operation: .forbidden)
        }
        guard let destinationIndexPath,
              let draggedId = session.localDragSession?.items.first?.localObject as? String,
              let destinationId = dataSource.itemIdentifier(for: destinationIndexPath)?.deviceId,
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
        session(for: id)?.status
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
