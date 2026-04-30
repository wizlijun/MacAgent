import SwiftUI

struct ContentView: View {
    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: "macbook.and.iphone")
                .resizable()
                .scaledToFit()
                .frame(maxWidth: 120)
                .foregroundStyle(.tint)
            Text("macagent")
                .font(.largeTitle.bold())
            Text("v0.0.1 · M0 skeleton")
                .font(.subheadline)
                .foregroundStyle(.secondary)
        }
        .padding()
    }
}

#Preview {
    ContentView()
}
