//! WebSocket Events for Real-Time Updates
//!
//! Provides real-time event streaming for:
//! - File upload/download progress
//! - Cluster health changes
//! - Job status updates

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Weak},
    time::Duration,
};
use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{debug, error, info, warn};

use crate::AppState;

// =============================================================================
// EVENT TYPES
// =============================================================================

/// Event types for WebSocket
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum Event {
    // File events
    FileCreated {
        bucket: String,
        key: String,
        size: u64,
    },
    FileDeleted {
        bucket: String,
        key: String,
    },
    UploadProgress {
        upload_id: String,
        bytes_uploaded: u64,
        total_bytes: u64,
        percent: f32,
    },
    UploadComplete {
        upload_id: String,
        bucket: String,
        key: String,
        etag: String,
    },
    DownloadProgress {
        download_id: String,
        bytes_downloaded: u64,
        total_bytes: u64,
        percent: f32,
    },

    // Cluster events
    NodeJoined {
        node_id: String,
        address: String,
    },
    NodeLeft {
        node_id: String,
        reason: String,
    },
    NodeHealthChanged {
        node_id: String,
        status: String,
        details: Option<String>,
    },

    // Replication events
    ReplicationStarted {
        chunk_id: String,
        source_node: String,
        target_node: String,
    },
    ReplicationComplete {
        chunk_id: String,
        target_node: String,
    },
    ReplicationFailed {
        chunk_id: String,
        error: String,
    },

    // Job events (for CyxWiz integration)
    JobStatusChanged {
        job_id: String,
        status: String,
        progress: Option<f32>,
    },

    // System events
    Heartbeat {
        timestamp: u64,
    },
    Error {
        code: String,
        message: String,
    },
}

impl Event {
    /// Get event category for filtering
    pub fn category(&self) -> &'static str {
        match self {
            Event::FileCreated { .. }
            | Event::FileDeleted { .. }
            | Event::UploadProgress { .. }
            | Event::UploadComplete { .. }
            | Event::DownloadProgress { .. } => "file",

            Event::NodeJoined { .. } | Event::NodeLeft { .. } | Event::NodeHealthChanged { .. } => {
                "cluster"
            }

            Event::ReplicationStarted { .. }
            | Event::ReplicationComplete { .. }
            | Event::ReplicationFailed { .. } => "replication",

            Event::JobStatusChanged { .. } => "job",

            Event::Heartbeat { .. } | Event::Error { .. } => "system",
        }
    }
}

// =============================================================================
// EVENT HUB
// =============================================================================

/// Central event hub for broadcasting events
pub struct EventHub {
    /// Broadcast channel for all events
    broadcast_tx: broadcast::Sender<Event>,

    /// Per-topic subscribers
    topic_subscribers: RwLock<HashMap<String, Vec<Weak<mpsc::Sender<Event>>>>>,

    /// Connected clients count
    client_count: RwLock<usize>,
}

impl EventHub {
    /// Create a new event hub
    pub fn new(capacity: usize) -> Self {
        let (broadcast_tx, _) = broadcast::channel(capacity);

        Self {
            broadcast_tx,
            topic_subscribers: RwLock::new(HashMap::new()),
            client_count: RwLock::new(0),
        }
    }

