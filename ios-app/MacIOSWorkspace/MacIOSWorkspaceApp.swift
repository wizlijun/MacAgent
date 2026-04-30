import SwiftUI

@main
struct MacIOSWorkspaceApp: App {
    @UIApplicationDelegateAdaptor(PushHandler.self) var pushHandler

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(pushHandler)
                .task {
                    await pushHandler.requestAuthorizationAndRegister()
                }
        }
    }
}
