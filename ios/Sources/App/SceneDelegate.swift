import BLETransport
import MirrorFeature
import NetworkTransport
import UIKit

final class SceneDelegate: UIResponder, UIWindowSceneDelegate {
    var window: UIWindow?

    func scene(
        _ scene: UIScene,
        willConnectTo session: UISceneSession,
        options connectionOptions: UIScene.ConnectionOptions
    ) {
        guard let windowScene = scene as? UIWindowScene else { return }
        let w = UIWindow(windowScene: windowScene)
        let root = MirrorViewController(bleClient: BLEClient(), networkClient: NetworkClient())
        w.rootViewController = UINavigationController(rootViewController: root)
        w.makeKeyAndVisible()
        window = w
    }
}
