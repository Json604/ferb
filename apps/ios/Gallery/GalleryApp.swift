import FerbGallery
import SwiftUI

// The design gallery (like Android's :gallery): every token, piece and screen of FerbDesign with invented data. It
// asks for no permission and touches nothing of FERB's.
@main
struct GalleryApp: App {
    var body: some Scene {
        WindowGroup { GalleryView() }
    }
}
