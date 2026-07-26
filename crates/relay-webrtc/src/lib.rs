pub mod client;
pub mod error;
pub mod fragment;
pub mod host;
pub mod peer;
pub mod proxy;
pub mod signaling;

pub use client::{WebRtcClient, WebRtcClientError, WsConnection, WsOpenResult};
pub use error::WebRtcError;
pub use host::WebRtcHost;
pub use proxy::{
    DataChannelMessage, DataChannelRequest, DataChannelResponse, DataChannelWsStream, WsClose,
    WsError, WsFrame, WsOpen, WsOpened,
};
pub use signaling::{IceCandidate, SdpAnswer, SdpOffer};

/// Ensure a process-level rustls crypto provider exists.
///
/// DTLS runs inside detached tasks, so a missing provider surfaces as a panic
/// on a runtime worker thread rather than an error: ICE connects, the data
/// channel never opens, and writes fail with `ErrClosedPipe`. Binaries that
/// install their own provider during startup still win — this only fills the
/// gap for those that do not.
fn ensure_crypto_provider() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        }
    });
}

/// Build a webrtc API restricted to UDP4 (IPv4 only).
///
/// Without this, the ICE agent tries IPv6 STUN which times out on most
/// networks and blocks ICE gathering.
fn build_api() -> webrtc::api::API {
    use webrtc::api::setting_engine::SettingEngine;
    use webrtc_ice::network_type::NetworkType;

    ensure_crypto_provider();

    let mut se = SettingEngine::default();
    se.set_network_types(vec![NetworkType::Udp4]);
    webrtc::api::APIBuilder::new()
        .with_setting_engine(se)
        .build()
}
