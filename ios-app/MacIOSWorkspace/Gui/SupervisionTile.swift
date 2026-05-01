import SwiftUI

/// One supervised-window thumbnail tile (active = stream, armed = JPEG, dead = placeholder).
struct SupervisionTile: View {
    let entry: SupervisionEntry
    @Bindable var store: SupervisionStore

    var body: some View {
        VStack(spacing: 4) {
            ZStack {
                content
                if entry.status == .active { activeBadge }
            }
            .frame(maxWidth: .infinity)
            .aspectRatio(4.0 / 3.0, contentMode: .fit)
            .background(Color.gray.opacity(0.2))
            .clipShape(RoundedRectangle(cornerRadius: 8))

            Text(entry.appName).font(.caption).lineLimit(1)
            Text(entry.title).font(.caption2).foregroundStyle(.secondary).lineLimit(1)
        }
        .contextMenu {
            Button("移除", role: .destructive) {
                store.requestRemove(supId: entry.supId)
            }
        }
    }

    @ViewBuilder
    private var content: some View {
        if entry.status == .active {
            GuiStreamView(videoTrack: store.activeTrack)
        } else if let b64 = entry.thumbJpegB64,
                  let data = Data(base64Encoded: b64),
                  let img = UIImage(data: data) {
            Image(uiImage: img)
                .resizable()
                .aspectRatio(contentMode: .fit)
        } else {
            Image(systemName: "rectangle.dashed")
                .font(.largeTitle)
                .foregroundStyle(.secondary)
        }
    }

    private var activeBadge: some View {
        VStack {
            Spacer()
            Rectangle().fill(Color.green).frame(height: 3)
        }
    }
}
