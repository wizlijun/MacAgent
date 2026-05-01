import SwiftUI

/// Reports detail-view geometry to SupervisionStore on appear and on size change.
struct ViewportTracker: ViewModifier {
    let store: SupervisionStore

    func body(content: Content) -> some View {
        content.background(
            GeometryReader { geo in
                Color.clear
                    .onAppear { store.reportViewport(w: geo.size.width, h: geo.size.height) }
                    .onChange(of: geo.size) { _, newSize in
                        store.reportViewport(w: newSize.width, h: newSize.height)
                    }
            }
        )
    }
}

extension View {
    /// Apply ViewportTracker to forward size changes to the active supervision entry.
    func viewportTracking(store: SupervisionStore) -> some View {
        modifier(ViewportTracker(store: store))
    }
}
