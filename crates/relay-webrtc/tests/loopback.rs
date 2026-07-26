//! End-to-end loopback test for the WebRTC transport.
//!
//! Wires a [`WebRtcClient`] directly to a [`WebRtcHost`] in-process, skipping
//! the relay entirely, and proxies a real HTTP request over the data channel.
//! If this passes, the data-channel transport itself is sound and any remote
//! failure lives in signaling or ICE connectivity, not in this crate.

use std::{collections::HashMap, net::SocketAddr, time::Duration};

use relay_webrtc::{WebRtcClient, WebRtcHost};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;

/// Spawn a minimal HTTP backend that answers every request with `200 OK`.
///
/// Returns the bound address. The listener task lives for the test's duration.
async fn spawn_backend() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind backend");
    let addr = listener.local_addr().expect("backend addr");

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                // Read just the request head; the test never sends a body.
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let body = "pong";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });

    addr
}

#[tokio::test(flavor = "multi_thread")]
async fn http_request_round_trips_over_data_channel() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("relay_webrtc=trace")),
        )
        .with_test_writer()
        .try_init();

    // Deliberately does *not* install a rustls provider: the crate must stand
    // up its own DTLS stack without help from the embedding binary.
    let backend_addr = spawn_backend().await;
    let shutdown = CancellationToken::new();
    let host = WebRtcHost::new(backend_addr, shutdown.child_token());

    let offer = WebRtcClient::create_offer("loopback-session".to_string())
        .await
        .expect("create offer");

    let answer = host
        .handle_offer(offer.offer.clone())
        .await
        .expect("handle offer");

    let client = WebRtcClient::connect(offer, &answer.sdp, shutdown.child_token())
        .await
        .expect("connect");

    assert!(
        client.wait_until_connected(Duration::from_secs(15)).await,
        "data channel never opened"
    );

    let response = client
        .send_request("GET", "/ping", HashMap::new(), None)
        .await
        .expect("send request over data channel");

    assert_eq!(response.status, 200);

    use base64::Engine as _;
    let body = response
        .body_b64
        .as_deref()
        .map(|b64| {
            base64::engine::general_purpose::STANDARD
                .decode(b64)
                .expect("decode body")
        })
        .expect("response body");
    assert_eq!(String::from_utf8_lossy(&body), "pong");

    client.shutdown().await;
    shutdown.cancel();
}
