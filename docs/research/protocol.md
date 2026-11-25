# Rex client/server protocol (working notes, 2026-11-25)

Two unrelated things share the word "protocol":

- **OSC 7501 Program Status**: in-band sequence from a program to its terminal. Officially documented, vendor-neutral, rev 0.4. Does not connect a client to a server.
- **Rex control/data API**: what a client speaks. Documented by the server itself at `GET /llms.txt` and `GET /api/help`.

## Provenance

| Tag | Source |
| --- | --- |
| O | Official docs: /rex/docs (server-api, remote-access, reference/api, program-status) |
| H | `help.json` captured from 0.1.0-snapshot.2562, API 21 (rex-rune `vendor/rexrs/codegen/`, commit bd77ede) |
| W | windex `docs/protocol.md` (1414e80): API 13, live-verified by its author |
| R | rex-rune `docs/server-api21.md`: API 13→21 diff, live-verified by its author |
| X | rexctl `PROTOCOL.md` (2fc3602): API 7 |

We have **not** tested anything against a live server. Precedence: your server's `/llms.txt`, then O/H, R, W, X.

## 1. Transport and authentication (O, W)

HTTP routes on one listener: `/api/control` (WebSocket), `/api/data` (WebSocket), `/api/help`, `/llms.txt`, `/llms.html`.

| Endpoint | Identity |
| --- | --- |
| Unix socket (`$XDG_RUNTIME_DIR/rex/server.sock`, shared `/run/rex/server.sock`) | peer credentials |
| SSH forward of that socket (`ssh -L`), vendor-recommended today for personal servers (login server: "something like Tailscale") | SSH login's uid |
| Built-in Tailscale `https://rex.<tailnet>.ts.net` / Tailscale Serve / `tailnet://:7297` | Tailscale identity |
| Plain HTTP / VPN | none; needs `--allow-anonymous` |

There are **no client credentials or tokens**: admission is by connection, refusals are HTTP 403. **A connection can do anything the server account can**, including start programs. Per-client authorization is planned by the vendor. Systemd linger is needed on Linux servers or sessions die at logout.
Bare hostnames try HTTPS 443, HTTPS 7297, HTTP 7297. Some `server.*` methods (stop, config_reload, tailscale.*) are restricted to the local socket (X).

## 2. Control channel: `rex.control.v1` (W, X, R)

WebSocket; send `Sec-WebSocket-Protocol: rex.control.v1` and check the echo. One JSON object per text message.

1. Client within 5 s: `{"type":"hello","api_revision":21,"api_min_revision":10,"client":{"kind":"app","platform":"linux","name":"…","version":"…"}}`; optional `reconnect_token`. `kind` is `app`, `cli`, `agent` or `other` (W); use a non-`app` kind for a monitor that cannot answer prompts.
2. Server: `welcome` with negotiated `api_revision`, `limits.max_message_bytes` (1 MiB seen), `client.{id,reconnect_token,resumed}`.
3. `{"type":"request","id":"7","method":"session.view","payload":{…}}` → `{"type":"response","id":"7","seq":N,"payload"|"error":{code,message,status}}`.
4. `{"type":"event","seq":N,"payload":{"type":…,"payload":…}}`; session events are wrapped in `session_event`, block events inside that in `block_event`.

Rules:
- Ids are opaque strings. Ignore unknown fields, events and enum values.
- Responses can be out of order. A documented list (key/event/focus/layout methods) is serialized per connection; block methods are not. **`terminal.write` is unordered with `client.key`**: do not use it for interactive typing.
- Apply a response, then only events with `seq` greater than its `seq`. `session_view_changed` can arrive out of revision order: refetch when the revision is higher.
- Client death: abrupt TCP loss keeps the client 60 s (resume with token, same id). A clean close ends it. Events in the gap are lost: re-bootstrap. Never replay input.
- Server pings every 120 s. Event backlog > 8 MiB closes 1008. API 18+ replaces oversized events with `event_dropped`.
- API 17+: `client_prompt_requested` goes to `kind: app` clients. Reply `not_supported` at once if unsupported, or scripts stall.

## 3. Sessions and layout

