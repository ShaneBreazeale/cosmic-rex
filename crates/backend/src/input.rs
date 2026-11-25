//! Typing, on its own control connection.
//!
//! The core sends one request at a time and waits for each answer, so on a 110 ms link a person
//! typing normally falls seconds behind; it also takes a pane's size before typing into it. The
//! server runs `client.key` and `block.event` in arrival order per connection, so this one sends
//! without waiting. It observes (`session.attach_all`) and never attaches, so it cannot own a
//! block or open its data: it can only type.
//!
//! All input uses this one path, so order never depends on which connection was faster. Nothing
//! is kept for later: input while the link is down or not answering is refused.
use futures_util::{SinkExt as _, StreamExt as _};
use rex_rune::{connection::HostModel, ssh, transport};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// An unanswered request this old means the link is not carrying input.
const STALL: Duration = Duration::from_secs(3);
/// And this old, that the connection is worth replacing.
const GIVE_UP: Duration = Duration::from_secs(10);
const TICK: Duration = Duration::from_millis(250);

static RUNNING: AtomicUsize = AtomicUsize::new(0);

pub(crate) struct Outgoing {
    pub method: &'static str,
    pub payload: Value,
}

#[derive(Default)]
pub(crate) struct State {
    up: AtomicBool,
    /// When each request still waiting for its answer was sent, oldest first. Answers to ordered
    /// methods come in order, so ids are not needed.
    waiting: Mutex<VecDeque<Instant>>,
    limit: AtomicUsize,
    /// Smoothed time to an answer, in microseconds; 0 until one arrives.
    round_trip: AtomicU64,
    notice: Mutex<Option<String>>,
}

impl State {
    fn age(&self) -> Option<Duration> {
        self.waiting.lock().expect("waiting").front().map(Instant::elapsed)
    }
    fn answered(&self) {
        if let Some(sent) = self.waiting.lock().expect("waiting").pop_front() {
            let new = sent.elapsed().as_micros() as u64;
            let old = self.round_trip.load(Ordering::Relaxed);
            let next = if old == 0 { new } else { (old * 7 + new) / 8 };
            self.round_trip.store(next.max(1), Ordering::Relaxed);
        }
    }
    pub fn round_trip(&self) -> Duration {
        Duration::from_micros(self.round_trip.load(Ordering::Relaxed))
    }
    pub fn live(&self) -> bool {
        self.up.load(Ordering::Acquire) && self.age().is_none_or(|a| a < STALL)
    }
}

pub(crate) struct Input {
    queue: mpsc::UnboundedSender<Outgoing>,
    pub state: Arc<State>,
    pub wake: flume::Receiver<()>,
}

impl Input {
    pub fn spawn(endpoint: String, core: Arc<Mutex<HostModel>>) -> Self {
        let (queue, outgoing) = mpsc::unbounded_channel();
        let (signal, wake) = flume::bounded(1);
        let state = Arc::new(State::default());
        let shared = state.clone();
        RUNNING.fetch_add(1, Ordering::AcqRel);
        let worker = move || {
            struct Done;
            impl Drop for Done {
                fn drop(&mut self) {
                    RUNNING.fetch_sub(1, Ordering::AcqRel);
                }
            }
            let _done = Done;
            match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime.block_on(run(endpoint, core, outgoing, shared, signal)),
                Err(e) => *shared.notice.lock().expect("notice") = Some(e.to_string()),
            }
        };
        if std::thread::Builder::new().name("rex-input".into()).spawn(worker).is_err() {
            RUNNING.fetch_sub(1, Ordering::AcqRel);
        }
        Self { queue, state, wake }
    }

    /// A channel whose far end the test holds.
    #[cfg(test)]
    pub fn intercepted(up: bool) -> (Self, mpsc::UnboundedReceiver<Outgoing>) {
        let (queue, sent) = mpsc::unbounded_channel();
        let state = Arc::new(State::default());
        state.up.store(up, Ordering::Release);
        (Self { queue, state, wake: flume::bounded(1).1 }, sent)
    }

    /// False when refused: nothing is kept to send later.
    pub fn send(&self, method: &'static str, payload: Value) -> bool {
        self.state.live() && self.queue.send(Outgoing { method, payload }).is_ok()
    }

    /// The newest problem, once.
    pub fn take_notice(&self) -> Option<String> {
        self.state.notice.lock().expect("notice").take()
    }
}

