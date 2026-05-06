# Stage 11 — Receive a single message (logic + UI; live decrypt out of scope)

Status: **logic complete, end-to-end decrypt test deferred.** Bridge
gains `Cmd::StartReceive` + three new `Event` variants; worker grows
a long-running `handle_receive` task that owns the linked
`Manager<S, Registered>`, streams `Received` items from
`Manager::receive_messages`, translates `DataMessage`/`SyncMessage`
bodies to `Event::Message`, and calls `store.flush_sessions()` on
`Received::QueueEmpty` per docs/REPORT.md Decision 5. UI gains a
`ConversationListScreen` that replaces the Stage 9c empty-list
placeholder in the post-link flow; on entry it emits
`Cmd::StartReceive`, then accumulates received messages and
displays them latest-first.

What this stage **does not** do: drive `receive_messages` against a
real Signal-server-issued ciphertext (live phone-in-the-loop or
mocked Signal server). That requires Stage 9b's xtask + Renode
infrastructure, same as Stage 10's deferred end-to-end test.
Stage 11's automated coverage is the state-machine tests.

## What landed

### Bridge

- **`Cmd::StartReceive`** — UI emits this on entry to
  `Screen::ConversationList`. Requires that `linked: Some(_)` (i.e.
  `LinkDevice` already completed). If not, the worker emits
  `ReceiveError("not linked yet — send Cmd::LinkDevice first")` and
  the request is dropped.
- **`Event::ReceiveStarted`** — worker has obtained the stream and
  is parked on its `next()`. UI uses this to flip
  `ReceiveStatus::Starting → Listening`.
- **`Event::Message { sender, body, timestamp }`** — one decrypted
  message. Stringly-typed for IPC-boundary cleanliness (same shape
  as `LinkComplete`). Sender is a `service_id_string()`; body is
  the `DataMessage::body` plaintext (or the mirrored `SynchronizeMessage::sent.message.body` when the message was sent from another linked device).
- **`Event::ReceiveError(String)`** — receive loop hit a fatal
  error and unwound. The Manager is consumed; recovery requires a
  fresh `LinkDevice`.
- **`handle_receive` task** (~75 LoC). Owns the Manager. Filters out:
  - non-text content (`EditMessage`, `ReceiptMessage`, `TypingMessage`,
    `CallMessage`)
  - empty-body DataMessages (attachment-only, reaction-only)
  - `Received::Contacts` (the store has already absorbed the
    contact-sync results; not surfaced to MVP UI)
- **Decision 5 wiring**: `Received::QueueEmpty → store.flush_sessions()`.
  Errors are non-fatal — the next QueueEmpty retries.

### UI

- New `crates/xous-app-signal-ui/src/screens/conversation_list.rs`
  (~140 LoC). Holds:
  - `status: ReceiveStatus { Starting | Listening | Error(String) }`
  - `messages: Vec<MessageSummary { sender, body, timestamp }>`,
    capped at `MAX_VISIBLE * 2 = 16` entries (Stage 11+ moves to
    PDDB-backed pagination)
- `Screen::ConversationList(ConversationListScreen)` joins the
  enum. Routed via `Ui::handle_event` for the three new `Event`
  variants — each transitions a field of the screen rather than
  pushing a new screen.
- `Ui::on_screen_entered` gains the `ConversationList` arm: emits
  `Cmd::StartReceive`. Side-effects still funnel through the one
  function (audit-friendly invariant from Stage 9c preserved).
- `LinkDoneScreen::handle_key`'s `Home` transition now goes to
  `ConversationList` instead of `EmptyList` so the receive loop
  starts the moment linking completes.
- The Stage 9c `EmptyListScreen` is kept in the enum but no longer
  referenced from any flow. Audit-friendly: it'll show up unused
  in dead-code metrics but the trait surface is unchanged. Stage
  11+ may delete it.

## Verification

```sh
$ cargo test -p xous-app-signal-ui
test result: ok. 24 passed; 0 failed; 0 ignored
# 18 from Stage 9c+10 + 6 new Stage 11:
#   conversation_list_starts_in_starting_status
#   receive_started_event_sets_listening_status
#   message_event_appends_to_list
#   receive_error_event_sets_error_status
#   message_event_ignored_when_not_on_conversation_list
#   message_list_caps_at_max_visible_times_two
# (also updated: link_done_home_transitions_to_conversation_list)

$ cargo test -p xous-signal-bridge          ✓ 3 passed
$ cargo test -p presage-store-pddb          ✓ 22 passed

$ cargo check --target=riscv32imac-unknown-xous-elf -p xous-app-signal
✓ Full rv32 cross-compile of the entire stack still passes.

$ cargo clippy --workspace --all-targets -- -D warnings   ✓ clean
$ cargo fmt --all -- --check                              ✓ clean
```

