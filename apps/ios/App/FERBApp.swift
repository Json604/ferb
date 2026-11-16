import FerbAppUI
import SwiftUI

// The FERB app: everything is in apps/ios/Packages (FerbAppUI's root, FerbAppModel, FerbCore, FerbMobile, FerbDesign).
@main
struct FERBApp: App {
    @UIApplicationDelegateAdaptor(FerbAppDelegate.self) private var delegate

    var body: some Scene {
        WindowGroup {
            FerbRoot(app: delegate.app)
        }
    }
}
