//! Live check against a real server: `cargo run --example live -- ENDPOINT`.
//!
//! Creates one throwaway session, uses only that, and destroys it at the end whatever happens.
//! A second client stands in for the Mac app. Prints nothing about any other session.
use rex_backend::{
    AgentState, AnswerKey, Host, Key, KeyAction, Outcome, Pointer, Reply, Select, Size, Snapshot, WaitingFor,
};
use rex_rune::connection::{HostCommand, HostConnection, HumanClose};
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

type Check = Result<(), String>;

fn until<T>(what: &str, seconds: u64, mut f: impl FnMut() -> Option<T>) -> Result<T, String> {
    let end = Instant::now() + Duration::from_secs(seconds);
    loop {
        if let Some(v) = f() {
            return Ok(v);
        }
        if Instant::now() > end {
            return Err(format!("timed out: {what}"));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn screen(s: &Snapshot) -> String {
    let Some(f) = s.panes.first().and_then(|p| p.frame.as_ref()) else { return String::new() };
    (0..f.row_count())
        .map(|y| {
            let row: String =
                f.row(y).map(|c| if c.text().is_empty() { " " } else { c.text() }).collect();
            row.trim_end().to_owned() + "\n"
        })
        .collect()
}
fn grid(s: &Snapshot) -> Option<(u16, u16)> {
    s.panes.first()?.frame.as_ref().map(|f| (f.cols(), f.rows()))
}

fn key(name: &str, text: &str, action: KeyAction) -> Key {
    Key {
        name: name.into(),
        mods: String::new(),
        consumed_mods: "",
        text: text.into(),
        unshifted: text.chars().next().filter(|c| c.is_alphanumeric()).map_or(0, |c| c as u32),
        action,
    }
}
fn run(host: &Host, block: &str, line: &str) {
    host.paste(block, line.into());
    host.key(block, &key("enter", "\n", KeyAction::Press));
    host.key(block, &key("enter", "", KeyAction::Release));
}

/// Type letters one at a time, waiting for each to be echoed.
fn echo_times(host: &Host, block: &str, letters: &str) -> Result<Vec<u128>, String> {
    let mut times = Vec::new();
    let mut typed = String::new();
    for c in letters.chars() {
        typed.push(c);
        let start = Instant::now();
        let name = format!("key_{c}");
        host.key(block, &key(&name, &c.to_string(), KeyAction::Press));
        host.key(block, &key(&name, "", KeyAction::Release));
        until("a typed letter to be echoed", 10, || {
            screen(&host.snapshot()).contains(&typed).then_some(())
        })?;
        times.push(start.elapsed().as_millis());
    }
    times.sort_unstable();
    Ok(times)
}

/// 24 letters sent at 10 keys a second, as a person types; how long after the last until all show.
fn burst(host: &Host, block: &str, releases: bool) -> Result<u128, String> {
    let letters = "zqxjkvwzqxjkvwzqxjkvwzqx";
    for c in letters.chars() {
        let name = format!("key_{c}");
        host.key(block, &key(&name, &c.to_string(), KeyAction::Press));
        if releases {
            host.key(block, &key(&name, "", KeyAction::Release));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let last = Instant::now();
    until("a burst of typing", 60, || screen(&host.snapshot()).contains(letters).then_some(()))?;
    let lag = last.elapsed().as_millis();
    std::thread::sleep(Duration::from_secs(8));
    host.key(block, &Key { mods: "ctrl".into(), text: String::new(), ..key("key_u", "u", KeyAction::Press) });
    std::thread::sleep(Duration::from_secs(1));
    Ok(lag)
}

fn block_on<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(future)
}

/// A program that asks `A?`, and asks `B?` in its place if told to, then reads one line.
fn ask(host: &Host, block: &str, n: u32, change: bool) -> Check {
    let report = |msg: &str| format!(r"printf '\033]7501;state=blocked:kind=permission:app=live-test:msg={msg}\033\\'");
    let then = if change { format!("sleep 3; {}; ", report("Qj8=")) } else { String::new() };
    let done = r"printf '\033]7501;state=working:app=live-test\033\\'";
    run(host, block, &format!(
        "clear; sh -c {}",
        quote(&format!("{}; echo proceed-{n}; {then}read a; {done}; echo got-{n}-$a; sleep 2", report("QT8=")))
    ));
    until("the question", 20, || {
        let s = host.snapshot();
        (screen(&s).contains(&format!("proceed-{n}"))
            && s.agents.iter().any(|a| a.block == block && a.state == AgentState::Blocked && a.message.as_deref() == Some("A?")))
        .then_some(())
    })
}
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn answers(host: &Host, block: &str, session: &str) -> Check {
    let q = || block_on(host.asker().expect("a Rex server").question(session.to_owned(), block.to_owned()));
    ask(host, block, 1, false)?;
    let question = q()?;
    ensure(
        question.message.as_deref() == Some("A?") && question.screen.iter().any(|l| l.contains("proceed-1")),
        "a question is read with its screen, without attaching",
    )?;
    let t = Instant::now();
    let outcome = block_on(host.asker().expect("a Rex server").answer(question.ticket.clone(), Reply::Line("yes please".into())));
    println!("      answered in {} ms: {outcome:?}", t.elapsed().as_millis());
    ensure(outcome == Outcome::Reacted, "an answered program reacts")?;
    until("the answer on screen", 10, || screen(&host.snapshot()).contains("got-1-yes please").then_some(()))?;
    let again = block_on(host.asker().expect("a Rex server").answer(question.ticket, Reply::Key(AnswerKey::Enter)));
    ensure(again == Outcome::Changed, "the same question can't be answered twice")?;
    std::thread::sleep(Duration::from_secs(3));

    ask(host, block, 2, true)?;
    let question = q()?;
    until("the program to change its question", 15, || {
        host.snapshot().agents.iter().any(|a| a.block == block && a.message.as_deref() == Some("B?")).then_some(())
    })?;
    let stale = block_on(host.asker().expect("a Rex server").answer(question.ticket, Reply::Line("WRONG".into())));
    ensure(stale == Outcome::Changed, "an answer to a replaced question is refused")?;
    std::thread::sleep(Duration::from_secs(1));
    ensure(!screen(&host.snapshot()).contains("WRONG"), "and nothing of it was typed")?;
    let outcome = block_on(host.asker().expect("a Rex server").answer(q()?.ticket, Reply::Line("right".into())));
    ensure(outcome == Outcome::Reacted, "the new question can be answered")?;
    std::thread::sleep(Duration::from_secs(3));
    ensure(
        host.snapshot().agents.iter().all(|a| a.block != block || a.state != AgentState::Blocked),
        "nothing is left waiting",
    )
}

fn pass(what: &str) {
    println!("PASS  {what}");
}
fn ensure(ok: bool, what: &str) -> Check {
    if ok {
        pass(what);
        Ok(())
    } else {
        Err(what.to_owned())
    }
}

fn checks(mac: &HostConnection, host: &Host, block: &str, session: &str) -> Check {
    let first = until("the pane to go live", 30, || {
        let s = host.snapshot();
        (s.panes.first().is_some_and(|p| p.live) && grid(&s).is_some()).then_some(s)
    })?;
    pass("connected, snapshot decoded, pane live");
    ensure(first.panes[0].size == Size::Elsewhere, "the other client owns the pane's size")?;
    let size = grid(&first);
    let owner = || {
        let m = mac.model.lock().unwrap();
        m.terminals.get(block).is_some_and(|c| c.model.lock().unwrap().owner)
    };

    host.resize(block, 91, 27, (8, 18));
    run(host, block, "echo live-$((40+2))-ok");
    until("command output", 20, || screen(&host.snapshot()).contains("live-42-ok").then_some(()))?;
    pass("typed and pasted into a pane someone else owns; output came back");
    std::thread::sleep(Duration::from_secs(1));
    let s = host.snapshot();
    ensure(owner() && s.panes[0].size == Size::Elsewhere, "typing took no ownership")?;
    ensure(grid(&s) == size, "nothing was resized")?;

    let slow = echo_times(host, block, "zqjxkvw")?;
    println!("      echo, pane owned elsewhere: median {} ms, worst {} ms", slow[3], slow[6]);
    host.key(block, &Key { mods: "ctrl".into(), text: String::new(), ..key("key_u", "u", KeyAction::Press) });

    println!("      burst lag, owned elsewhere: {} ms ({} ms without releases)", burst(host, block, true)?, burst(host, block, false)?);

    host.fit_here(session, block);
    until("ownership after Fit", 15, || {
        (host.snapshot().panes.first()?.size == Size::Here).then_some(())
    })?;
    host.resize(block, 91, 27, (8, 18));
    until("the new size", 15, || (grid(&host.snapshot()) == Some((91, 27))).then_some(()))?;
    pass("Fit here takes ownership and resizes, only when asked");

    let fast = echo_times(host, block, "qzxjvkw")?;
    println!("      echo, pane owned here:      median {} ms, worst {} ms", fast[3], fast[6]);
    host.key(block, &Key { mods: "ctrl".into(), text: String::new(), ..key("key_u", "u", KeyAction::Press) });

    // Local echo: how soon a letter is drawn, and that it was right.
    let line = "echo the quick brown fox";
    let (mut drawn, mut slowest) = (0, Duration::ZERO);
    for c in line.chars() {
        let name = if c == ' ' { "space".to_owned() } else { format!("key_{c}") };
        let start = Instant::now();
        host.key(block, &key(&name, &c.to_string(), KeyAction::Press));
        if host.snapshot().panes[0].guess.ends_with(c) {
            drawn += 1;
            slowest = slowest.max(start.elapsed());
        }
        host.key(block, &key(&name, "", KeyAction::Release));
        for _ in 0..10 {
            std::thread::sleep(Duration::from_millis(10));
            host.snapshot();
        }
    }
    println!("      local echo: {drawn} of {} letters drawn at once, slowest {:.2} ms", line.len(), slowest.as_secs_f64() * 1000.0);
    ensure(drawn > line.len() / 2, "most typing is drawn before the server answers")?;
    until("the guesses to be confirmed", 10, || {
        let s = host.snapshot();
        (s.panes[0].guess.is_empty() && screen(&s).contains(line)).then_some(())
    })?;
    pass("and the server agreed with all of it");
    host.key(block, &Key { mods: "ctrl".into(), text: String::new(), ..key("key_u", "u", KeyAction::Press) });
    run(host, block, "printf 'Pass''word: '; read -s x; echo; echo got-${#x}");
    until("the password prompt", 15, || screen(&host.snapshot()).contains("Password:").then_some(()))?;
    std::thread::sleep(Duration::from_millis(500));
    let mut leaked = false;
    for c in "hunterabc".chars() {
        host.key(block, &key(&format!("key_{c}"), &c.to_string(), KeyAction::Press));
        for _ in 0..15 {
            leaked |= !host.snapshot().panes[0].guess.is_empty();
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    host.key(block, &key("enter", "\n", KeyAction::Press));
    until("the password to arrive", 15, || screen(&host.snapshot()).contains("got-9").then_some(()))?;
    ensure(!leaked, "nothing typed at a password prompt was ever drawn")?;

    let t = Instant::now();
    for _ in 0..1000 {
        std::hint::black_box(host.snapshot());
    }
    println!("      snapshot: {:.1} us each", t.elapsed().as_secs_f64() * 1000.0);
    println!("      burst lag, owned here:      {} ms ({} ms without releases)", burst(host, block, true)?, burst(host, block, false)?);

    selecting(host, block)?;
    answers(host, block, session)?;

    // "Run the live test?" as a blocked permission request, from a process of its own.
    let report = r"printf '\033]7501;state=blocked:kind=permission:app=live-test:msg=UnVuIHRoZSBsaXZlIHRlc3Q/\033\\'";
    run(host, block, &format!("sh -c {}", quote(&format!("{report}; sleep 30"))));
    let agent = until("the status report", 15, || {
        host.snapshot().agents.into_iter().find(|a| a.block == block && a.state == AgentState::Blocked)
    })?;
    ensure(agent.waiting_for == Some(WaitingFor::Permission), "blocked permission request reaches the inbox")?;
    ensure(agent.message.as_deref() == Some("Run the live test?"), "with its message intact")?;
    ensure(agent.session == session && agent.app == "live-test", "from the right session and program")?;
    host.key(block, &Key { mods: "ctrl".into(), text: String::new(), ..key("key_c", "c", KeyAction::Press) });
    until("the record to clear at the next prompt", 15, || {
        host.snapshot().agents.iter().all(|a| a.block != block || a.state != AgentState::Blocked).then_some(())
    })
    .map(|()| pass("interrupting clears it"))
    .or_else(|_| {
        println!("NOTE  the record stayed after Ctrl+C (needs shell prompt marks)");
        Ok::<(), String>(())
    })?;
    tabs(host, block, session)
}

/// A line chosen with a triple click and the whole screen with Select all, then the screen cleared.
fn selecting(host: &Host, block: &str) -> Check {
    run(host, block, "clear; echo pick-$((6*7))-this-line");
    let row = until("the line to pick", 15, || {
        screen(&host.snapshot()).lines().position(|l| l == "pick-42-this-line")
    })?;
    let at = |x: u16| Pointer { cell: (x, row as u16), position: (f32::from(x) * 8.0 + 2.0, row as f32 * 18.0 + 4.0), cell_width: 8.0, height: 400.0 };
    let text = |revision: u64| {
        until("the selection's text", 10, || {
            let s = host.snapshot();
            let f = s.panes.first()?.frame.clone()?;
            (f.selection_revision() >= revision).then(|| f.selected_text().unwrap_or_default().to_owned())
        })
    };
    host.selection(block, Select::Press(3), at(4)).ok_or("no selection")?;
    let up = host.selection(block, Select::Release, at(4)).ok_or("no release")?;
    ensure(text(up)? == "pick-42-this-line", "a triple click selects the line, and its text is there to copy")?;
    let press = host.selection(block, Select::Press(1), at(2)).ok_or("no press")?;
    ensure(text(press)?.is_empty(), "a single click clears it and selects nothing")?;
    host.selection(block, Select::Press(1), at(5)).ok_or("no press")?;
    host.selection(block, Select::Drag, at(9)).ok_or("no drag")?;
    let up = host.selection(block, Select::Release, at(9)).ok_or("no release")?;
    let got = text(up)?;
    ensure(got == "42-t", &format!("dragging selects up to the cell edge the pointer passed (got {got:?})"))?;
    let all = host.selection(block, Select::All, at(0)).ok_or("no select all")?;
    ensure(text(all)?.contains("pick-42-this-line"), "Select all takes the whole screen")?;
    host.clear(block);
    until("the screen to clear", 15, || (!screen(&host.snapshot()).contains("pick-42")).then_some(()))?;
    pass("Clear empties the screen and scrollback");
    Ok(())
}

/// A second tab in the throwaway session: renamed, then closed, leaving the first one as it was.
fn tabs(host: &Host, block: &str, session: &str) -> Check {
    host.split(session, block, rex_backend::Split::Right);
    let new = until("the split", 15, || {
        let s = host.snapshot();
        (s.tab_panes == 2).then(|| s.panes.into_iter().find(|p| p.block != block)).flatten()
    })?;
    host.zoom(session, &new.block);
    until("the zoom", 15, || host.snapshot().panes.iter().any(|p| p.block == new.block && p.zoomed).then_some(()))?;
    pass("a zoomed pane says so, with its tab's other pane still counted");
    host.close_pane(session, &new.block);
    until("the pane to close", 15, || {
        let s = host.snapshot();
        (s.tab_panes == 1 && s.panes.iter().all(|p| p.block == block)).then_some(())
    })?;
    pass("closing a pane closes only that pane");
    let first = host.snapshot().tabs;
    ensure(first.len() == 1, "the throwaway session starts with one tab")?;
    host.new_tab(session, block);
    let new = until("the new tab", 15, || {
        host.snapshot().tabs.into_iter().find(|t| first.iter().all(|f| f.id != t.id))
    })?;
    host.rename_tab(session, &new.id, "live-renamed");
    until("the new name", 15, || {
        host.snapshot().tabs.iter().any(|t| t.id == new.id && t.label == "live-renamed").then_some(())
    })?;
    pass("a tab renamed from here is renamed on the server");
    host.close_tab(session, &new.id);
    until("the tab to close", 15, || (host.snapshot().tabs == first).then_some(()))?;
    ensure(host.snapshot().sessions.iter().any(|s| s.id == session), "closing a tab closes only that tab")?;
    sessions(host, session)
}

/// The throwaway session renamed, then moved to each end of the list. Every other session keeps
/// its name and its order among the rest.
fn sessions(host: &Host, session: &str) -> Check {
    let others = |s: &Snapshot| -> Vec<(String, String)> {
        s.sessions.iter().filter(|x| x.id != session).map(|x| (x.id.clone(), x.label.clone())).collect()
    };
    let before = others(&host.snapshot());
    let renamed = format!("{session}-renamed");
    host.rename_session(session, &renamed);
    until("the session's new name", 15, || {
        host.snapshot().sessions.iter().any(|s| s.id == session && s.label == renamed).then_some(())
    })?;
    pass("a session renamed from here is renamed on the server");
    if before.is_empty() {
        println!("NOTE  no other session to move past");
        return Ok(());
    }
    let place = || host.snapshot().sessions.iter().position(|s| s.id == session);
    let count = before.len() + 1;
    host.move_session(session, count);
    until("the session to move to the end", 15, || (place() == Some(count - 1)).then_some(()))?;
    host.move_session(session, 0);
    until("the session to move to the top", 15, || (place() == Some(0)).then_some(()))?;
    ensure(others(&host.snapshot()) == before, "moving a session leaves the others as they were")?;
    pass("a session moved from here moves on the server");
    Ok(())
}

fn main() {
    let endpoint = std::env::args().nth(1).expect("usage: live ENDPOINT");
    rex_backend::init();
    let label = format!("cosmic-rex-live-test-{}", std::process::id());
    let start = Instant::now();
    let mac = HostConnection::spawn(endpoint.clone(), None, Some((label.clone(), None)), None, true, None);
    let ids = || -> BTreeSet<String> {
        mac.model.lock().unwrap().sessions.iter().map(|s| s.session_id.clone()).collect()
    };
    let ours = until("the throwaway session", 40, || {
        let m = mac.model.lock().unwrap();
        m.sessions.iter().find(|s| s.label == label).map(|s| s.session_id.clone())
    });
    let session = match ours {
        Ok(id) => id,
        Err(e) => {
            println!("FAIL  {e}: {}", mac.model.lock().unwrap().status);
            std::process::exit(1);
        }
    };
    println!("      connected and session created in {} ms, API {}", start.elapsed().as_millis(), mac.model.lock().unwrap().protocol.api_revision.map_or("not reported".into(), |r| r.to_string()));
    let others: BTreeSet<String> = ids().into_iter().filter(|i| *i != session).collect();
    let mut result = (|| {
        let _ = mac.commands.send(HostCommand::Select(session.clone()));
        let block = until("the first pane", 20, || {
            let m = mac.model.lock().unwrap();
            (m.selected.as_ref() == Some(&session)).then(|| m.focused().map(str::to_owned)).flatten()
        })?;
        let _ = mac.commands.send(HostCommand::Claim { session: session.clone(), blocks: vec![block.clone()] });
        let host = Host::connect(endpoint, Some(session.clone()));
        // `live ENDPOINT hold SECONDS`: keep the session for a look with the app instead.
        if let Some(seconds) = std::env::args().nth(3).and_then(|s| s.parse().ok()) {
            until("the pane to go live", 30, || {
                host.snapshot().panes.first().is_some_and(|p| p.live).then_some(())
            })?;
            run(&host, &block, r"clear; printf '\033]7501;state=blocked:kind=question:app=live-test:msg=UnVuIHRoZSBsaXZlIHRlc3Q/\033\\'; ls -la / | head -18; sleep 60");
            println!("HOLD {session}");
            std::thread::sleep(Duration::from_secs(seconds));
            return Ok(());
        }
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| checks(&mac, &host, &block, &session)))
            .unwrap_or_else(|_| Err("a check panicked".into()))
    })();
    // Exactly the session this run created, by ID.
    let _ = mac.commands.send(HostCommand::Close {
        kind: HumanClose::Session,
        payload: serde_json::json!({"session_id": session}),
    });
    let cleaned = until("cleanup", 20, || (!ids().contains(&session)).then_some(()))
        .and_then(|()| ensure(others.is_subset(&ids()), "throwaway session removed, every other session still there"));
    if result.is_ok() {
        result = cleaned;
    } else if let Err(e) = cleaned {
        println!("FAIL  {e}");
    }
    drop(mac);
    rex_backend::shutdown(Duration::from_secs(5));
    match result {
        Ok(()) => println!("ALL PASS"),
        Err(e) => {
            println!("FAIL  {e}");
            std::process::exit(1);
        }
    }
}
