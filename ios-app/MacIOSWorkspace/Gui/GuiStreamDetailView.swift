import SwiftUI

struct GuiStreamDetailView: View {
    @Bindable var store: SupervisionStore
    let entry: SupervisionEntry

    @StateObject private var inputClient: InputClient
    @StateObject private var modState = ModifierState()
    @StateObject private var hwKbd = HardwareKeyboardDetector()
    @State private var lastDragLocation: CGPoint = .zero
    @State private var contentSize: CGSize = .zero
    @State private var showRetryBanner = false

    init(store: SupervisionStore, entry: SupervisionEntry) {
        self.store = store
        self.entry = entry
        _inputClient = StateObject(wrappedValue: InputClient(supId: entry.supId, glue: store.glue!))
    }

    var body: some View {
        VStack(spacing: 0) {
            ZStack {
                GuiStreamView(videoTrack: store.activeTrack)
                    .aspectRatio(CGFloat(entry.width) / CGFloat(entry.height), contentMode: .fit)
                    .background(GeometryReader { geo in
                        Color.clear
                            .onAppear { contentSize = geo.size }
                            .onChange(of: geo.size) { _, new in contentSize = new }
                    })
                    .gesture(tapGesture)
                    .simultaneousGesture(panGesture)
                HardwareKeyControllerView(inputClient: inputClient)
                    .allowsHitTesting(false)
                    .frame(width: 0, height: 0)
                if showRetryBanner {
                    permissionBanner
                }
            }
            if !hwKbd.isConnected {
                ModifierStickyRow(state: modState)
                SpecialKeyRow(input: inputClient, modState: modState)
            }
            GuiInputBar(input: inputClient, modState: modState)
        }
        .navigationTitle(entry.title.isEmpty ? entry.appName : entry.title)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button(role: .destructive) {
                    Task { await store.remove(supId: entry.supId) }
                } label: { Image(systemName: "stop.circle") }
            }
        }
        .onChange(of: store.lastInputAck) { _, ack in
            if let a = ack, a.code == "permission_denied" { showRetryBanner = true }
            if let a = ack, a.code == "ok" { showRetryBanner = false }
        }
    }

    private var tapGesture: some Gesture {
        SpatialTapGesture(coordinateSpace: .local)
            .onEnded { value in
                guard contentSize.width > 0 else { return }
                let nx = value.location.x / contentSize.width
                let ny = value.location.y / contentSize.height
                Task { await inputClient.tap(normalizedX: nx, normalizedY: ny) }
            }
    }

    private var panGesture: some Gesture {
        DragGesture(minimumDistance: 8)
            .onChanged { value in
                let dx = value.location.x - lastDragLocation.x
                let dy = value.location.y - lastDragLocation.y
                lastDragLocation = value.location
                Task { await inputClient.scroll(dx: dx, dy: dy) }
            }
            .onEnded { _ in lastDragLocation = .zero }
    }

    private var permissionBanner: some View {
        VStack {
            HStack {
                Image(systemName: "exclamationmark.triangle.fill")
                Text("Mac 未授予 Accessibility 权限")
                Spacer()
                Button("再试一次") { showRetryBanner = false }
            }
            .padding(8)
            .background(Color.yellow.opacity(0.9))
            Spacer()
        }
    }
}
