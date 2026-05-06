# Stage 12 — Send a single message (logic + UI; live network out of scope)

Status: **logic complete, end-to-end send test deferred.** Worker
gains a `manager_task` that multiplexes the receive stream with
inbound send commands via `futures::select`, dropping and re-opening
the stream around each `Manager::send_message` call (presage forces
this dance — both `receive_messages` and `send_message` borrow
`&mut self`). UI gains a `ComposeScreen` with whole-line input,
reachable from `ConversationList` via `'c'`. State-machine tests
cover every transition.

What this stage **does not** do: drive `Manager::send_message`
against a real Signal-server-issued recipient session (live
phone-in-the-loop or mocked Signal server). Same deferral as Stages
10 and 11 — the integration gate is Stage 9b's xtask + Renode harness.

## What landed

### Bridge

- **`Cmd::SendMessage { recipient, body }`** — `recipient` is the
  service-id string (e.g. `"00000000-0000-4000-8000-000000000abc"`
  for an Aci uuid); `body` is the plaintext UTF-8 message. The
  worker forwards to the manager task's inner channel.
- **`Event::SendComplete { timestamp }`** — `manager.send_message`
  returned Ok. `timestamp` is the wall-clock UNIX-millis we tagged
  the message with.
- **`Event::SendError(String)`** — send failed (parsed bad UUID,
  network error, recipient session expired, etc.). Stringly-typed
  for IPC-boundary cleanliness.
- **`InnerSend` struct** — private payload type, decomposes
  `Cmd::SendMessage` for the inner channel between worker
  dispatcher and `manager_task`.
- **Worker `send_to_manager: Option<Sender<InnerSend>>`** — Some
  while the manager task is alive; None before `StartReceive` and
  after the task dies. `Cmd::SendMessage` arriving while None
  emits `SendError("not receiving; send Cmd::StartReceive first")`.
- **`manager_task`** (~95 LoC): owns Manager for life. Outer loop
  opens stream and `futures::select`s between `stream.next()` and
  the inner send-channel. On send: drops the stream (releases the
  `&mut manager` borrow), calls `handle_send`, loops back to
  re-open the stream. The brief reconnect cost is the price of
  presage's API design.
- **`process_received` helper** (~45 LoC): factored out of the old
  `handle_receive` so the same per-item logic runs after every
  stream re-open. `Decision 5`'s flush-on-`QueueEmpty` is
  preserved.
- **`handle_send` helper** (~45 LoC): parses recipient UUID into
  `ServiceId::Aci`, builds a text-only `DataMessage` at the
  current wall-clock timestamp, calls `manager.send_message`,
  and emits exactly one event (`SendComplete` or `SendError`).

### UI

- New `crates/xous-app-signal-ui/src/screens/compose.rs` (~140
  LoC): `ComposeScreen { recipient, state, body }` with
  `SendState::{Editing, Sending, Sent{ts}, Error(reason)}`.
  `submit(body)` consumes the line and transitions Editing →
  Sending; `on_send_complete` and `on_send_error` advance from
  Sending. Idempotent — second `submit` while Sending is a no-op.
- **`Screen::Compose(ComposeScreen)`** populated in the enum,
  replacing the Stage 9c placeholder.
- **`ConversationList::handle_key`** — `'c'` pushes Compose
  prefilled with the most-recent received message's sender as
  recipient. With no messages yet, `'c'` is a no-op (no recipient
  to populate); future stage may surface a contact-picker.
- **`Ui::dispatch_line(line)`** — new test/runtime entry point.
  When top is Compose, hands the line to `submit`, emits
  `Cmd::SendMessage` if the screen accepted it. Other screens
  ignore the line.
- **`Ui::run`** routes stdin lines: when Compose is on top, the
  whole line goes to `dispatch_line`; otherwise the first char
  becomes a `Key` via `parse_key`. The string `"esc"` or
  `"/cancel"` typed in Compose triggers `Key::Esc` (back-out)
  rather than treating those as a body.
- **`Ui::handle_event`** routes `SendComplete`/`SendError` to the
  Compose screen on top, ignored if user navigated away.

## Verification

```sh
$ cargo test -p xous-app-signal-ui
test result: ok. 31 passed; 0 failed; 0 ignored
# 24 from Stages 9c-11 + 7 new Stage 12:
#   conversation_list_c_with_no_messages_is_noop
#   conversation_list_c_with_messages_pushes_compose
#   dispatch_line_emits_send_message_cmd
#   empty_dispatch_line_does_not_send
#   send_complete_event_advances_compose
#   send_error_event_advances_compose_to_error
#   compose_esc_pops_to_conversation_list

$ cargo test -p xous-signal-bridge          ✓ 3 passed
$ cargo test -p presage-store-pddb          ✓ 22 passed

$ cargo check --target=riscv32imac-unknown-xous-elf -p xous-app-signal
✓ Full rv32 cross-compile of the entire stack still passes.

$ cargo clippy --workspace --all-targets -- -D warnings   ✓ clean
$ cargo fmt --all -- --check                              ✓ clean
```

## Test coverage delta (7 new tests)

| Test | What it asserts |
|---|---|
| `conversation_list_c_with_no_messages_is_noop` | Pressing `c` with empty `messages` is a no-op (no Compose pushed; no recipient available) |
| `conversation_list_c_with_messages_pushes_compose` | Pressing `c` with a known sender pushes `Compose` prefilled with that recipient |
| `dispatch_line_emits_send_message_cmd` | Line input on Compose emits `Cmd::SendMessage { recipient, body }` and advances state to Sending |
| `empty_dispatch_line_does_not_send` | Empty line input doesn't send and leaves state in Editing |
| `send_complete_event_advances_compose` | `Event::SendComplete{ts}` flips state Sending → Sent{ts} |
| `send_error_event_advances_compose_to_error` | `Event::SendError(reason)` flips state Sending → Error(reason) |
| `compose_esc_pops_to_conversation_list` | Esc returns to ConversationList |