/// Wait for every input connection to say goodbye, so servers forget it at once.
pub(crate) fn finished(timeout: Duration) -> bool {
    let end = Instant::now() + timeout;
    while RUNNING.load(Ordering::Acquire) > 0 {
        if Instant::now() > end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

async fn run(
    endpoint: String,
    core: Arc<Mutex<HostModel>>,
    mut outgoing: mpsc::UnboundedReceiver<Outgoing>,
    state: Arc<State>,
    wake: flume::Sender<()>,
) {
    let connected = || core.lock().expect("host model").connected;
    let mut wait = TICK;
    loop {
        // The core connects first: it owns the SSH forward, and knows when a host is back.
        while !connected() {
            tokio::select! {
                next = outgoing.recv() => if next.is_none() { return },
                () = tokio::time::sleep(TICK) => wait = TICK,
            }
        }
        let opened = tokio::select! {
            opened = open(&endpoint) => opened,
            () = closed(&mut outgoing) => return,
        };
        let result = match opened {
            Ok((socket, limit)) => {
                wait = TICK;
                state.limit.store(limit, Ordering::Release);
                // Anything refused meanwhile was never queued; drop a straggler all the same.
                while outgoing.try_recv().is_ok() {}
                state.up.store(true, Ordering::Release);
                let _ = wake.try_send(());
                let result = carry(socket, &mut outgoing, &state, &wake, &connected).await;
                state.up.store(false, Ordering::Release);
                state.waiting.lock().expect("waiting").clear();
                let _ = wake.try_send(());
                result
            }
            Err(error) => Err(error),
        };
        match result {
            Ok(()) => return,
            Err(error) => {
                *state.notice.lock().expect("notice") = Some(format!("Typing is paused: {error}"));
                let _ = wake.try_send(());
            }
        }
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = closed(&mut outgoing) => return,
        }
        wait = (wait * 2).min(Duration::from_secs(8));
    }
}

/// Resolves when this client is done with the host. Input sent meanwhile is discarded.
async fn closed(outgoing: &mut mpsc::UnboundedReceiver<Outgoing>) {
    while outgoing.recv().await.is_some() {}
}

async fn open(endpoint: &str) -> Result<(transport::Socket, usize), String> {
    let text = |e: &dyn std::fmt::Display| crate::plain(&e.to_string(), 200);
    let endpoint = match ssh::Target::parse(endpoint).map_err(|e| text(&e))? {
        Some(target) => format!("unix://{}", target.local_socket().display()),
        None => endpoint.to_owned(),
    };
    let mut socket = transport::socket(&endpoint, "/api/control", "rex.control.v1")
        .await
        .map_err(|e| text(&e))?;
    let hello = json!({
        "type": "hello",
        "api_revision": transport::API_REVISION,
        "api_min_revision": transport::MIN_API_REVISION,
        "client": {
            // Not an app: apps are asked to show prompts, and this can't.
            "kind": "other",
            "platform": "linux",
            "name": "cosmic-rex input",
            "version": env!("CARGO_PKG_VERSION"),
        },
    });
    let attach = json!({
        "type": "request", "id": "attach", "method": "session.attach_all",
        "payload": {"event_scope": "session"},
    });
    let setup = async {
        socket.send(Message::text(hello.to_string())).await.map_err(|e| text(&e))?;
        let welcome = next_json(&mut socket).await?;
        if welcome["type"] != "welcome" || welcome["protocol"] != "rex.control.v1" {
            return Err("the server did not welcome this client".to_owned());
        }
        let limit = welcome["limits"]["max_message_bytes"].as_u64().unwrap_or(65536);
        socket.send(Message::text(attach.to_string())).await.map_err(|e| text(&e))?;
        loop {
            let answer = next_json(&mut socket).await?;
            if answer["type"] == "response" && answer["id"] == "attach" {
                if let Some(message) = answer["error"]["message"].as_str() {
                    return Err(crate::plain(message, 200));
                }
                return Ok(limit as usize);
            }
        }
    };
    let limit = tokio::time::timeout(Duration::from_secs(8), setup)
        .await
        .map_err(|_| "the server did not answer in time".to_owned())??;
    Ok((socket, limit))
}

async fn next_json(socket: &mut transport::Socket) -> Result<Value, String> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => {
                return serde_json::from_str(&text).map_err(|e| e.to_string());
            }
            Some(Ok(Message::Close(frame))) => {
                let why = frame.map(|f| f.reason.to_string()).unwrap_or_default();
                return Err(format!("the server closed the connection: {}", crate::plain(&why, 160)));
            }
            Some(Ok(_)) => {}
            Some(Err(e)) => return Err(crate::plain(&e.to_string(), 200)),
            None => return Err("the connection ended".into()),
        }
    }
}