    /// Subscribe to all events
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.broadcast_tx.subscribe()
    }

    /// Subscribe to specific topics
    pub async fn subscribe_topics(&self, topics: Vec<String>) -> mpsc::Receiver<Event> {
        let (tx, rx) = mpsc::channel(32);
        let tx = Arc::new(tx);

        let mut subs = self.topic_subscribers.write().await;
        for topic in topics {
            subs.entry(topic)
                .or_insert_with(Vec::new)
                .push(Arc::downgrade(&tx));
        }

        rx
    }

    /// Publish an event to all subscribers
    pub async fn publish(&self, event: Event) {
        // Broadcast to all subscribers
        let _ = self.broadcast_tx.send(event.clone());

        // Also send to topic subscribers
        let category = event.category().to_string();
        let subs = self.topic_subscribers.read().await;

        if let Some(subscribers) = subs.get(&category) {
            for weak_tx in subscribers {
                if let Some(tx) = weak_tx.upgrade() {
                    let _ = tx.send(event.clone()).await;
                }
            }
        }
    }

    /// Get current client count
    pub async fn client_count(&self) -> usize {
        *self.client_count.read().await
    }

    /// Increment client count
    async fn add_client(&self) {
        let mut count = self.client_count.write().await;
        *count += 1;
        debug!(count = *count, "Client connected");
    }

    /// Decrement client count
    async fn remove_client(&self) {
        let mut count = self.client_count.write().await;
        *count = count.saturating_sub(1);
        debug!(count = *count, "Client disconnected");
    }

    /// Clean up dead topic subscribers
    pub async fn cleanup_subscribers(&self) {
        let mut subs = self.topic_subscribers.write().await;

        for subscribers in subs.values_mut() {
            subscribers.retain(|weak| weak.upgrade().is_some());
        }

        // Remove empty topics
        subs.retain(|_, v| !v.is_empty());
    }
}

// =============================================================================
// WEBSOCKET HANDLER
// =============================================================================

/// Query parameters for WebSocket connection
#[derive(Debug, Deserialize)]
pub struct WsQuery {
    /// Topics to subscribe to (comma-separated)
    #[serde(default)]
    pub topics: Option<String>,

    /// Authentication token
    #[serde(default)]
    pub token: Option<String>,
}

/// Create WebSocket routes
pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/ws", get(ws_handler))
}

/// WebSocket upgrade handler
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Query(query): Query<WsQuery>,
) -> impl IntoResponse {
    // Parse topics
    let topics: Vec<String> = query
        .topics
        .as_ref()
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();

    info!(topics = ?topics, "WebSocket connection request");

    ws.on_upgrade(move |socket| handle_socket(socket, state, topics))
}

/// Handle WebSocket connection
async fn handle_socket(socket: WebSocket, state: Arc<AppState>, topics: Vec<String>) {
    let (mut sender, mut receiver) = socket.split();

    // Subscribe to events
    let mut event_rx = if topics.is_empty() {
        // Subscribe to all events
        let rx = state.event_hub.subscribe();
        EventReceiver::Broadcast(rx)
    } else {
        // Subscribe to specific topics
        let rx = state.event_hub.subscribe_topics(topics).await;
        EventReceiver::Topic(rx)
    };

    state.event_hub.add_client().await;

    // Spawn heartbeat task
    let heartbeat_interval = Duration::from_secs(30);
    let (heartbeat_tx, mut heartbeat_rx) = mpsc::channel::<()>(1);

    tokio::spawn(async move {
        loop {
            tokio::time::sleep(heartbeat_interval).await;
            if heartbeat_tx.send(()).await.is_err() {
                break;
            }
        }
    });

    // Main loop
    loop {
        tokio::select! {
            // Handle incoming messages from client
            Some(msg) = receiver.next() => {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Err(e) = handle_client_message(&text, &state).await {
                            warn!(error = %e, "Error handling client message");
                        }
                    }
                    Ok(Message::Ping(data)) => {
                        if sender.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Ok(Message::Pong(_)) => {
                        // Client responded to ping
                    }
                    Ok(Message::Close(_)) => {
                        debug!("Client sent close frame");
                        break;
                    }
                    Err(e) => {
                        warn!(error = %e, "WebSocket error");
                        break;
                    }
                    _ => {}
                }
            }

            // Handle events to send to client
            event = event_rx.recv() => {
                match event {
                    Some(event) => {
                        let json = match serde_json::to_string(&event) {
                            Ok(j) => j,
                            Err(e) => {
                                error!(error = %e, "Failed to serialize event");
                                continue;
                            }
                        };

                        if sender.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                    }
                    None => {
                        // Channel closed
                        break;
                    }
                }
            }

            // Send heartbeat
            _ = heartbeat_rx.recv() => {
                let event = Event::Heartbeat {
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                };

                let json = serde_json::to_string(&event).unwrap_or_default();
                if sender.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
        }
    }

    state.event_hub.remove_client().await;
    debug!("WebSocket connection closed");
}

/// Enum to handle different receiver types
enum EventReceiver {
    Broadcast(broadcast::Receiver<Event>),
    Topic(mpsc::Receiver<Event>),
}