## Files changed

```
modified:
  Cargo.lock
  crates/xous-signal-bridge/src/cmd.rs                      (+SendMessage,
                                                             +SendComplete/Error)
  crates/xous-signal-bridge/src/lib.rs                      (handle_receive →
                                                             manager_task,
                                                             +process_received,
                                                             +handle_send,
                                                             +InnerSend, +select
                                                             multiplexer,
                                                             +send_to_manager
                                                             dispatcher state)
  crates/xous-app-signal-ui/src/lib.rs                      (+dispatch_line,
                                                             +SendComplete/Error
                                                             handlers,
                                                             +stdin line-vs-key
                                                             routing,
                                                             +top_id mapping,
                                                             +7 tests)
  crates/xous-app-signal-ui/src/screen.rs                   (Compose variant
                                                             populated, hint
                                                             updated)
  crates/xous-app-signal-ui/src/screens/mod.rs              (+pub mod compose;)
  crates/xous-app-signal-ui/src/screens/conversation_list.rs ('c' pushes Compose)

new:
  crates/xous-app-signal-ui/src/screens/compose.rs          (~140 LoC, 1 screen
                                                             struct)
  stage/REPORT-12.md                                        (this file)
```

## Audit-friendly invariants preserved (and one new)

- Side-effects still emit from one function (`Ui::on_screen_entered`)
  for screen-entry triggers; whole-line side-effects emit from
  `Ui::dispatch_line` only when top is Compose. Audit story: two
  match arms, one place each.
- The send/receive multiplexer is one `select!` block in
  `manager_task`. The "drop stream → send → re-open" pattern is
  three lines of explicit code; no clever borrow tricks.
- Event handling for stale Compose events (user navigated away)
  drops them on the floor — same guard pattern as Stage 10/11
  link/receive events. Symmetric across all event-routed screens.
- Whole-line input is only consumed by Compose. No screen can
  accidentally interpret a line as something else; the routing is
  in one place (`Ui::run`'s stdin handler).

## Hosted-mode flow now end-to-end

```
Splash → Home (Link this device)        Cmd::LinkDevice
   ↓                                     Event::LinkUrl
LinkStarting → LinkShowUrl → ...
   ↓                                     Event::LinkComplete
LinkDone → Home                         Cmd::StartReceive
   ↓                                     Event::ReceiveStarted
ConversationList (Listening)             Event::Message ×N
   ↓ press 'c' (with ≥1 msg received)
Compose (Editing)
   ↓ type "hello" + Enter               Cmd::SendMessage{recipient,body}
                                         (manager_task: drop stream, send, re-open)
Compose (Sending)
                                         Event::SendComplete{ts}
Compose (Sent)
   ↓ Esc
ConversationList (still Listening)
```

## What's deferred

- **End-to-end send test against `chat.signal.org`** (or a mocked
  Signal server). Same deferral as Stages 10 and 11; gate is
  Stage 9b's xtask + Renode harness. The state-machine tests
  cover the UI/IPC plumbing; the integration test covers
  protocol conformance.
- **Outgoing message echo into ConversationList.** Right now a
  successful send shows on the Compose screen as `Sent{ts}` but
  doesn't append to the ConversationList. Trivial follow-up:
  `Event::SendComplete` could also push a synthetic
  `MessageSummary { sender: "(me)", body: <captured>, ts }` into
  the underlying ConversationList. Defer to Stage 12+1.
- **Per-character input on Compose.** Hosted mode uses whole-line
  via stdin (one Enter = one send). On-device GAM mode (Stage 9c
  follow-up) will need per-`Key::Char(c)` input handling with
  backspace, cursor movement, etc. The `ComposeScreen.body` field
  is already a `String` ready to grow per-char; `submit` accepts a
  pre-built body string from either entry path.
- **Manual recipient entry.** Currently the recipient must be a
  prior received-message sender. A future stage adds a
  contact-picker (or a "Type recipient UUID" line input mode).
  The `ComposeScreen::new(recipient)` constructor doesn't care
  where the recipient comes from.
- **Reconnect cost on every send.** Each `Cmd::SendMessage`
  drops the receive stream and re-opens it after the send. Signal's
  WS reconnect is fast (sub-second typically) and pending messages
  are re-delivered in the next batch, so no data loss — but
  high-volume bidirectional chat will spend observable time on
  stream reconnects. A future refactor that holds Manager via a
  shared `Rc<RefCell<...>>` and yields the borrow between stream
  items would avoid this; not worth the complexity for MVP.
- **Group send.** `Cmd::SendMessage` is 1:1 only. Group v2 sends
  use `Manager::send_message_to_group` and need the master-key
  bytes; out of MVP scope per UI.md §13.

## MVP status

With Stage 12 complete, the three MVP flows from `docs/ROADMAP.md`'s
intro all have logic-level coverage:

1. **Link as secondary device** ✓ (Stage 10)
2. **Receive a single message** ✓ (Stage 11)
3. **Send a single message** ✓ (Stage 12)

Hardware-confirmed end-to-end testing for all three is gated on
Stage 9b's xtask + Renode harness. After that lands, this project
is at MVP.
