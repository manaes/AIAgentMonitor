import BLETransport
// 이 파일은 App(전체지원, NETWORK_TRANSPORT 켜짐)과 AppBLE(BLE 전용) 두 타깃이
// 같은 소스로 컴파일한다. 두 타깃의 MirrorFeature 모듈 이름 자체가 다르므로
// (MirrorFeature vs MirrorFeatureBLE) import 도 갈라야 한다 — Project.swift 상단
// 주석 참고(NetworkTransport 를 import 하려면 모듈 자체의 최소 배포 타깃 이상이어야
// 하는데, `#if` 로 아예 컴파일에서 빼는 것 말고는 우회할 방법이 없다).
#if NETWORK_TRANSPORT
import MirrorFeature
import NetworkTransport
#else
import MirrorFeatureBLE
#endif
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
        #if NETWORK_TRANSPORT
        let root = MirrorViewController(
            bleClient: BLEClient(),
            networkClient: NetworkClient(),
            initialTransport: MirrorViewController.preferredTransport
        )
        #else
        let root = MirrorViewController(
            bleClient: BLEClient(),
            initialTransport: MirrorViewController.preferredTransport
        )
        #endif
        w.rootViewController = UINavigationController(rootViewController: root)
        w.makeKeyAndVisible()
        window = w
    }
}
