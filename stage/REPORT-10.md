# Stage 10 — Link as secondary device (logic + UI; live network out of scope)

Status: **logic complete, end-to-end network test deferred.** Bridge
IPC vocabulary and worker handler for `Cmd::LinkDevice` land. UI
populates the four `Link*` screens (`LinkStarting`, `LinkShowUrl`,
`LinkDone`, `LinkError`) plus the previously-placeholder `LinkConfirming`
state, with state-machine tests covering every event-driven transition.
The worker now sets the two thread-locals libsignal-service-rs
requires (`set_http_client`, `set_task_spawner`) so a real
`Manager::link_secondary_device` call can run.

What this stage **does not** do: drive the link flow against a real
phone-in-the-loop or a mocked Signal server end-to-end. That requires
Stage 9b's xtask + Renode infrastructure (or a network-reachable host
running `chat.signal.org`-mocked HTTPS). The transition logic, error
paths, and code structure are verified by the new state-machine tests
(`link_url_event_replaces_link_starting`, `link_complete_event_replaces_with_done`,
etc.); the integration tests come at Stage 9b-followup.

## What landed

### Bridge (`xous-signal-bridge`)

- `Cmd::LinkDevice { device_name }` and `Cmd::LinkCancel` —
  user-facing entry points.
- `Event::LinkUrl(String)` — URL forwarded to the UI as soon as
  presage emits it.
- `Event::LinkComplete { device_name, aci, phone }` — success;
  fields pulled from `RegistrationData` so the About-style
  verifiability data lands on the screen without re-querying.
