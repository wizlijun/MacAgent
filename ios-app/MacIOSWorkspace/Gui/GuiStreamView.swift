import SwiftUI
import WebRTC

struct GuiStreamView: UIViewRepresentable {
    let videoTrack: RTCVideoTrack?

    func makeUIView(context: Context) -> RTCMTLVideoView {
        let view = RTCMTLVideoView()
        view.videoContentMode = .scaleAspectFit
        if let track = videoTrack {
            track.add(view)
            context.coordinator.attachedTrack = track
            context.coordinator.attachedView = view
        }
        return view
    }

    func updateUIView(_ uiView: RTCMTLVideoView, context: Context) {
        if context.coordinator.attachedTrack !== videoTrack {
            context.coordinator.attachedTrack?.remove(uiView)
            if let track = videoTrack {
                track.add(uiView)
                context.coordinator.attachedTrack = track
            } else {
                context.coordinator.attachedTrack = nil
            }
            context.coordinator.attachedView = uiView
        }
    }

    static func dismantleUIView(_ uiView: RTCMTLVideoView, coordinator: Coordinator) {
        coordinator.attachedTrack?.remove(uiView)
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    final class Coordinator {
        var attachedTrack: RTCVideoTrack?
        weak var attachedView: RTCMTLVideoView?
    }
}
