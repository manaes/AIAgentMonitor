import UIKit

final class SceneDelegate: UIResponder, UIWindowSceneDelegate {
    var window: UIWindow?
    private let environment = AppEnvironment()
    private var listViewController: DeviceListViewController?
    /// `sceneWillEnterForeground` 는 **콜드런치에도 호출**된다. 그때는 이미
    /// `viewDidLoad → startFleet()` 이 첫 라운드를 시작한 뒤라, 그대로 두면 한 벌이
    /// 통째로 취소되고 다시 걸리며 두 라운드가 겹쳐 동시성 상한(4)을 넘는다.
    private var hasEnteredForegroundOnce = false

    func scene(
        _ scene: UIScene,
        willConnectTo session: UISceneSession,
        options connectionOptions: UIScene.ConnectionOptions
    ) {
        guard let windowScene = scene as? UIWindowScene else { return }
        let list = DeviceListViewController(environment: environment)
        listViewController = list

        let window = UIWindow(windowScene: windowScene)
        window.rootViewController = UINavigationController(rootViewController: list)
        // Palette 이 고정 다크 팔레트라 라이트 모드 기기에서는 본문만 어둡고 내비바가
        // 밝아진다. 루트 윈도우에서 한 번 강제하면 모달·시트까지 전부 따라온다.
        window.overrideUserInterfaceStyle = .dark
        window.makeKeyAndVisible()
        self.window = window
    }

    /// iOS 가 앱을 suspend 하면 QUIC 연결이 전부 조용히 끊긴다 — 복귀 시 대상은
    /// "오프라인이던 장치"가 아니라 전부다. 단, 콜드런치의 첫 호출은 건너뛴다.
    func sceneWillEnterForeground(_ scene: UIScene) {
        guard hasEnteredForegroundOnce else {
            hasEnteredForegroundOnce = true
            return
        }
        listViewController?.reconnectAll()
    }

    /// 스냅샷 캐시 쓰기는 스로틀돼 있어 최대 30초 뒤처질 수 있다. 백그라운드로 갈 때
    /// 마지막 상태를 마저 써서 다음 실행이 목록을 미리 채울 수 있게 한다.
    func sceneDidEnterBackground(_ scene: UIScene) {
        listViewController?.flushCache()
    }
}