/// Ok when this client is done; an error when the connection is.
async fn carry(
    mut socket: transport::Socket,
    outgoing: &mut mpsc::UnboundedReceiver<Outgoing>,
    state: &State,
    wake: &flume::Sender<()>,
    connected: &impl Fn() -> bool,
) -> Result<(), String> {
    let mut id = 0u64;
    let mut tick = tokio::time::interval(TICK);
    let mut was_live = true;
    loop {
        tokio::select! {
            next = outgoing.recv() => {
                let Some(next) = next else {
                    let _ = tokio::time::timeout(Duration::from_secs(1), socket.close(None)).await;
                    return Ok(());
                };
                id += 1;
                let message = json!({
                    "type": "request", "id": id.to_string(),
                    "method": next.method, "payload": next.payload,
                })
                .to_string();
                if message.len() > state.limit.load(Ordering::Acquire) {
                    *state.notice.lock().expect("notice") =
                        Some("Too large to send; nothing was typed".into());
                    let _ = wake.try_send(());
                    continue;
                }
                state.waiting.lock().expect("waiting").push_back(Instant::now());
                tokio::time::timeout(STALL, socket.send(Message::text(message)))
                    .await
                    .map_err(|_| "the server is not reading".to_owned())?
                    .map_err(|e| crate::plain(&e.to_string(), 200))?;
            }
            answer = next_json(&mut socket) => {
                let answer = answer?;
                if answer["type"] == "response" {
                    state.answered();
                    if let Some(message) = answer["error"]["message"].as_str() {
                        *state.notice.lock().expect("notice") =
                            Some(format!("Not typed: {}", crate::plain(message, 200)));
                        let _ = wake.try_send(());
                    }
                }
            }
            _ = tick.tick() => {
                if !connected() {
                    return Err("the box is reconnecting".into());
                }
                if state.age().is_some_and(|a| a > GIVE_UP) {
                    return Err("the server stopped answering".into());
                }
            }
        }
        let live = state.live();
        if live != was_live {
            was_live = live;
            let _ = wake.try_send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

    /// A server that answers the handshake, then records requests and answers only when told.
    fn serve(
        path: std::path::PathBuf,
        seen: flume::Sender<Value>,
        answer: flume::Receiver<()>,
    ) -> std::thread::JoinHandle<()> {
        let listener = std::os::unix::net::UnixListener::bind(path).unwrap();
        listener.set_nonblocking(true).unwrap();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            runtime.block_on(async move {
                let listener = tokio::net::UnixListener::from_std(listener).unwrap();
                let (stream, _) = listener.accept().await.unwrap();
                #[allow(clippy::result_large_err)]
                let echo = |_: &Request, mut r: Response| {
                    let protocol = "rex.control.v1".parse().unwrap();
                    r.headers_mut().insert("Sec-WebSocket-Protocol", protocol);
                    Ok(r)
                };
                let mut ws = tokio_tungstenite::accept_hdr_async(stream, echo).await.unwrap();
                let mut pending = VecDeque::new();
                loop {
                    tokio::select! {
                        m = ws.next() => {
                            let Some(Ok(m)) = m else { return };
                            let Message::Text(t) = m else { continue };
                            let v: Value = serde_json::from_str(&t).unwrap();
                            seen.send(v.clone()).unwrap();
                            let reply = if v["type"] == "hello" {
                                json!({"type": "welcome", "protocol": "rex.control.v1",
                                       "limits": {"max_message_bytes": 65536}, "api_revision": 18})
                            } else if v["method"] == "session.attach_all" {
                                json!({"type": "response", "id": v["id"], "seq": 0, "payload": {}})
                            } else {
                                pending.push_back(v["id"].clone());
                                continue;
                            };
                            ws.send(Message::text(reply.to_string())).await.unwrap();
                        }
                        Ok(()) = answer.recv_async() => {
                            while let Some(id) = pending.pop_front() {
                                let r = json!({"type": "response", "id": id, "seq": 0, "payload": {}});
                                ws.send(Message::text(r.to_string())).await.unwrap();
                            }
                        }
                    }
                }
            });
        })
    }

    fn eventually(what: &str, f: impl Fn() -> bool) {
        let end = Instant::now() + Duration::from_secs(8);
        while !f() {
            assert!(Instant::now() < end, "{what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn input_is_pipelined_in_order_only_types_and_stops_when_unanswered() {
        let dir = std::env::temp_dir().join(format!("cosmic-rex-input-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.sock");
        let (seen, requests) = flume::unbounded();
        let (answer, answers) = flume::unbounded();
        let server = serve(path.clone(), seen, answers);
        let core = Arc::new(Mutex::new(HostModel { connected: true, ..Default::default() }));
        let input = Input::spawn(format!("unix://{}", path.display()), core.clone());
        assert!(!input.send("client.key", json!({})), "refused before it is up");
        eventually("connects", || input.state.live());
        for n in 0..50 {
            assert!(input.send("client.key", json!({"n": n})));
        }
        // All 50 arrive although not one has been answered.
        let got: Vec<Value> = (0..52)
            .map(|_| requests.recv_timeout(Duration::from_secs(5)).expect("pipelined"))
            .collect();
        assert_eq!(got[0]["client"]["kind"], "other");
        assert_eq!(got[1]["method"], "session.attach_all");
        for (n, v) in got[2..].iter().enumerate() {
            assert_eq!((v["method"].as_str(), v["payload"]["n"].as_u64()), (Some("client.key"), Some(n as u64)));
        }
        answer.send(()).unwrap();
        eventually("answers are counted", || input.state.age().is_none());
        assert!(input.state.live());

        assert!(input.send("client.key", json!({"n": 50})));
        eventually("an unanswered key pauses typing", || !input.state.live());
        assert!(!input.send("client.key", json!({"n": 51})), "refused, not kept for later");
        answer.send(()).unwrap();
        eventually("an answer resumes it", || input.state.live());

        core.lock().unwrap().connected = false;
        eventually("goes down with the host", || !input.state.live());
        drop(input);
        let after: Vec<Value> = requests.try_iter().collect();
        assert!(after.iter().all(|v| v["payload"]["n"] == 50), "nothing refused was sent");
        server.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
