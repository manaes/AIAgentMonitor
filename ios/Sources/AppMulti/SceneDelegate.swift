import UIKit

final class SceneDelegate: UIResponder, UIWindowSceneDelegate {
    var window: UIWindow?
    private let environment = AppEnvironment()
    private var listViewController: DeviceListViewController?

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
        window.makeKeyAndVisible()
        self.window = window
    }

    /// iOS 가 앱을 suspend 하면 QUIC 연결이 전부 조용히 끊긴다 — 복귀 시 대상은
    /// "오프라인이던 장치"가 아니라 전부다.
    func sceneWillEnterForeground(_ scene: UIScene) {
        listViewController?.reconnectAll()
    }
}
