//! Answering an agent from the inbox.
//!
//! The client can't know what "1" means to a program, so it sends keys and text, never meanings.
//! What it can know is whether the question is still the one the person was shown: the server
//! stamps every status report, and a reply is sent only if the stamp hasn't moved. That leaves
//! one network trip in which a program could change its mind; nothing here can close that.
use crate::{Key, KeyAction, WaitingFor, input::Sender, plain};
use serde_json::{Value, json};
use std::time::Duration;

const LINES: usize = 14;
const COLUMNS: usize = 160;
const TAIL: u64 = 12_000;

/// The exact report a reply is for.
#[derive(Clone, Debug, PartialEq)]
pub struct Ticket {
    session: String,
    block: String,
    record: String,
    stamp: String,
    kind: Option<WaitingFor>,
    message: Option<String>,
}

/// What a program is waiting for, as the server has it now. All text is stripped and bounded.
#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    pub waiting_for: Option<WaitingFor>,
    pub app: String,
    pub message: Option<String>,
    /// The end of the screen, which is what gives the keys their meaning.
    pub screen: Vec<String>,
    pub ticket: Ticket,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnswerKey {
    One,
    Two,
    Three,
    Y,
    N,
    Enter,
    Escape,
}

impl AnswerKey {
    pub const ALL: [Self; 7] =
        [Self::One, Self::Two, Self::Three, Self::Y, Self::N, Self::Enter, Self::Escape];