impl EventReceiver {
    async fn recv(&mut self) -> Option<Event> {
        match self {
            EventReceiver::Broadcast(rx) => rx.recv().await.ok(),
            EventReceiver::Topic(rx) => rx.recv().await,
        }
    }
}

/// Handle incoming client messages
async fn handle_client_message(text: &str, state: &AppState) -> Result<(), String> {
    // Parse as JSON command
    let cmd: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("Invalid JSON: {}", e))?;

    let action = cmd["action"]
        .as_str()
        .ok_or("Missing 'action' field")?;

    match action {
        "subscribe" => {
            // Client wants to subscribe to additional topics
            // This would require re-architecting the subscription model
            debug!("Subscribe request received (not implemented)");
        }
        "unsubscribe" => {
            debug!("Unsubscribe request received (not implemented)");
        }
        _ => {
            return Err(format!("Unknown action: {}", action));
        }
    }

    Ok(())
}

// =============================================================================
// EVENT PUBLISHERS
// =============================================================================

/// Helper functions to publish common events
impl AppState {
    /// Publish file created event
    pub async fn publish_file_created(&self, bucket: &str, key: &str, size: u64) {
        self.event_hub
            .publish(Event::FileCreated {
                bucket: bucket.to_string(),
                key: key.to_string(),
                size,
            })
            .await;
    }

    /// Publish file deleted event
    pub async fn publish_file_deleted(&self, bucket: &str, key: &str) {
        self.event_hub
            .publish(Event::FileDeleted {
                bucket: bucket.to_string(),
                key: key.to_string(),
            })
            .await;
    }

    /// Publish upload progress event
    pub async fn publish_upload_progress(
        &self,
        upload_id: &str,
        bytes_uploaded: u64,
        total_bytes: u64,
    ) {
        let percent = if total_bytes > 0 {
            (bytes_uploaded as f32 / total_bytes as f32) * 100.0
        } else {
            0.0
        };

        self.event_hub
            .publish(Event::UploadProgress {
                upload_id: upload_id.to_string(),
                bytes_uploaded,
                total_bytes,
                percent,
            })
            .await;
    }

    /// Publish node health changed event
    pub async fn publish_node_health_changed(
        &self,
        node_id: &str,
        status: &str,
        details: Option<String>,
    ) {
        self.event_hub
            .publish(Event::NodeHealthChanged {
                node_id: node_id.to_string(),
                status: status.to_string(),
                details,
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_category() {
        let event = Event::FileCreated {
            bucket: "test".to_string(),
            key: "file.txt".to_string(),
            size: 1024,
        };
        assert_eq!(event.category(), "file");

        let event = Event::NodeJoined {
            node_id: "n1".to_string(),
            address: "localhost:50051".to_string(),
        };
        assert_eq!(event.category(), "cluster");
    }

    #[test]
    fn test_event_serialization() {
        let event = Event::UploadProgress {
            upload_id: "abc123".to_string(),
            bytes_uploaded: 500,
            total_bytes: 1000,
            percent: 50.0,
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("UploadProgress"));
        assert!(json.contains("percent"));

        // Deserialize back
        let parsed: Event = serde_json::from_str(&json).unwrap();
        if let Event::UploadProgress { percent, .. } = parsed {
            assert!((percent - 50.0).abs() < 0.01);
        } else {
            panic!("Wrong event type");
        }
    }

    #[tokio::test]
    async fn test_event_hub_broadcast() {
        let hub = EventHub::new(16);

        let mut rx1 = hub.subscribe();
        let mut rx2 = hub.subscribe();

        let event = Event::Heartbeat { timestamp: 12345 };
        hub.publish(event.clone()).await;

        // Both receivers should get the event
        let received1 = rx1.recv().await.unwrap();
        let received2 = rx2.recv().await.unwrap();

        if let (Event::Heartbeat { timestamp: t1 }, Event::Heartbeat { timestamp: t2 }) =
            (received1, received2)
        {
            assert_eq!(t1, 12345);
            assert_eq!(t2, 12345);
        } else {
            panic!("Wrong event types");
        }
    }
}