## Test coverage delta (6 new tests + 1 updated)

| Test | What it asserts |
|---|---|
| `conversation_list_starts_in_starting_status` | After `LinkDone + Home`, top is `ConversationList` with `status=Starting`, `messages=[]` |
| `receive_started_event_sets_listening_status` | `Event::ReceiveStarted` flips status to `Listening` |
| `message_event_appends_to_list` | Two consecutive `Event::Message` calls produce `messages.len() == 2` in arrival order |
| `receive_error_event_sets_error_status` | `Event::ReceiveError(reason)` sets `status=Error(reason)` |
| `message_event_ignored_when_not_on_conversation_list` | Stale `Event::Message` arriving while UI is on splash is dropped (no crash, no off-screen mutation) |
| `message_list_caps_at_max_visible_times_two` | Pushing 50 messages keeps only the latest 16 — bounded memory |
| `link_done_home_transitions_to_conversation_list` (updated) | Replaces the Stage 10 `link_done_home_transitions_to_empty_list` test. After `LinkDone + Home`, top is `ConversationList` and `Cmd::StartReceive` lands in the cmd queue |

## Files changed

```
modified:
  Cargo.lock
  crates/xous-signal-bridge/src/cmd.rs                      (+StartReceive,
                                                             +ReceiveStarted/Message/
                                                              ReceiveError)
  crates/xous-signal-bridge/src/lib.rs                      (+handle_receive
                                                             ~75 LoC,
                                                             +StartReceive Cmd
                                                             dispatch)
  crates/xous-app-signal-ui/src/lib.rs                      (+handle_event arms
                                                             for the three new
                                                             events,
                                                             +on_screen_entered
                                                             arm,
                                                             +top_id mapping,
                                                             +6 tests, 1 updated)
  crates/xous-app-signal-ui/src/screen.rs                   (+ConversationList
                                                             variant + render +
                                                             handle_key + hint)
  crates/xous-app-signal-ui/src/screens/mod.rs              (+pub mod
                                                             conversation_list;)
  crates/xous-app-signal-ui/src/screens/link.rs             (LinkDone Home
                                                             transitions to
                                                             ConversationList)

new:
  crates/xous-app-signal-ui/src/screens/conversation_list.rs (~140 LoC)
  stage/REPORT-11.md                                         (this file)
```

## What's deferred

- **End-to-end decrypt test against a real ciphertext.** Stage 9b's
  Renode + mocked-Signal-server harness will inject a known-good
  envelope that exercises the `ServiceCipher::open_envelope` →
  `SessionStore::store_session` → `Event::Message` chain on real
  hardware. Stage 11 ships the logic; the integration gate lands
  with Stage 9b.
- **Per-thread grouping.** UI.md §5.7 has the populated conversation
  list grouped by thread with bold-for-unread + filled-rect badge.
  Stage 11 MVP is a flat chronological list. Per-thread grouping
  needs a per-thread cache that Stage 11+1 can build from
  `presage::ContentsStore::messages` (already populated by presage
  internals as messages arrive).
- **PDDB-backed message persistence on UI restart.** Right now
  `ConversationListScreen.messages` is in-memory only (capped at
  16). Restarting the app loses the visible list — though the
  underlying PDDB has all messages from `ContentsStore::save_message`.
  Stage 11+1 reads from PDDB on screen entry.
- **Concurrent send-while-receiving.** The receive task moves the
  Manager out of `linked: Option<...>`. After `StartReceive`, no
  other Cmd has access to the Manager. **This blocks Stage 12's
  `SendMessage`** — Stage 12 must refactor: either move
  send-cmd-handling *into* the receive task (multiplex via an
  inner channel), or share the Manager via `Rc<RefCell<...>>` and
  yield the borrow between stream items, or split the Manager's
  send/receive roles. Pick at the start of Stage 12.
- **Receive loop survives WS reconnect.** When the underlying WS
  closes (network blip), the stream returns `None` and our handler
  emits `ReceiveError`. presage's library does not auto-reconnect
  in v0.91. A robust client wraps `receive_messages` in an outer
  retry loop with exponential backoff. Stage 11+1 follow-up.
- **Reading control-message effects from the store.** Stage 11
  filters out non-text DataMessages, but presage's internals
  *do* mutate the store for those (e.g., `ReadReceipt`,
  `TypingMessage`-driven contact updates). Stage 11 never displays
  those effects; a real conversation view (Stage 11 follow-up)
  would surface read receipts as `✓✓` on outgoing bubbles.
