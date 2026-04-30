import SwiftUI

struct ContentView: View {
    @State var store = PairStore()

    var body: some View {
        switch store.state {
        case .unpaired:
            UnpairedView(store: store)
        case .paired(let pair):
            PairedView(pair: pair, store: store)
        }
    }
}

struct UnpairedView: View {
    @State var store: PairStore
    @State var presenting = false
    @State var error: String?

    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: "qrcode.viewfinder")
                .resizable()
                .scaledToFit()
                .frame(maxWidth: 80)
                .foregroundStyle(.tint)
            Text("macagent").font(.largeTitle.bold())
            Text("v0.0.1 · M1 unpaired").font(.subheadline).foregroundStyle(.secondary)
            Button("扫码配对 Mac") { presenting = true }.buttonStyle(.borderedProminent)
            if let err = error { Text(err).foregroundStyle(.red).font(.footnote) }
        }
        .padding()
        .sheet(isPresented: $presenting) {
            QRScannerView { json in
                presenting = false
                Task {
                    do { try await PairingFlow.claim(scannedJSON: json, store: store) }
                    catch { self.error = "\(error)" }
                }
            }
        }
    }
}

#Preview {
    ContentView()
}