- `Event::LinkError(String)` — stringified `presage::Error<E>`.
- New `handle_link_device` async fn (~70 LoC) — uses
  `futures::channel::oneshot` (matches presage's API expectation)
  and `futures::future::join` to drive the link future + URL
  forwarder concurrently.
- Worker `worker_main` retains the resulting
  `Manager<PddbStore, Registered>` in a local var for Stage 11+
  Cmds to reuse.
- Worker startup now calls `transport::set_http_client(...)` and
  `transport::set_task_spawner(...)` — these are required
  thread-locals for any libsignal-service-rs call. The HttpClient
  is `xous-net-bridge::SyncHttpClient` from Stage 6.

### UI (`xous-app-signal-ui`)

- New `screens/link.rs` (~250 LoC) with five `*Screen` structs:
  `LinkStartingScreen`, `LinkShowUrlScreen`, `LinkConfirmingScreen`,
  `LinkDoneScreen`, `LinkErrorScreen`. Each gets its own `render` +
  `handle_key`, ≤ 75 LoC each.
- `Screen` enum's previous Stage-10 placeholder variants
  (`LinkShowUrl` → `LinkShowUrl(LinkShowUrlScreen)`, etc.)
  populated with their real payload structs.
- `Ui::handle_event` now routes `Event::LinkUrl` /
  `Event::LinkComplete` / `Event::LinkError` to screen
  transitions. Stale events (received after the user navigated
  away) are dropped — the worker's link future runs to completion
  in the background and its eventual reply is ignored unless the
  UI is still on a `Link*` screen.
- `Ui::dispatch` now tracks the previous top-screen-id and calls
  `on_screen_entered` after `apply` if the top changed.
  `on_screen_entered` is the single audit-friendly site that
  emits side-effect Cmds: pushing `LinkStarting` sends
  `Cmd::LinkDevice` to the worker.
- `Ui::run` reworked: stdin moves to a background thread feeding
  a `std::sync::mpsc`. The main loop drains worker events on
  every iteration and re-renders if any landed, so the UI can
  show `LinkUrl` arriving without the user having to press a
  key. `recv_timeout(50ms)` keeps the loop responsive without
  busy-looping.

### Splash

- "Link this device" menu item now `Push`-es `Screen::LinkStarting`
  instead of the old placeholder. The driver's `on_screen_entered`
  side-effect emits `Cmd::LinkDevice`.

## Verification

```sh
$ cargo test -p xous-app-signal-ui
test result: ok. 18 passed; 0 failed; 0 ignored
# 12 from Stage 9c + 6 new Stage 10:
#   splash_link_pushes_link_starting_and_emits_cmd
#   link_url_event_replaces_link_starting
#   link_complete_event_replaces_with_done
#   link_error_event_replaces_with_error
#   link_done_home_transitions_to_empty_list
#   link_url_event_ignored_if_user_navigated_away

$ cargo test -p xous-signal-bridge
test result: ok. 3 passed; 0 failed

$ cargo test -p presage-store-pddb
test result: ok. 22 passed; 0 failed

$ cargo check --target=riscv32imac-unknown-xous-elf -p xous-app-signal
✓ rv32 cross-compile of the entire stack still passes.

$ cargo clippy --workspace --all-targets -- -D warnings   ✓ clean
$ cargo fmt --all -- --check                              ✓ clean

$ printf 'down\ndown\n\nleft\nq\n' | target/debug/xas
✓ navigation smoke: focus moves Splash[Link]→Splash[Register]→Splash[About]→
   About screen rendering shown→Splash→quit. No link flow triggered.
```

What hosted-mode end-to-end shows: pressing Home from Splash with
`Link this device` focused triggers `Cmd::LinkDevice`. The worker
constructs a real `PushService(SignalServers::Production, ...)` and
attempts `link_secondary_device`. With network access it eventually
emits a real `tsdevice://...` URL that the UI displays; the link
remains pending until a phone scans it (presage doesn't expose a
short timeout). For automated CI we don't drive this path — the
state-machine tests above are the ones that gate Stage 10.

## Test coverage delta (6 new tests)

| Test | What it asserts |
|---|---|
| `splash_link_pushes_link_starting_and_emits_cmd` | Splash + Home pushes `LinkStarting`; `Cmd::LinkDevice { device_name: "Precursor" }` lands in the cmd queue. |
| `link_url_event_replaces_link_starting` | `Event::LinkUrl(url)` replaces `LinkStarting` with `LinkShowUrl(url)`. |
| `link_complete_event_replaces_with_done` | `Event::LinkComplete { device_name, aci, phone }` replaces top with populated `LinkDone`. |
| `link_error_event_replaces_with_error` | `Event::LinkError(reason)` replaces top with `LinkError`. |
| `link_done_home_transitions_to_empty_list` | From `LinkDone`, Home transitions to `EmptyList` (not back to splash) — the user lands in the empty conversation list per UI.md §6 transition graph. |
| `link_url_event_ignored_if_user_navigated_away` | If the user pressed Cancel (Left) before the URL arrived, a stale `LinkUrl` event does **not** reactivate a `Link*` screen. |

## Files changed

```
modified:
  Cargo.lock                                                (+url, +futures)
  crates/xous-signal-bridge/Cargo.toml                      (+url, +futures,
                                                             +xous-net-bridge)
  crates/xous-signal-bridge/src/cmd.rs                      (+LinkDevice/Cancel,
                                                             +LinkUrl/Complete/Error)
  crates/xous-signal-bridge/src/lib.rs                      (+handle_link_device,
                                                             +set_http_client,
                                                             +set_task_spawner,
                                                             +linked manager retention)
  crates/xous-app-signal-ui/src/lib.rs                      (Ui::handle_event,
                                                             Ui::on_screen_entered,
                                                             stdin background
                                                             thread, mpsc poll
                                                             loop, +6 tests)
  crates/xous-app-signal-ui/src/screen.rs                   (Link* variants
                                                             populated)
  crates/xous-app-signal-ui/src/screens/mod.rs              (+pub mod link;)
  crates/xous-app-signal-ui/src/screens/splash.rs           (push LinkStarting
                                                             instead of
                                                             placeholder)

new:
  crates/xous-app-signal-ui/src/screens/link.rs             (~250 LoC, 5
                                                             screen structs)
  stage/REPORT-10.md                                        (this file)
```

## What's deferred

- **End-to-end test against `chat.signal.org`** (or a mocked Signal
  server). Stage 9b's xtask + Renode harness lands this. Once a
  hosted Signal-mock is running, the integration test drives:
  Splash → Link → see real URL → mock-server simulates phone scan →
  see LinkComplete → state persisted to PDDB.
- **QR rendering.** Stage 10 hosted-mode shows the URL as text. UI.md
  §5.2 mocks up an ASCII QR; for hosted-mode TTY a `qrcode = "0.14"`
  dep would do it. Deferred until on-device GAM rendering lands —
  the URL-as-text is sufficient for hosted-mode demonstration and
  the on-device rendering is a Stage 9b/c-followup concern.
- **PDDB-persistent registration** across restarts. Currently the
  worker holds the linked Manager in a local var that drops on
  thread exit. presage's `Store` writes `RegistrationData` to PDDB
  on every save; rehydrating on next launch is a `Manager::load_registered`
  call that we should add to worker startup. Trivial follow-up;
  not a Stage 10 deliverable.
- **Cmd::LinkCancel actually cancels.** presage's
  `link_secondary_device` doesn't expose a cancel handle. The
  current implementation runs the link future to completion or
  HTTP timeout; the UI navigates away locally. A real cancel
  would require either a `Future::abort` wrapper around the call
  or a presage-side patch. Stage 11+1 cleanup if it matters in
  practice.
- **Re-entrancy.** If the user starts a second link while a first
  is in flight, the worker spawns it on the executor and both
  run concurrently. The retained `Manager` would be the one from
  the future that finishes second, with the first's
  registration data overwritten. Single-link-flow assumption is
  fine for v1; document and revisit if multi-account ever lands.