- `session.bootstrap {event_scope, attach_session_ids?, data_tickets?}`: status, session list, attach, views and tickets in one call.
- `session.attach` joins one session (needed for `data.connect`, own/disown). `session.attach_all {event_scope: session|all}` observes every session **without** joining; `all` adds block events. The second is what an agent monitor needs.
- Session → windows (tabs) → layers (tiled, floating) → block tree. `rect`/`bounds` are fractions of the window. The server owns layout; clients request and wait for `session_view_changed`.
- Ownership: one client's size/theme applies per block and owning "gives no other rights" (O). `session.own_block` `take` wins at once, `request` gets only unowned blocks; both join a queue (takes ahead of requests, most recent first) used when the owner leaves. A dropped owner gets blocks back on reconnect unless ownership changed. `attach_all` alone cannot own (`bad_request`), so a pure monitor cannot resize other clients. Then `…terminal.resize`. Focused window is shared server state.
- API 21: `tiled_zoomed_block_id`. API 19: `focus_history.reverse`. API 20: `client.undo_close` (`undoable` closes).
- API 19-21 observed (R): `session.view` records focus history, `session.attach` does not; passive refresh of attached sessions should use attach.

## 4. Terminal data channel: `rex.data.terminal.v1` (W, X)

1. Get ticket from bootstrap or `data.connect {session_id, block_id, data_ref_id:"terminal", subprotocol}`: 16 bytes base64url, single use, 60 s.
2. Open `/api/data`, send binary `01`+ticket(16). Reply is WELCOME (32 bytes: `81`, token[16], 3 zero, retention_ms u32le, nonce u64le) or REJECT (`82`, reason).
3. Subsequent binary messages are one byte stream: **Ghostty snapshot** (`GHOSTSNP` 01 00, records of tag u16le/len u32le/CRC-32C, tag 6 = FINISH), then raw PTY bytes with Rex APCs `ESC _ rex1;<nonce>;<json> ESC \` (`resize`, `theme`, `child_exited`). Honour only the WELCOME nonce. Message boundaries carry no meaning.
4. Resume: `02`+token+offset u64le, offset counts all bytes after WELCOME, snapshot included. Refused: new ticket, new terminal. Close 4000 = block closed, stop. 1013 = reconnect fresh.
5. Client sends nothing after the handshake. Input always uses control.

Upload/download use `rex.data.bytes.v1` (API 11 / 13).

## 5. Input (W)

`client.key` (physical W3C key snake_case, mods, consumed_mods, text, unshifted_codepoint, action): Lua bindings run first; **server encodes**. `block.event` for paste, mouse, wheel, focus. Client never builds escape sequences or bracketed-paste wrappers. Mouse x/y are 0-based cells.

## 6. Agent-control subset

| Goal | Calls |
| --- | --- |
| Watch all agents | `session.attach_all {event_scope:"all"}`, then events `program_status_changed` / `program_status_removed`, `bell`, `desktop_notification`, `title_changed`, `process_changed`, `child_exited` |
| Initial state | `session.list`, `session.view`, `com.superlogical.terminal.program_status` per block |
| Read screen | `com.superlogical.terminal.format` (text/html/vt; includes scrollback; paginate with `blob_read`) |
| Reply | `block.event` paste + `client.key` Enter |
| Start | `session.create` / `session.new_window` / `new_split` with shell flavor and `cwd`/`command` options |
| Stop | Ctrl-C through `client.key`, or `block.close` |
| Jump another client | `client.perform` (that client must opt in) |

Program states: idle, working, done, blocked (`kind` permission/question/auth), error; plus `app`, `msg`, `title`, `progress`, `id`. Status text is untrusted: plain text only, show its source terminal, rate-limit notifications (O).

## 7. Open questions, to check on your server

- Snapshot version compatibility between the server's Ghostty and the pinned `ghostty-vt` crate. W reports crate 0.8.0 works at API 13; R claims tests at API 21.
- W saw `msg=`/`title=` OSC 7501 reports produce no event on an API 13 build, which contradicts O. Unresolved (base64 or old build?).
- W saw left-click press/release not delivered on one API 13 build.
- Native QUIC `carriers` in welcome: optional, ignored by all clients read here.
- Windows named-pipe endpoint: unverified.
- `session.view` focus side effect may change between builds.

Refresh: `rex api list --json` and `curl --unix-socket $XDG_RUNTIME_DIR/rex/server.sock http://localhost/llms.txt`.