    /// What is sent, not what it might mean.
    pub fn label(self) -> &'static str {
        match self {
            Self::One => "1",
            Self::Two => "2",
            Self::Three => "3",
            Self::Y => "y",
            Self::N => "n",
            Self::Enter => "Enter",
            Self::Escape => "Esc",
        }
    }
    fn key(self, action: KeyAction) -> Key {
        let (name, text) = match self {
            Self::One => ("digit_1", "1"),
            Self::Two => ("digit_2", "2"),
            Self::Three => ("digit_3", "3"),
            Self::Y => ("key_y", "y"),
            Self::N => ("key_n", "n"),
            Self::Enter => ("enter", "\n"),
            Self::Escape => ("escape", ""),
        };
        Key {
            name: name.into(),
            mods: String::new(),
            consumed_mods: "",
            text: if action == KeyAction::Release { String::new() } else { text.into() },
            unshifted: text.chars().next().filter(|c| c.is_alphanumeric()).map_or(0, u32::from),
            action,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    Key(AnswerKey),
    /// Typed as one line, then Enter.
    Line(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Sent, and the program's report changed.
    Reacted,
    /// Sent, but nothing changed within a few seconds. Never sent again by itself.
    Unconfirmed,
    /// Not sent: the question is no longer the one shown.
    Changed,
    /// Not sent.
    NotSent(String),
    /// The connection failed after it was sent: it may or may not have arrived.
    Unknown,
}

/// Reads and answers questions on one box, whether or not their session is open here.
#[derive(Clone)]
pub struct Asker(pub(crate) Sender);

impl Asker {
    async fn call(&self, method: &'static str, payload: Value) -> Result<Value, String> {
        self.0.call(method, payload).await
    }

    async fn records(&self, session: &str, block: &str) -> Result<Vec<Value>, String> {
        let payload = json!({"session_id": session, "block_id": block, "args": {}});
        let reply = self.call("com.superlogical.terminal.program_status", payload).await?;
        Ok(reply["records"].as_array().cloned().unwrap_or_default())
    }

    pub async fn question(self, session: String, block: String) -> Result<Question, String> {
        let records = self.records(&session, &block).await?;
        let Some(r) = records
            .iter()
            .filter(|r| r["state"] == "blocked")
            .max_by(|a, b| a["updated_at"].as_str().cmp(&b["updated_at"].as_str()))
        else {
            return Err("It is not waiting any more".into());
        };
        let ticket = ticket(&session, &block, r)?;
        let screen = self.screen(&session, &block).await?;
        let text = |v: &Value, max| v.as_str().map(|t| plain(t, max));
        Ok(Question {
            waiting_for: ticket.kind,
            app: text(&r["effective_app"], 32)
                .or(text(&r["app"], 32))
                .unwrap_or_else(|| "program".into()),
            message: ticket.message.as_deref().map(|m| plain(m, 300)),
            screen,
            ticket,
        })
    }

    /// The last lines of the screen. The server sends output from its beginning, scrollback
    /// included, so ask how long it is and read only the end.
    async fn screen(&self, session: &str, block: &str) -> Result<Vec<String>, String> {
        let body = |args: Value| json!({"session_id": session, "block_id": block, "args": args});
        let first = self
            .call(
                "com.superlogical.terminal.format",
                body(json!({"format": "text", "trim": true, "max_bytes": TAIL})),
            )
            .await?;
        let mut content = first["content"].as_str().unwrap_or_default().to_owned();
        if let (Some(id), Some(total)) = (first["id"].as_u64(), first["total"].as_u64()) {
            let start = total.saturating_sub(TAIL);
            // An offset inside a character is refused; a character is at most four bytes.
            let mut tail = Err(String::new());
            for nudge in 0..4 {
                let args = json!({"id": id, "offset": start + nudge, "max_bytes": TAIL + 64});
                tail = self.call("com.superlogical.terminal.blob_read", body(args)).await;
                if tail.is_ok() {
                    break;
                }
            }
            content = tail?["data"].as_str().unwrap_or_default().to_owned();
        }
        Ok(excerpt(&content))
    }

    pub async fn answer(self, ticket: Ticket, reply: Reply) -> Outcome {
        if ticket.kind == Some(WaitingFor::Auth) {
            return Outcome::NotSent("Sign-in is answered in the terminal".into());
        }
        let events = match events(&reply) {
            Ok(events) => events,
            Err(e) => return Outcome::NotSent(e),
        };
        match self.current(&ticket).await {
            Ok(true) => {}
            Ok(false) => return Outcome::Changed,
            Err(e) => return Outcome::NotSent(e),
        }
        // Straight to the block, past key bindings, in order on this connection.
        let mut last = Ok(Value::Null);
        for event in events {
            let payload =
                json!({"session_id": ticket.session, "block_id": ticket.block, "event": event});
            last = self.call("block.event", payload).await;
            if last.is_err() {
                break;
            }
        }
        if last.is_err() {
            return Outcome::Unknown;
        }
        for _ in 0..10 {
            match self.current(&ticket).await {
                Ok(true) => pause(Duration::from_millis(400)).await,
                Ok(false) => return Outcome::Reacted,
                Err(_) => return Outcome::Unconfirmed,
            }
        }
        Outcome::Unconfirmed
    }

    /// Whether the very report on the ticket still stands.
    async fn current(&self, t: &Ticket) -> Result<bool, String> {
        let records = self.records(&t.session, &t.block).await?;
        Ok(records
            .iter()
            .filter(|r| r["state"] == "blocked")
            .any(|r| ticket(&t.session, &t.block, r).as_ref() == Ok(t)))
    }
}

/// A timer for any executor: these futures run on the frontend's.
pub async fn pause(time: Duration) {
    let (done, wait) = flume::bounded::<()>(1);
    std::thread::spawn(move || {
        std::thread::sleep(time);
        let _ = done.send(());
    });
    let _ = wait.recv_async().await;
}

impl Ticket {
    pub(crate) fn sample() -> Self {
        Self {
            session: "demo".into(),
            block: "demo".into(),
            record: String::new(),
            stamp: String::new(),
            kind: None,
            message: None,
        }
    }
}

fn ticket(session: &str, block: &str, r: &Value) -> Result<Ticket, String> {
    Ok(Ticket {
        session: session.to_owned(),
        block: block.to_owned(),
        record: r["id"].as_str().unwrap_or_default().to_owned(),
        stamp: r["updated_at"].as_str().ok_or("This server does not date its reports")?.to_owned(),
        kind: match r["kind"].as_str() {
            Some("permission") => Some(WaitingFor::Permission),
            Some("question") => Some(WaitingFor::Question),
            Some("auth") => Some(WaitingFor::Auth),
            _ => None,
        },
        message: r["msg"].as_str().map(str::to_owned),
    })
}

fn events(reply: &Reply) -> Result<Vec<Value>, String> {
    let key = |k: AnswerKey, a| json!({"type": "key", "payload": k.key(a).payload()});
    let press = |k| [key(k, KeyAction::Press), key(k, KeyAction::Release)];
    match reply {
        Reply::Key(k) => Ok(press(*k).to_vec()),
        Reply::Line(text) => {
            let text = text.trim();
            if text.is_empty() {
                return Err("Nothing to send".into());
            }
            if text.chars().count() > 2000 || text.chars().any(char::is_control) {
                return Err("Only one line of plain text can be sent from here".into());
            }
            let mut events = vec![json!({"type": "paste", "payload": {"text": text}})];
            events.extend(press(AnswerKey::Enter));
            Ok(events)
        }
    }
}

fn excerpt(content: &str) -> Vec<String> {
    let lines: Vec<_> = content.trim_end().lines().collect();
    lines[lines.len().saturating_sub(LINES)..].iter().map(|l| plain(l, COLUMNS)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(stamp: &str, msg: &str) -> Value {
        json!({"state": "blocked", "kind": "permission", "msg": msg, "updated_at": stamp})
    }

    #[test]
    fn a_ticket_matches_only_the_very_same_report() {
        let t = ticket("s", "b", &record("2026-11-25T10:00:00Z", "Run x?")).unwrap();
        assert_eq!(ticket("s", "b", &record("2026-11-25T10:00:00Z", "Run x?")).as_ref(), Ok(&t));
        // Asked again, word for word: a different question.
        assert_ne!(ticket("s", "b", &record("2026-11-25T10:00:01Z", "Run x?")).as_ref(), Ok(&t));
        assert_ne!(ticket("s", "b", &record("2026-11-25T10:00:00Z", "Run y?")).as_ref(), Ok(&t));
        assert_ne!(ticket("s", "c", &record("2026-11-25T10:00:00Z", "Run x?")).as_ref(), Ok(&t));
        assert!(ticket("s", "b", &json!({"state": "blocked"})).is_err(), "undated: can't be guarded");
    }

    #[test]
    fn replies_are_single_lines_or_single_keys() {
        assert_eq!(events(&Reply::Key(AnswerKey::One)).unwrap().len(), 2);
        let line = events(&Reply::Line(" yes please ".into())).unwrap();
        assert_eq!(line[0]["payload"]["text"], "yes please");
        assert_eq!(line[1]["payload"]["key"], "enter");
        for bad in ["", "   ", "a\nb", "a\x1b[A", "a\rb"] {
            assert!(events(&Reply::Line(bad.into())).is_err(), "{bad:?}");
        }
        assert!(events(&Reply::Line("x".repeat(3000))).is_err());
    }

    #[test]
    fn the_excerpt_is_the_end_and_is_plain() {
        let content: String = (0..40).map(|n| format!("line {n}\x1b\u{202e}\n")).collect::<String>() + "\n\n";
        let lines = excerpt(&content);
        assert_eq!((lines.len(), lines[13].as_str()), (LINES, "line 39"));
        assert!(excerpt("").is_empty());
    }
}
