//! macagent-core::rtc_peer
//!
//! 单 PeerConnection 封装。M2 仅暴露 offer/answer/ice/ctrl-channel 4 类操作。
//! M5 加 H.264 video track（add_local_h264_video_track）。
//! 更多 DataChannel（pty/clip/input）由 macagent-app 处理。
//!
//! ## ICE strategy
//! `create_offer` / `create_answer` wait for ICE gathering to complete before returning,
//! so the returned SDP already contains all candidates (vanilla ICE).
//! Trickle ICE (via `on_local_candidate`) is also supported but optional.

use anyhow::{Context, Result};
use bytes::Bytes;
use std::sync::Arc;
use std::time::Duration;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_candidate::{RTCIceCandidate, RTCIceCandidateInit};
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::media::Sample;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::offer_answer_options::RTCOfferOptions;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::TrackLocal;

pub struct IceServer {
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerState {
    New,
    Connecting,
    Connected,
    Disconnected,
    Failed,
    Closed,
}

/// Handle to a locally added H.264 video track. Use `push_sample` to feed encoded frames.
pub struct VideoTrackHandle {
    track: Arc<TrackLocalStaticSample>,
}

impl VideoTrackHandle {
    /// Push one H.264 NALU sample. `duration` is the frame display duration (e.g. 33 ms at 30 fps).
    /// Returns Ok(()) even if the peer is not yet connected (webrtc-rs discards the sample silently).
    pub async fn push_sample(&self, data: Bytes, duration: Duration) -> Result<()> {
        self.track
            .write_sample(&Sample {
                data,
                duration,
                ..Default::default()
            })
            .await?;
        Ok(())
    }
}

pub struct RtcPeer {
    pc: Arc<RTCPeerConnection>,
}

pub struct CtrlChannel {
    dc: Arc<RTCDataChannel>,
}

impl RtcPeer {
    pub async fn new(ice: Vec<IceServer>) -> Result<Self> {
        let mut m = MediaEngine::default();
        m.register_default_codecs()
            .context("register default codecs")?;

        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut m)
            .context("register default interceptors")?;

        let api = APIBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .build();

        let cfg = RTCConfiguration {
            ice_servers: ice
                .into_iter()
                .map(|s| RTCIceServer {
                    urls: s.urls,
                    username: s.username.unwrap_or_default(),
                    credential: s.credential.unwrap_or_default(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };

        let pc = api.new_peer_connection(cfg).await?;
        Ok(Self { pc: Arc::new(pc) })
    }

    /// Create the "ctrl" DataChannel and return a handle to it.
    /// Must be called before `create_offer` so the SDP includes a data m= line.
    pub async fn open_ctrl_channel(&self) -> Result<CtrlChannel> {
        let dc = self
            .pc
            .create_data_channel("ctrl", None)
            .await
            .context("create_data_channel ctrl")?;
        Ok(CtrlChannel { dc })
    }

    /// Create an SDP offer, wait for ICE gathering to complete, and return the
    /// complete SDP string (includes all local candidates).
    pub async fn create_offer(&self) -> Result<String> {
        let offer = self.pc.create_offer(None).await.context("create_offer")?;

        // Register gathering-complete promise BEFORE set_local_description to
        // avoid a race where gathering finishes before we await the channel.
        let mut gathering_complete = self.pc.gathering_complete_promise().await;

        self.pc
            .set_local_description(offer)
            .await
            .context("set_local_description (offer)")?;

        // Wait for all local ICE candidates to be gathered.
        let _ = gathering_complete.recv().await;

        let desc = self
            .pc
            .local_description()
            .await
            .context("local_description after gathering")?;
        Ok(desc.sdp)
    }

    /// Create an SDP answer (after `apply_remote_offer`), wait for ICE gathering,
    /// and return the complete SDP string.
    pub async fn create_answer(&self) -> Result<String> {
        let answer = self.pc.create_answer(None).await.context("create_answer")?;

        let mut gathering_complete = self.pc.gathering_complete_promise().await;

        self.pc
            .set_local_description(answer)
            .await
            .context("set_local_description (answer)")?;

        let _ = gathering_complete.recv().await;

        let desc = self
            .pc
            .local_description()
            .await
            .context("local_description after gathering")?;
        Ok(desc.sdp)
    }

    /// Apply a remote SDP offer (answerer side).
    pub async fn apply_remote_offer(&self, sdp: &str) -> Result<()> {
        let desc =
            RTCSessionDescription::offer(sdp.to_string()).context("parse remote offer SDP")?;
        self.pc
            .set_remote_description(desc)
            .await
            .context("set_remote_description (offer)")?;
        Ok(())
    }

    /// Apply a remote SDP answer (offerer side).
    pub async fn apply_remote_answer(&self, sdp: &str) -> Result<()> {
        let desc =
            RTCSessionDescription::answer(sdp.to_string()).context("parse remote answer SDP")?;
        self.pc
            .set_remote_description(desc)
            .await
            .context("set_remote_description (answer)")?;
        Ok(())
    }

    /// Apply a remote ICE candidate (trickle ICE; optional when using vanilla ICE).
    /// `candidate_json` is a JSON-encoded `RTCIceCandidateInit`.
    pub async fn apply_remote_candidate(&self, candidate_json: &str) -> Result<()> {
        let init: RTCIceCandidateInit =
            serde_json::from_str(candidate_json).context("parse ICE candidate JSON")?;
        self.pc
            .add_ice_candidate(init)
            .await
            .context("add_ice_candidate")?;
        Ok(())
    }

    /// Register a callback for locally gathered ICE candidates (trickle ICE).
    /// The callback receives a JSON string (RTCIceCandidateInit serialized).
    /// None-candidates (gathering complete) are filtered out.
    pub async fn on_local_candidate(&self, cb: impl Fn(String) + Send + Sync + 'static) {
        self.pc
            .on_ice_candidate(Box::new(move |c: Option<RTCIceCandidate>| {
                if let Some(candidate) = c {
                    if let Ok(init) = candidate.to_json() {
                        if let Ok(json) = serde_json::to_string(&init) {
                            cb(json);
                        }
                    }
                }
                Box::pin(async {})
            }));
    }

    /// Register a callback for connection state changes.
    pub async fn on_state_change(&self, cb: impl Fn(PeerState) + Send + Sync + 'static) {
        self.pc
            .on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
                cb(map_state(s));
                Box::pin(async {})
            }));
    }

