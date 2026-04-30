import Combine
import UIKit
import UserNotifications

@MainActor
final class PushHandler: NSObject, ObservableObject, UIApplicationDelegate {
    /// Shared last-known APNs token; readable from any isolation context.
    nonisolated(unsafe) static var currentTokenHex: String?

    @Published var deviceTokenHex: String?
    @Published var lastDeepLink: String?
    private let center = UNUserNotificationCenter.current()

    override init() {
        super.init()
        center.delegate = self
    }

    func requestAuthorizationAndRegister() async {
        do {
            let granted = try await center.requestAuthorization(options: [.alert, .sound, .badge])
            if granted {
                await MainActor.run {
                    UIApplication.shared.registerForRemoteNotifications()
                }
            }
        } catch {
            print("[PushHandler] requestAuthorization error:", error)
        }
    }

    func application(_ application: UIApplication,
                     didRegisterForRemoteNotificationsWithDeviceToken token: Data) {
        let hex = token.map { String(format: "%02x", $0) }.joined()
        deviceTokenHex = hex
        Self.currentTokenHex = hex
        print("[PushHandler] APNs token:", hex)
    }

    func application(_ application: UIApplication,
                     didFailToRegisterForRemoteNotificationsWithError error: Error) {
        print("[PushHandler] APNs register failed:", error)
    }
}

extension PushHandler: UNUserNotificationCenterDelegate {
    func userNotificationCenter(_ center: UNUserNotificationCenter,
                                willPresent notification: UNNotification) async -> UNNotificationPresentationOptions {
        return [.banner, .sound]
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter,
                                didReceive response: UNNotificationResponse) async {
        let userInfo = response.notification.request.content.userInfo
        if let deeplink = userInfo["deeplink"] as? String {
            await MainActor.run { lastDeepLink = deeplink }
        }
    }
}
