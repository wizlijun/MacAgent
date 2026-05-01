import SwiftUI

struct GuiStreamDetailView: View {
    @Bindable var store: SupervisionStore
    let entry: SupervisionEntry

    @StateObject private var inputClient: InputClient
    @StateObject private var modState = ModifierState()
    @StateObject private var hwKbd = HardwareKeyboardDetector()
    @State private var lastDragLocation: CGPoint? = nil
    @State private var contentSize: CGSize = .zero
    @State private var showRetryBanner = false
    @State private var fitToastVisible = false
    @State private var fitToastGen = 0

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
        .viewportTracking(store: store)
        .overlay(alignment: .top) {
            if fitToastVisible {
                HStack {
                    Image(systemName: "exclamationmark.triangle.fill")
                    Text("无法调整窗口尺寸（letterbox 显示）")
                        .font(.callout)
                    Spacer()
                }
                .padding(8)
                .background(Color.yellow.opacity(0.9))
                .transition(.move(edge: .top))
                .allowsHitTesting(false)
            }
        }
        .onChange(of: store.lastFitFailed) { _, new in
            guard new != nil else { return }
            // Bump generation so a stale dismiss closure can't kill a fresher toast.
            fitToastGen += 1
            let gen = fitToastGen
            fitToastVisible = true
            DispatchQueue.main.asyncAfter(deadline: .now() + 5) {
                if fitToastGen == gen { fitToastVisible = false }
            }
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
                defer { lastDragLocation = value.location }
                guard let last = lastDragLocation else { return }
                let dx = value.location.x - last.x
                let dy = value.location.y - last.y
                if dx != 0 || dy != 0 {
                    Task { await inputClient.scroll(dx: dx, dy: dy) }
                }
            }
            .onEnded { _ in lastDragLocation = nil }
    }

    private var permissionBanner: some View {
        VStack {
            HStack {
                Image(systemName: "exclamationmark.triangle.fill")
                Text(ErrorMessage.humanize("permission_denied"))
                Spacer()
                Button("再试一次") { showRetryBanner = false }
            }
            .padding(8)
            .background(Color.yellow.opacity(0.9))
            Spacer()
        }
    }
}
