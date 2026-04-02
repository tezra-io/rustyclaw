//! Integration test: Axon channel round-trip message delivery.
//!
//! Requires a live Axon broker at `~/.axon/broker.sock` and keys for
//! "rusty" and "aira" in `~/.axon/keys/`.
//!
//! Run with: `cargo test --test axon_integration -- --include-ignored`

use std::path::PathBuf;
use std::time::Duration;

use rustyclaw::channels::traits::{Channel, SendMessage};
use rustyclaw::channels::AxonChannel;

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME must be set"))
}

fn broker_socket() -> PathBuf {
    home().join(".axon/broker.sock")
}

fn keys_dir() -> PathBuf {
    home().join(".axon/keys")
}

#[tokio::test]
#[ignore]
async fn axon_round_trip_message_delivery() {
    let socket = broker_socket();
    let keys = keys_dir();

    // Receiver: "rusty" listens on the event socket
    let rusty = AxonChannel::new(
        "rusty".to_string(),
        socket.clone(),
        keys.clone(),
        500,
        vec![],
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);

    // Spawn listener in background — listen() is long-running with reconnect loop
    let listener = tokio::spawn(async move {
        let _ = rusty.listen(tx).await;
    });

    // Give the listener time to connect and register
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Sender: "aira" sends via a cmd socket
    let aira = AxonChannel::new("aira".to_string(), socket, keys, 500, vec![]);

    aira.send(&SendMessage::new("integration test hello", "rusty"))
        .await
        .expect("send should succeed");

    // Wait for the message to arrive
    let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out waiting for message")
        .expect("channel closed without message");

    assert_eq!(msg.content, "integration test hello");
    // Note: The broker stamps `from` with the agent's internal ULID, not the
    // friendly name "aira". Sender resolution (ULID → name) is a future enhancement.
    // We verify the field is populated (non-empty) rather than matching the name.
    assert!(
        !msg.sender.is_empty(),
        "sender should be populated by broker"
    );
    assert_eq!(msg.channel, "axon");

    listener.abort();
}
