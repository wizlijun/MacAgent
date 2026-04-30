import SwiftUI

struct PairedView: View {
    let pair: PairStore.PairedPair
    @State var store: PairStore

    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: "checkmark.seal.fill")
                .resizable()
                .scaledToFit()
                .frame(maxWidth: 80)
                .foregroundStyle(.green)
            Text("已配对").font(.title.bold())
            Text("pair_id: \(pair.pairId.prefix(8))…")
                .font(.caption)
                .foregroundStyle(.secondary)
            Button("撤销并重新配对") { try? store.revoke() }
                .buttonStyle(.bordered)
                .tint(.red)
        }.padding()
    }
}
