//! Per-task WebSocket state feed compatible with the reference download SDK.
use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use tokio::{
    net::TcpListener,
    sync::{Mutex, broadcast},
};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        Message,
        handshake::server::{Request, Response},
    },
};

#[derive(Clone, Serialize)]
pub struct TaskSnapshot {
    pub id: String,
    pub status: String,
    pub name: String,
    pub error: Option<String>,
}

/// Shape consumed by the unmodified reference `public/dl.sdk.js`.  In
/// particular, that SDK deliberately ignores a bare task object received over
/// the socket; it accepts task records only inside a `task_upsert` or
/// `task_update` envelope.
#[derive(Serialize)]
struct TaskUpsert<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    tasks: &'a [TaskSnapshot],
}

fn websocket_update(snapshot: &TaskSnapshot) -> Result<String> {
    serde_json::to_string(&TaskUpsert {
        kind: "task_upsert",
        tasks: std::slice::from_ref(snapshot),
    })
    .map_err(Into::into)
}

#[derive(Clone, Default)]
pub struct DownloadTaskBridge {
    states: Arc<Mutex<HashMap<String, TaskSnapshot>>>,
    feeds: Arc<Mutex<HashMap<String, broadcast::Sender<TaskSnapshot>>>>,
    scopes: Arc<Mutex<HashMap<String, (String, String, i64)>>>,
}

impl DownloadTaskBridge {
    pub async fn register_recording(
        &self,
        id: &str,
        platform: crate::model::Platform,
        page: &url::Url,
    ) {
        self.scopes.lock().await.insert(
            id.into(),
            (
                platform.as_str().into(),
                page.to_string(),
                chrono::Utc::now().timestamp_millis(),
            ),
        );
    }
    pub async fn recording_list(&self) -> Vec<serde_json::Value> {
        let list = self.list().await;
        let scopes = self.scopes.lock().await;
        list.into_iter()
            .map(|t| {
                let mut v = serde_json::to_value(&t).unwrap();
                if let Some((platform, page, created)) = scopes.get(&t.id) {
                    v["platform"] = serde_json::json!(platform);
                    v["page_url"] = serde_json::json!(page);
                    v["created_at_ms"] = serde_json::json!(created);
                }
                v
            })
            .collect()
    }
    pub async fn list(&self) -> Vec<TaskSnapshot> {
        let mut tasks: Vec<_> = self.states.lock().await.values().cloned().collect();
        tasks.sort_by(|left, right| left.id.cmp(&right.id));
        tasks
    }

    pub async fn detail(&self, id: &str) -> Option<TaskSnapshot> {
        self.states.lock().await.get(id).cloned()
    }

    pub async fn set_status(&self, id: &str, status: &str) -> Option<TaskSnapshot> {
        let mut states = self.states.lock().await;
        let snapshot = states.get_mut(id)?;
        snapshot.status = status.to_owned();
        let snapshot = snapshot.clone();
        drop(states);
        let sender = self.feeds.lock().await.get(id).cloned();
        if let Some(sender) = sender {
            let _ = sender.send(snapshot.clone());
        }
        Some(snapshot)
    }

    pub async fn retry(&self, id: &str) -> Option<TaskSnapshot> {
        self.set_status(id, "downloading").await
    }

    pub async fn start(&self, id: &str) -> Option<TaskSnapshot> {
        self.set_status(id, "downloading").await
    }
    pub async fn pause(&self, id: &str) -> Option<TaskSnapshot> {
        self.set_status(id, "paused").await
    }

    pub async fn delete(&self, id: &str) -> Option<TaskSnapshot> {
        let snapshot = self.states.lock().await.remove(id)?;
        if let Some(sender) = self.feeds.lock().await.get(id).cloned() {
            let _ = sender.send(TaskSnapshot {
                id: snapshot.id.clone(),
                status: "deleted".into(),
                name: snapshot.name.clone(),
                error: None,
            });
        }
        Some(snapshot)
    }

    pub async fn publish(&self, snapshot: TaskSnapshot) {
        self.states
            .lock()
            .await
            .insert(snapshot.id.clone(), snapshot.clone());
        let mut feeds = self.feeds.lock().await;
        let sender = feeds
            .entry(snapshot.id.clone())
            .or_insert_with(|| broadcast::channel(32).0);
        let _ = sender.send(snapshot);
    }
    pub async fn bind(self) -> Result<SocketAddr> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let bridge = self.clone();
                tokio::spawn(async move {
                    let _ = bridge.serve(stream).await;
                });
            }
        });
        Ok(address)
    }
    async fn serve(&self, stream: tokio::net::TcpStream) -> Result<()> {
        let task_id = Arc::new(std::sync::Mutex::new(String::new()));
        let captured = Arc::clone(&task_id);
        let socket = accept_hdr_async(stream, move |request: &Request, response: Response| {
            if let Some(query) = request.uri().query() {
                if let Some((_, value)) =
                    url::form_urlencoded::parse(query.as_bytes()).find(|(key, _)| key == "task_id")
                {
                    *captured.lock().unwrap() = value.into_owned();
                }
            }
            Ok(response)
        })
        .await?;
        let task_id = task_id.lock().unwrap().clone();
        if task_id.is_empty() {
            return Ok(());
        }
        let (mut write, mut read) = socket.split();
        if let Some(snapshot) = self.states.lock().await.get(&task_id).cloned() {
            write
                .send(Message::Text(websocket_update(&snapshot)?))
                .await?;
        }
        let sender = self
            .feeds
            .lock()
            .await
            .entry(task_id)
            .or_insert_with(|| broadcast::channel(32).0)
            .clone();
        let mut updates = sender.subscribe();
        loop {
            tokio::select! {
                incoming = read.next() => match incoming {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(payload))) => write.send(Message::Pong(payload)).await?,
                    Some(Ok(_)) => {},
                    Some(Err(_)) => break,
                },
                update = updates.recv() => match update {
                    Ok(snapshot) => write.send(Message::Text(websocket_update(&snapshot)?)).await?,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                },
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn websocket_uses_reference_upsert_envelope_for_initial_and_later_states() {
        let bridge = DownloadTaskBridge::default();
        bridge
            .publish(TaskSnapshot {
                id: "task-1".into(),
                status: "waiting".into(),
                name: "直播".into(),
                error: None,
            })
            .await;
        let address = bridge.clone().bind().await.unwrap();
        let (socket, _) = tokio_tungstenite::connect_async(format!(
            "ws://{address}/ws/v1/download_task?task_id=task-1"
        ))
        .await
        .unwrap();
        let (_, mut read) = socket.split();

        let initial = read.next().await.unwrap().unwrap().into_text().unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&initial).unwrap(),
            serde_json::json!({"type":"task_upsert","tasks":[{"id":"task-1","status":"waiting","name":"直播","error":null}]})
        );

        bridge.start("task-1").await.unwrap();
        let updated = read.next().await.unwrap().unwrap().into_text().unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&updated).unwrap()["tasks"][0]["status"],
            "downloading"
        );
    }
}
