# Architecture

```
rex-rune (upstream, pinned git revision, UI feature off)   swap with scripts/update-backend.sh
    │   protocol, SSH/Tailscale, reconnect, Ghostty terminal state
crates/backend  (rex-backend)   the only crate that names rex-rune, rexrs or ghostty-vt
    │   Boxes, Host, Snapshot, Frame, Agent: plain, frontend-neutral API; contract tests
crates/cosmic   (cosmic-rex)   libcosmic frontend; its manifest has no core dependency
```

Rules:
- No rex-rune type appears in `rex-backend`'s public API. Core API changes are absorbed there.
- Contract tests in `rex-backend` build core models from server JSON, so a core upgrade that changes parsing, layout or status handling fails before it reaches the frontend.
- Terminal frames are the one hot path: `Frame` wraps the core's frame and reads cells in place, so the split costs no copy.
- Nothing upstream is vendored here. To patch the core, check out a fork and point `rex-rune`, `rexrs` and `ghostty-vt` in the root `Cargo.toml` at it.

Input: typing has its own control connection per box (`crates/backend/src/input.rs`), pipelined, observing only. The core sends one request at a time, which put fast typing 3-9 s behind on a 112 ms link; this is 122 ms per echo. All input uses it, so order is never in question. If an answer is over 3 s late the pane shows as paused and input is refused, not buffered.

Live check: `cargo run --release -p rex-backend --example live -- ENDPOINT` (one throwaway session, removed afterwards).

Safety: attaching never creates, stops or destroys anything. Status text is stripped and bounded in the backend and shown only as plain text. Remote-control requests from other clients are refused. Typing never takes a pane's size from another client (`Host::type_into`); taking it is an explicit, confirmed act. Input to a pane that is not live is dropped, never queued. Pasting is bound to the box and pane that was focused when it was requested. Selecting is this client's own, made on its copy of the screen (`Host::selection`): nothing reaches the server or the program. Mouse presses, drags and the wheel go to a program only when it asked for mouse reports and Shift is not held; only a press asks for an unclaimed pane's size, as typing does. Clear (`Host::clear`) empties a live pane's screen and scrollback for every device and takes no size.

Licences: this repo and rex-rune are MIT; libcosmic is MPL-2.0 (used as a dependency). cosmic-term is GPL-3.0: its code must not be copied here.

Local echo: `predict.rs` draws typed characters before the server echoes them (underlined until confirmed). Guesses are hidden until one has come true since the last Enter, control key or unexplained cursor move, so nothing typed at a prompt that does not echo is drawn. Off on links under 25 ms, when the cursor is hidden, or with `--no-local-echo`.

Answering: `answer.rs` reads a waiting program's report and the end of its screen, and sends keys or one line of text. Right before sending it re-reads the report and refuses unless the server's `updated_at`, id, kind and message are exactly those shown. Sign-in requests are refused. Nothing is ever sent twice. One network trip remains in which a program could change its mind.
