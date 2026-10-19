//! Rust equivalent of the reference Channels WebSocket request bridge.
//!
//! The browser owns the authenticated `joinLive` call. Rust only sends a
//! narrow request and waits for its matching response; no session material is
//! copied out of the page.
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use tokio::{
    net::TcpListener,
    sync::{Mutex, mpsc, oneshot},
};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct LiveInfoRequest {
    pub username: String,
    pub object_id: String,
    pub object_nonce_id: String,
    pub live_id: String,
}

#[derive(Default)]
struct BridgeState {
    sender: Option<mpsc::UnboundedSender<Message>>,
    pending: HashMap<String, oneshot::Sender<Value>>,
}

#[derive(Clone)]
pub struct ChannelsBridge {
    pub address: SocketAddr,
    state: Arc<Mutex<BridgeState>>,
}

impl ChannelsBridge {
    pub async fn bind() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let state = Arc::new(Mutex::new(BridgeState::default()));
        let accept_state = Arc::clone(&state);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let state = Arc::clone(&accept_state);
                tokio::spawn(async move {
                    let _ = serve(stream, state).await;
                });
            }
        });
        Ok(Self { address, state })
    }

    pub async fn request_live_info(&self, request: LiveInfoRequest) -> Result<Value> {
        let id = Uuid::new_v4().to_string();
        let (reply_tx, reply_rx) = oneshot::channel();
        let message = json!({"type":"api_call","data":{"id":id,"key":"key:channels:live_info","data":{"username":request.username,"oid":request.object_id,"nid":request.object_nonce_id,"id":request.live_id}}});
        let mut state = self.state.lock().await;
        let sender = state
            .sender
            .clone()
            .context("Channels page is not connected")?;
        state.pending.insert(id, reply_tx);
        sender
            .send(Message::Text(message.to_string()))
            .context("sending Channels API request")?;
        drop(state);
        tokio::time::timeout(std::time::Duration::from_secs(10), reply_rx)
            .await
            .context("waiting for Channels joinLive response")?
            .context("Channels page disconnected before joinLive response")
    }
}

async fn serve(stream: tokio::net::TcpStream, state: Arc<Mutex<BridgeState>>) -> Result<()> {
    let socket = accept_async(stream)
        .await
        .context("accepting Channels WebSocket")?;
    let (mut write, mut read) = socket.split();
    let (sender, mut receiver) = mpsc::unbounded_channel();
    state.lock().await.sender = Some(sender);
    let writer = tokio::spawn(async move {
        while let Some(message) = receiver.recv().await {
            if write.send(message).await.is_err() {
                break;
            }
        }
    });
    while let Some(message) = read.next().await {
        let message = message?;
        let Message::Text(text) = message else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Some(id) = value.get("id").and_then(Value::as_str) else {
            continue;
        };
        if let Some(reply) = state.lock().await.pending.remove(id) {
            let _ = reply.send(value);
        }
    }
    writer.abort();
    state.lock().await.sender = None;
    Ok(())
}
