use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;

use crate::{
    WebRtcError,
    peer::{self, PeerConfig, PeerHandle},
    signaling::{IceCandidate, SdpAnswer, SdpOffer},
};

/// Manages WebRTC peer connections for the local host.
///
/// Accepts SDP offers from remote peers, creates peer connections, and runs
/// tasks that proxy data channel traffic to the local backend.
pub struct WebRtcHost {
    inner: Arc<Mutex<WebRtcHostInner>>,
}

struct WebRtcHostInner {
    peers: HashMap<String, PeerHandle>,
    local_backend_addr: SocketAddr,
    shutdown: CancellationToken,
}

impl WebRtcHost {
    pub fn new(local_backend_addr: SocketAddr, shutdown: CancellationToken) -> Self {
        Self {
            inner: Arc::new(Mutex::new(WebRtcHostInner {
                peers: HashMap::new(),
                local_backend_addr,
                shutdown,
            })),
        }
    }

    /// Accept an SDP offer and return an SDP answer.
    ///
    /// Creates a new peer connection and spawns its event loop task.
    pub async fn handle_offer(&self, offer: SdpOffer) -> Result<SdpAnswer, WebRtcError> {
        let session_id = offer.session_id.clone();

        let (peer_shutdown, local_backend_addr) = {
            let inner = self.inner.lock().await;
            (inner.shutdown.child_token(), inner.local_backend_addr)
        };

        let peer_connection = peer::new_peer_connection().await?;

        // Attach handlers before the offer is applied. `accept_offer` starts
        // ICE, and a data channel that arrives while `on_data_channel` is
        // unset is discarded for the lifetime of the connection — see
        // `peer::attach_handlers`.
        let disconnect_token = peer::attach_handlers(
            &peer_connection,
            PeerConfig {
                local_backend_addr,
                shutdown: peer_shutdown.clone(),
            },
        );

        let answer_sdp = peer::accept_offer(&peer_connection, &offer.sdp).await?;

        let old_peer = {
            let mut inner = self.inner.lock().await;
            let old_peer = inner.peers.remove(&session_id);

            let handle = PeerHandle {
                peer_connection: peer_connection.clone(),
                shutdown: peer_shutdown,
            };
            inner.peers.insert(session_id.clone(), handle);
            old_peer
        };

        // Clean up any existing peer with the same session ID.
        if let Some(old_peer) = old_peer {
            old_peer.shutdown.cancel();
            let _ = old_peer.peer_connection.close().await;
        }

        let inner_ref = Arc::clone(&self.inner);
        let peer_connection_for_task = peer_connection.clone();

        tokio::spawn(async move {
            if let Err(e) = peer::run_peer(peer_connection_for_task, disconnect_token).await {
                tracing::warn!(?e, %session_id, "WebRTC peer task failed");
            }

            // Remove self from the peer map on exit, but only if this peer is
            // still the registered one: a reconnect reuses the session ID, and
            // cancelling the old peer must not evict its replacement.
            let mut inner = inner_ref.lock().await;
            let is_current = inner
                .peers
                .get(&session_id)
                .is_some_and(|current| Arc::ptr_eq(&current.peer_connection, &peer_connection));
            if is_current {
                inner.peers.remove(&session_id);
            }
        });

        Ok(SdpAnswer {
            sdp: answer_sdp,
            session_id: offer.session_id,
        })
    }

    /// Add a trickle ICE candidate for an active peer session.
    pub async fn add_ice_candidate(&self, candidate: IceCandidate) -> Result<(), WebRtcError> {
        let peer_connection = {
            let inner = self.inner.lock().await;
            inner
                .peers
                .get(&candidate.session_id)
                .map(|peer| peer.peer_connection.clone())
                .ok_or_else(|| WebRtcError::SessionNotFound {
                    session_id: candidate.session_id.clone(),
                })?
        };

        let init = RTCIceCandidateInit {
            candidate: candidate.candidate,
            sdp_mid: candidate.sdp_mid,
            sdp_mline_index: candidate.sdp_m_line_index.map(|v| v as u16),
            ..Default::default()
        };

        peer_connection.add_ice_candidate(init).await?;

        Ok(())
    }
}