    /// Return the current connection state without registering a callback.
    pub async fn connection_state(&self) -> PeerState {
        map_state(self.pc.connection_state())
    }

    /// Trigger an ICE restart by creating a new offer with `ice_restart = true`.
    /// The returned offer SDP must be relayed to the remote peer (via signaling).
    pub async fn restart_ice(&self) -> Result<String> {
        let opts = RTCOfferOptions {
            ice_restart: true,
            ..Default::default()
        };
        let offer = self
            .pc
            .create_offer(Some(opts))
            .await
            .context("create_offer for ICE restart")?;

        let mut gathering_complete = self.pc.gathering_complete_promise().await;
        self.pc
            .set_local_description(offer)
            .await
            .context("set_local_description for ICE restart")?;
        let _ = gathering_complete.recv().await;

        let desc = self
            .pc
            .local_description()
            .await
            .context("local_description after ICE restart gathering")?;
        Ok(desc.sdp)
    }

    /// Add a local H.264 video track to this PeerConnection.
    /// Returns a `VideoTrackHandle` for pushing encoded frames.
    ///
    /// **Must be called before `create_offer` / `create_answer`** so that the
    /// negotiated SDP contains a `m=video` section.
    pub async fn add_local_h264_video_track(&self) -> Result<VideoTrackHandle> {
        let codec = RTCRtpCodecCapability {
            mime_type: "video/H264".to_string(),
            clock_rate: 90_000,
            channels: 0,
            sdp_fmtp_line: "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
                .to_string(),
            ..Default::default()
        };
        let track = Arc::new(TrackLocalStaticSample::new(
            codec,
            "video-cap".to_string(),
            "macagent".to_string(),
        ));
        self.pc
            .add_track(track.clone() as Arc<dyn TrackLocal + Send + Sync>)
            .await
            .context("add H.264 video track")?;
        Ok(VideoTrackHandle { track })
    }

    /// Close the PeerConnection.
    pub async fn close(&self) -> Result<()> {
        self.pc.close().await.context("close peer connection")?;
        Ok(())
    }
}

impl CtrlChannel {
    /// Send a UTF-8 text message over the DataChannel.
    pub async fn send_text(&self, msg: &str) -> Result<()> {
        self.dc
            .send_text(msg.to_string())
            .await
            .context("send_text on ctrl channel")?;
        Ok(())
    }

    /// Register a callback for incoming text messages.
    pub async fn on_message(&self, cb: impl Fn(String) + Send + Sync + 'static) {
        self.dc.on_message(Box::new(move |msg: DataChannelMessage| {
            if msg.is_string {
                if let Ok(s) = String::from_utf8(msg.data.to_vec()) {
                    cb(s);
                }
            }
            Box::pin(async {})
        }));
    }

    /// Register a callback for when the DataChannel opens.
    pub async fn on_open(&self, cb: impl Fn() + Send + Sync + 'static) {
        self.dc.on_open(Box::new(move || {
            cb();
            Box::pin(async {})
        }));
    }

    /// Register a callback for when the DataChannel closes.
    pub async fn on_close(&self, cb: impl Fn() + Send + Sync + 'static) {
        self.dc.on_close(Box::new(move || {
            cb();
            Box::pin(async {})
        }));
    }
}

fn map_state(s: RTCPeerConnectionState) -> PeerState {
    match s {
        RTCPeerConnectionState::New => PeerState::New,
        RTCPeerConnectionState::Connecting => PeerState::Connecting,
        RTCPeerConnectionState::Connected => PeerState::Connected,
        RTCPeerConnectionState::Disconnected => PeerState::Disconnected,
        RTCPeerConnectionState::Failed => PeerState::Failed,
        RTCPeerConnectionState::Closed => PeerState::Closed,
        _ => PeerState::Disconnected,
    }
}
