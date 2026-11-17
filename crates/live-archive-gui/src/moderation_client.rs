//! Runs the controller in this process and communicates with its loopback API.
use serde_json::{Value, json};
use std::{path::PathBuf, sync::mpsc, thread};
use tokio::{
    sync::{mpsc as async_mpsc, oneshot},
    time::{Duration, interval},
};

pub enum Command {
    Action { path: String, body: Value },
    WatchRooms(Vec<String>),
    Stop,
}

pub enum Event {
    Ready(String),
    State(Value),
    Timeline(String, Value),
    Error(String),
    ActionError {
        path: String,
        body: Value,
        error: String,
    },
    ActionFinished {
        path: String,
        body: Value,
    },
    Stopped,
}

pub struct Client {
    commands: async_mpsc::UnboundedSender<Command>,
    pub events: mpsc::Receiver<Event>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Client {
    pub fn start(config: PathBuf) -> Self {
        let (commands, rx) = async_mpsc::unbounded_channel();
        let (events, receiver) = mpsc::channel();
        let thread = thread::spawn(move || {
            let result = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build();
            match result {
                Ok(rt) => rt.block_on(run(config, rx, &events)),
                Err(error) => {
                    let _ = events.send(Event::Error(error.to_string()));
                }
            }
            let _ = events.send(Event::Stopped);
        });
        Self {
            commands,
            events: receiver,
            thread: Some(thread),
        }
    }
    pub fn action(&self, path: &str, body: Value) {
        let _ = self.commands.send(Command::Action {
            path: path.into(),
            body,
        });
    }
    pub fn watch_rooms(&self, rooms: Vec<String>) {
        let _ = self.commands.send(Command::WatchRooms(rooms));
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

async fn run(
    config: PathBuf,
    mut commands: async_mpsc::UnboundedReceiver<Command>,
    events: &mpsc::Sender<Event>,
) {
    let (ready_tx, ready_rx) = oneshot::channel();
    let (stop_tx, stop_rx) = oneshot::channel();
    let mut server = tokio::spawn(async move {
        live_archive_core::moderation::control::serve_in_process(
            &config,
            |url| {
                let _ = ready_tx.send(url);
            },
            async {
                let _ = stop_rx.await;
            },
        )
        .await
    });
    let url = tokio::select! {
        ready = ready_rx => match ready { Ok(url) => url, Err(_) => { report_server(server.await, events); return; } },
        result = &mut server => { report_server(result, events); return; },
        _ = commands.recv() => { let _ = stop_tx.send(()); report_server(server.await, events); return; }
    };
    let Some((base, token)) = url.split_once('#') else {
        let _ = events.send(Event::Error("中控地址无效".into()));
        let _ = stop_tx.send(());
        let _ = server.await;
        return;
    };
    let _ = events.send(Event::Ready(url.clone()));
    let http = match reqwest::Client::builder().no_proxy().build() {
        Ok(client) => client,
        Err(error) => {
            let _ = events.send(Event::Error(error.to_string()));
            let _ = stop_tx.send(());
            let _ = server.await;
            return;
        }
    };
    let mut timer = interval(Duration::from_secs(1));
    let mut watched = Vec::<String>::new();
    loop {
        tokio::select! {
            _ = timer.tick() => {
                match request(&http, base, token, "state", None).await {
                    Ok(state) => {
                        // The selected room may disappear while the UI is processing
                        // the new state. Do not poll its obsolete timeline.
                        let active_watched = watched_rooms_in_state(&watched, &state);
                        let _ = events.send(Event::State(state));
                        for id in active_watched {
                            match request(&http, base, token, "timeline", Some(json!({"id": id}))).await {
                                Ok(timeline) => { let _ = events.send(Event::Timeline(id.to_owned(), timeline)); }
                                // A room can also close between state and timeline requests.
                                Err(error) if error == "unknown room" => {}
                                Err(error) => { let _ = events.send(Event::Error(error)); }
                            }
                        }
                    }
                    Err(error) => { let _ = events.send(Event::Error(error)); }
                }
            }
            command = commands.recv() => match command {
                Some(Command::Action {path, body}) => match request(&http, base, token, &path, Some(body.clone())).await { Ok(_) => { let _ = events.send(Event::ActionFinished { path, body }); }, Err(error) => { let error=if error=="unknown room" { "直播间已关闭或断开，请选择当前直播间".to_owned() } else { error }; let _ = events.send(Event::ActionError { path, body, error }); } },
                Some(Command::WatchRooms(rooms)) => watched = rooms,
                Some(Command::Stop) | None => break,
            },
            result = &mut server => { report_server(result, events); return; }
        }
    }
    let _ = stop_tx.send(());
    report_server(server.await, events);
}

fn watched_rooms_in_state<'a>(watched: &'a [String], state: &Value) -> Vec<&'a str> {
    let rooms = state["rooms"].as_array();
    watched
        .iter()
        .filter(|id| {
            rooms.is_some_and(|rooms| {
                rooms
                    .iter()
                    .any(|room| room["id"].as_str() == Some(id.as_str()))
            })
        })
        .map(String::as_str)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polling_skips_closed_rooms_and_keeps_the_other_selection() {
        let watched = vec!["room-a".to_owned(), "room-b".to_owned()];
        assert_eq!(
            watched_rooms_in_state(&watched, &json!({"rooms":[{"id":"room-a"}]})),
            vec!["room-a"]
        );
        assert!(watched_rooms_in_state(&watched, &json!({"rooms":[]})).is_empty());
    }
}

async fn request(
    http: &reqwest::Client,
    base: &str,
    token: &str,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let url = format!("{base}api/{path}");
    let request = if let Some(body) = body {
        http.post(url).json(&body)
    } else {
        http.get(url)
    };
    let response = request
        .bearer_auth(token)
        .timeout(Duration::from_secs(4))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let value: Value = response.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(value["error"].as_str().unwrap_or("操作失败").into());
    }
    Ok(value)
}

fn report_server(
    result: Result<anyhow::Result<()>, tokio::task::JoinError>,
    events: &mpsc::Sender<Event>,
) {
    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            let _ = events.send(Event::Error(e.to_string()));
        }
        Err(e) => {
            let _ = events.send(Event::Error(e.to_string()));
        }
    }
}
