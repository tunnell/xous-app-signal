# UI.md — User-interface design for `xas` (xous-app-signal)

Target hardware: **Precursor**, 336 × 536 px monochrome Sharp Memory
LCD (1 bit per pixel, ~200 ppi), physical QWERTY keyboard with 4-way
arrows + center "Home" select key, no touchscreen, no function row.
~16 MiB total RAM with ~4 MiB usable per app.

This document is a design spec, not an implementation. It locks in the
**screens we need**, the **keyboard map**, the **ASCII mockup** for
each screen, and the **state graph** between them. Implementation is
later (Stage 9c+). Open questions are flagged.

The design draws heavily on a research memo (see chat history for the
full text) covering Signal's mobile reference UI, BlackBerry / Nokia /
Telegram-BB feature-phone conventions, terminal chat clients (irssi,
weechat, gurk-rs, gomuks, profanity, mutt), and Xous-side prior art
(`libs/chat`, mtxchat, the GAM graphics layer). The memo's per-row
visual conventions and keyboard map are adopted; the memo's
recommendation to *use `libs/chat` as our UI base* is **not** —
that would re-introduce the workspace-merge architectural problem
Decision 7 explicitly avoids. Section 9 below addresses this.

## 1. Goals and constraints

- **End-user verifiability** (the project's driving value). Every
  pixel rendered should be defensible from a few hundred lines of UI
  code we own; no opaque widget toolkit.
- **Keyboard-only.** No touch, no scroll wheel. Arrow keys + Home +
  printable ASCII. No `Alt`/`F-keys` on Precursor.
- **Monochrome.** Information density encoded in shape, weight,
  position, and inversion only.
- **Memory-conservative.** ≤ 8 KiB RAM for transient UI state,
  ≤ 32 KiB for the per-screen working buffer. The 4 MiB app budget
  is mostly for `libsignal` and TLS state.
- **Single-thread, sync.** UI runs in `xous-app-signal`'s main
  thread; manager events arrive over the `Event` channel from
  `xous-signal-worker`. No locks, no `Send` plumbing, no async UI.

## 2. Screen inventory

| # | Screen | Stage that needs it | What it shows |
|---|---|---|---|
| 1 | Splash / first-run | 9c | App started, not yet registered. Two CTA's: "Link a phone" / "Register a number". |
| 2 | Linking — show URL | 10 | The `tsdevice://...` provisioning URL, optionally rendered as a QR. Status: "Waiting for scan…". |
| 3 | Linking — confirm | 10 | "Confirm on phone" prompt while presage waits for the secondary-device confirmation. |
| 4 | Linking — done | 10 | "Linked as `<device name>`. Press Home to continue." |
| 5 | Linking — error | 10 | Error string + Retry / Cancel choice. |
| 6 | Empty conversation list | 11+ | "No conversations yet. Press Menu to start one." |
| 7 | Conversation list | 11+ | The main screen. Per-row name, timestamp, preview, unread badge. |
| 8 | Conversation view (read) | 11+ | Messages in a thread, scrollable. |
| 9 | Conversation view (compose) | 12 | Same screen with the bottom-row text input active. |
| 10 | App menu | always | New chat / Link device / Register / Settings / About / Quit. |
| 11 | About | always | Version, libsignal hash, build date. |
| 12 | Toast / banner | transient | One-line transient overlay for errors / "message sent" etc. |

12 screens total. Most of MVP work is screens 7-9.

## 3. Layout primitives

All screens share three structural elements:

```
┌──────────────────────────────────────────────────┐  ← top hairline
│ STATUS BAR  (24 px)                              │
├──────────────────────────────────────────────────┤
│                                                  │
│              CONTENT AREA  (492 px)              │
│                                                  │
├──────────────────────────────────────────────────┤
│ HINT FOOTER (20 px)                              │
└──────────────────────────────────────────────────┘  ← bottom hairline
```

### 3.1 Status bar (24 px tall)

```
┌──────────────────────────────────────────────────┐
│ xas         [WiFi] [TLS] 14:32  ▲ 12  ●●         │
└──────────────────────────────────────────────────┘
```

- **App name** (`xas`), bold, left-aligned.
- **Connection chips**: `[WiFi]` if a network IP is bound,
  `[TLS]` if the WS to chat.signal.org is open, `[OFF]` if
  neither. White-on-black inverted box per chip.
- **Clock**, 24-hour, center.
- **Total unread badge**: `▲ <n>` if any thread has `unread > 0`;
  filled triangle prefix because filled rect would compete with
  per-row badges.
- **Worker status indicator**: `●●` solid = idle, `○●` /  `●○`
  alternating = receive or send in flight, `××` = worker thread
  dead. Pure visual cue; not interactive.

### 3.2 Hint footer (20 px tall)

Per-screen. Lists the 3-5 most relevant key actions. Examples:

- Conversation list: `↑↓ Select   Home Open   n Next unread   ☰ Menu`
- Conversation view: `↑↓ Scroll   ← Back   Home Reply   ☰ Menu`
- Linking screen: `Home OK   ← Cancel`
- Menu: `↑↓ Select   Home Choose   ← Close`

### 3.3 Content area (492 px tall)

The actual screen content. Each screen fills this region. Section 5
shows ASCII mockups for each.

## 4. Visual encoding (no color allowed)

| Semantic | Encoding |
|---|---|
| Unread row | Bold name + bold preview + leading `●` glyph + trailing filled-rect badge `[<n>]` |
| Read row | Regular weight, no leading glyph |
| Selected / focused row | Whole-row inversion (black bg, white fg), bold/weight unchanged |
| Pinned row | Leading `▪` glyph; pinned rows grouped at top, separated by `══` double rule |
| Muted | Trailing `M` after name |
| Outgoing message status | Leading glyph in preview / message: `⏱` pending, `✓` sent, `✓✓` delivered, `!` failed |
| Section header | Inverted small-caps text inside double horizontal rule |
| Row separator | 1-px horizontal rule |
| Section separator | 2-px horizontal rule |
| Overlay / modal border | 2-px border, white background, drop-shadow effect via single offset rule |

Adopting from the research memo: Signal mobile uses bold-for-unread +
filled-circle counter; Telegram-BB uses bold-for-unread +
filled-rectangle counter; mutt uses single-letter status flag column
(`N`/`O`/`r`/`F`/`D`); BlackBerry uses leading envelope glyph. Our
encoding combines these — bold-for-unread is the primary cue; the
leading `●` is a redundancy aid (supports users who can't reliably
tell weight at-a-glance on a 1-bit display); the filled-rect badge
is the count.

## 5. Per-screen mockups

Width: 50 chars (≈ 336 px / 7-px-per-char). Height: ~20 rows of content
plus status (1 row) + footer (1 row) = 22 rows total. ASCII shown is
indicative; real rendering uses GAM `TextView`s with bold/regular
weight selection.

### 5.1 Splash / first-run (Screen #1)

```
┌──────────────────────────────────────────────────┐
│ xas         [OFF]                14:32     ●●    │
├──────────────────────────────────────────────────┤
│                                                  │
│                                                  │
│                       xas                        │
│                                                  │
│           Signal client for Precursor            │
│                                                  │
│                  Not yet linked.                 │
│                                                  │
│         ┌────────────────────────────┐           │
│         │  ▸ Link this device        │           │
│         │    Register a phone number │           │
│         │    About                   │           │
│         │    Quit                    │           │
│         └────────────────────────────┘           │
│                                                  │
│                                                  │
│                                                  │
│                                                  │
├──────────────────────────────────────────────────┤
│  ↑↓ Select   Home Choose                         │
└──────────────────────────────────────────────────┘
```

The ▸ is the focus glyph; it cycles with arrows. "Link this device"
is the recommended path (uses an existing Signal account on a phone).
"Register a phone number" is the alternative for users who want
their Precursor to be the primary device — a Stage 13+ feature, the
button can be greyed out via `[ Register a phone number ]`-style
brackets to signal it's not yet implemented.

### 5.2 Linking — show URL (Screen #2)

```
┌──────────────────────────────────────────────────┐
│ xas         [WiFi][TLS]          14:33     ●○    │
├──────────────────────────────────────────────────┤
│                                                  │
│              Link this device                    │
│                                                  │
│   1. On your phone, open Signal.                 │
│   2. Settings → Linked Devices → Link new.       │
│   3. Scan this code or enter the URL below.      │
│                                                  │
│        ████  ██  ██  ████  ██  ██████████        │
│        ██  ████  ████  ██████  ██  ██  ██        │
│        ██████  ██  ██████  ██  ██  ██████        │  ← QR ~120 px square,
│        ██  ██████████  ██████████████  ██        │     centered
│        ██████  ██  ██  ██  ██  ██  ████          │
│        ██  ██  ██████████  ████  ██  ██          │
│        ██████████  ██████  ██████████████        │
│                                                  │
│   tsdevice:/?uuid=…&pubkey=…                     │
│                                                  │
│   Waiting for scan…                              │
│                                                  │
├──────────────────────────────────────────────────┤
│  Home Cancel                                     │
└──────────────────────────────────────────────────┘
```

The `tsdevice://` URL is the same one Signal phones expect from a
secondary-device QR. Below the QR, the URL is rendered as text in case
the phone's QR scanner can't lock onto a 120-px monochrome target —
the user can type it, copy it, or photograph the screen for a
larger-display read.

### 5.3 Linking — confirm (Screen #3)

```
┌──────────────────────────────────────────────────┐
│ xas         [WiFi][TLS]          14:34     ●○    │
├──────────────────────────────────────────────────┤
│                                                  │
│              Link this device                    │
│                                                  │
│   ✓ Code scanned.                                │
│                                                  │
│   On your phone, you should now see:             │
│                                                  │
│        "Link this device as 'Precursor'?"        │
│                                                  │
│   Tap Link on your phone to continue.            │
│                                                  │
│   This may take 30–60 seconds.                   │
│                                                  │
│                                                  │
│             ──────────────                       │
│                  [...]                           │   ← spinner / busy anim
│             ──────────────                       │
│                                                  │
│                                                  │
│                                                  │
├──────────────────────────────────────────────────┤
│  Home Cancel                                     │
└──────────────────────────────────────────────────┘
```

### 5.4 Linking — done (Screen #4)

```
┌──────────────────────────────────────────────────┐
│ xas         [WiFi][TLS]          14:35     ●●    │
├──────────────────────────────────────────────────┤
│                                                  │
│                                                  │
│                       ✓                          │
│                                                  │
│                  Linked.                         │
│                                                  │
│       Device name: Precursor                     │
│       ACI: a3b9f1c2-…                            │
│       Phone: +1 415 555 0199                     │
│                                                  │
│                                                  │
│       You can now receive and send messages.     │
│                                                  │
│                                                  │
│                                                  │
├──────────────────────────────────────────────────┤
│  Home Continue                                   │
└──────────────────────────────────────────────────┘
```

Pressing Home transitions to Screen #6 or #7 depending on whether the
PDDB has any threads cached yet (the link flow downloads the contact
list and existing thread history; on first link there are no threads
locally, so #6 — the empty list — is what the user sees).

### 5.5 Linking — error (Screen #5)

```
┌──────────────────────────────────────────────────┐
│ xas         [WiFi][OFF]          14:35     ●●    │
├──────────────────────────────────────────────────┤
│                                                  │
│                                                  │
│                       ✗                          │
│                                                  │
│              Linking failed.                     │
│                                                  │
│   Reason:                                        │
│     no route to chat.signal.org                  │
│                                                  │
│                                                  │
│   ┌────────────────────────────┐                 │
│   │  ▸ Try again               │                 │
│   │    Cancel                  │                 │
│   └────────────────────────────┘                 │
│                                                  │
│                                                  │
│                                                  │
├──────────────────────────────────────────────────┤
│  ↑↓ Select   Home Choose                         │
└──────────────────────────────────────────────────┘
```

### 5.6 Empty conversation list (Screen #6)

```
┌──────────────────────────────────────────────────┐
│ xas         [WiFi][TLS]          14:36           │
├──────────────────────────────────────────────────┤
│                                                  │
│                                                  │
│                                                  │
│              No conversations yet.               │
│                                                  │
│       Press Menu to start a new chat or          │
│       sync your contact list.                    │
│                                                  │
│                                                  │
│                                                  │
│                                                  │
│                                                  │
│                                                  │
│                                                  │
│                                                  │
│                                                  │
│                                                  │
├──────────────────────────────────────────────────┤
│  ☰ Menu                                          │
└──────────────────────────────────────────────────┘
```

No empty list. No empty grid. A short instruction directing the user
at the only useful action. The hint footer collapses to one item.

### 5.7 Conversation list — populated (Screen #7)

```
┌──────────────────────────────────────────────────┐
│ xas         [WiFi][TLS]    14:36   ▲ 4   ●●      │
├──────────────────────────────────────────────────┤
│ ▪ Alice Nguyen                  2m            ● 3│  ← pinned, 3 unread
│   sure, meet at 6                                │
├──────────────────────────────────────────────────┤
│ ▸ Bob Kowalski                  12m           ● 1│  ← focused + 1 unread
│   did you get the file?                          │
├──────────────────────────────────────────────────┤
│   Carol Whitfield               1h               │  ← read
│   Thanks!                                        │
├──────────────────────────────────────────────────┤
│   Dad                           Yesterday        │
│   ✓✓ On my way                                   │
├──────────────────────────────────────────────────┤
│   +1 415 555 0199               Mon              │
│   Your Uber has arrived                          │
├──────────────────────────────────────────────────┤
│   Eve Tanaka                    Mar 12           │
│   ! Lunch tomorrow?                              │  ← last outgoing failed
├──────────────────────────────────────────────────┤
│   Frank                         Feb 28           │
│   See you then.                                  │
├──────────────────────────────────────────────────┤
│   Group chat is deferred to v2                   │
│                                                  │
├──────────────────────────────────────────────────┤
│  ↑↓ Select  Home Open  n Next unread  ☰ Menu     │
└──────────────────────────────────────────────────┘
```

Per-row layout (left → right):

- 12-px state column: `▪` (pinned), `▸` (focused), or blank
- Display name, bold if `unread > 0`, ellipsized at ~60% width
- Right-aligned: relative timestamp ("2m", "1h", "Yesterday",
  "Mon", "Mar 12"), bold if unread
- Right-edge: filled-rect unread badge `● <n>` if `unread > 0`,
  omitted otherwise; for `n ≥ 100` shows `99+`
- Second line: last-message preview, ellipsized; outgoing-message
  status glyph (`⏱`/`✓`/`✓✓`/`!`) prepended if last message was
  outgoing

Pinned rows go at the top in pin-add order (no auto-resort within
the pinned section, matching Signal Android's documented behaviour).
Unpinned rows sort by `last_message_ts desc`. Section break between
pinned and unpinned is a 2-px rule; rows within a section are
1-px-rule separated.

Row height ≈ 48 px: 22-px primary line + 22-px preview line + 4-px
padding. With status (24 px) + footer (20 px) + content (492 px),
492 / 48 = 10 visible rows + a half-row preview of the next.

### 5.8 Conversation view — reading (Screen #8)

```
┌──────────────────────────────────────────────────┐
│ ← Bob Kowalski                       ●○          │
├──────────────────────────────────────────────────┤
│                                                  │
│                                       Mon 14:30  │
│                  ┌──────────────────────────┐    │
│                  │ Did you get the file?    │    │
│                  └──────────────────────────┘    │
│                                                  │
│                                       Mon 14:31  │
│                  ┌──────────────────────────┐    │
│                  │ Yes, looks good!         │    │
│                  └──────────────────────────┘ ✓✓ │
│                                                  │
│   Mon 14:32                                      │
│   ┌─────────────────────────────────────┐        │
│   │ Great, can you send the signed PDF? │        │
│   └─────────────────────────────────────┘        │
│                                                  │
│   2m ago                                         │
│   ┌────────────────────────┐                     │
│   │ Working on it now      │                     │
│   └────────────────────────┘                     │
│                                                  │
│                                                  │
│                                                  │
├──────────────────────────────────────────────────┤
│  ↑↓ Scroll   ← Back   Home Reply   ☰ Menu        │
└──────────────────────────────────────────────────┘
```

Top bar replaces the global status bar with a per-thread header:
back-arrow + contact name on left; worker indicator on right. The
TLS/wifi chips compress to make space. (Optional: overflow them into
the menu if the header crowds.)

Outgoing messages right-aligned, incoming left-aligned, BBM/SMS
convention. Bubble has a 1-px border and rounded-corner rendering
where the GAM supports it (otherwise square corners are fine —
visual grouping matters more than aesthetics).

Last outgoing message's status (`✓`/`✓✓`/etc.) is shown to the
*right of the bubble*, separated from message content (Signal mobile
puts it inside; on a 336-px-wide LCD, outside is more legible).

### 5.9 Conversation view — composing (Screen #9)

```
┌──────────────────────────────────────────────────┐
│ ← Bob Kowalski                       ●○          │
├──────────────────────────────────────────────────┤
│                                       Mon 14:32  │
│                  ┌──────────────────────────┐    │
│                  │ Yes, looks good!         │    │
│                  └──────────────────────────┘ ✓✓ │
│                                                  │
│   2m ago                                         │
│   ┌────────────────────────┐                     │
│   │ Working on it now      │                     │
│   └────────────────────────┘                     │
│                                                  │
│                                                  │
│                                                  │
│                                                  │
│                                                  │
│                                                  │
│                                                  │
├──────────────────────────────────────────────────┤
│ ┌──────────────────────────────────────────────┐ │
│ │ Sending you the signed PDF in a min.|        │ │  ← compose box,
│ │                                              │ │     1-2 lines tall,
│ │                                              │ │     grows downward
│ └──────────────────────────────────────────────┘ │
│  Home Send   ← Back (empty input)   Esc Discard  │
└──────────────────────────────────────────────────┘
```

The compose box absorbs the bottom 80 px of the content area; the
hint footer slides up to be the prompt help. While the input has
text, `←` moves the cursor; when empty, `←` exits to the conversation
list (same convention BBM uses).

Press `Home` to send. While the message is in flight a small `⏱`
prefix shows on the new (bottom) bubble; on success it becomes `✓`,
on delivery `✓✓`, on failure `!`.

### 5.10 App menu (Screen #10)

```
┌──────────────────────────────────────────────────┐
│ xas         [WiFi][TLS]          14:38     ●●    │
├──────────────────────────────────────────────────┤
│                                                  │
│           ┌──────────────────────────┐           │
│           │       xas — Menu         │           │
│           ├──────────────────────────┤           │
│           │ ▸ New chat               │           │
│           │   Mark all read          │           │
│           │   ──────────────────     │           │
│           │   Link another device    │           │
│           │   Settings               │           │
│           │   About                  │           │
│           │   ──────────────────     │           │
│           │   Quit                   │           │
│           └──────────────────────────┘           │
│                                                  │
│                                                  │
│                                                  │
│                                                  │
│                                                  │
│                                                  │
├──────────────────────────────────────────────────┤
│  ↑↓ Select   Home Choose   ← Close               │
└──────────────────────────────────────────────────┘
```

Modal overlay; the main screen behind it greys out (or dithers, if
GAM doesn't support real grey). Section breaks between groups
(actions / device-management / quit).

### 5.11 About (Screen #11)

```
┌──────────────────────────────────────────────────┐
│ xas         [WiFi][TLS]          14:38     ●●    │
├──────────────────────────────────────────────────┤
│                                                  │
│                       xas                        │
│            (xous-app-signal v0.1.0)              │
│                                                  │
│   ────────────────────────────────────────       │
│                                                  │
│   Built: 2026-05-06 14:00 UTC                    │
│   Toolchain: rustc 1.85 stable                   │
│                                                  │
│   libsignal:        v0.91.0  (98915c44)          │
│   libsignal-svc-rs: forked HEAD                  │
│   presage:          forked HEAD                  │
│   curve25519-dalek: 4.1.3 (betrusted+lizard)     │
│   libcrux-ml-kem:   0.0.8                        │
│   spqr:             1.5.1                        │
│   smol-rs:          pinned                       │
│                                                  │
│   Signal Trust Root: pinned                      │
│   PDDB basis:        signal                      │
│                                                  │
├──────────────────────────────────────────────────┤
│  ← Back                                          │
└──────────────────────────────────────────────────┘
```

About is a deliberate end-user verifiability surface. Listing every
upstream version makes it possible to take a photo of this screen
and reproduce the exact build.

### 5.12 Toast / banner (Screen #12)

A 32-px overlay that drops in from the top of the content area,
shows for 2-3 seconds, then slides out. Used for:

- "Message sent." (green check, 2 s)
- "Message failed: <reason>." (red `!`, 5 s)
- "Network reconnected." (info, 2 s)
- "Network lost." (red, 5 s)

```
                  ┌────────────────────────────┐
                  │ ✓ Message sent             │
                  └────────────────────────────┘
```

Doesn't block input; user can keep navigating beneath it.

## 6. State graph

Transitions between screens. Edges labeled with the trigger.

```
                       (start)
                          │
                          ▼
                  ┌──────────────────┐
                  │  1. Splash /     │
                  │     first-run    │
                  └──────────────────┘
                       │       │
            "Link"    Home    Home   "Register"
                       ▼       ▼      (deferred)
                  ┌──────────────────┐
                  │  2. Link — show  │
                  │     URL          │
                  └──────────────────┘
                       │       │
                  scan ok    cancel
                       ▼       ▼
                  ┌──────────────────┐         (back to 1)
                  │  3. Link —       │
                  │     confirm      │
                  └──────────────────┘
                       │       │
                  success    error
                       ▼       ▼
                ┌──────────┐  ┌──────────────┐
                │ 4. Done  │  │  5. Error    │──→ retry → 2
                └──────────┘  └──────────────┘
                       │
                      Home
                       ▼
            ┌──────────────────────┐
            │  6. Empty list  /    │
            │  7. Conversation     │← (decided by store state)
            │     list             │
            └──────────────────────┘
                  │      ↑       │
                Home    ←       Menu
                  ▼      │       ▼
           ┌──────────────┐ ┌──────────────┐
           │ 8. Conv view │ │ 10. App menu │
           │    (read)    │ │              │
           └──────────────┘ └──────────────┘
                  │              │
                Home          (action)
                  ▼              ▼
           ┌──────────────┐  (varies)
           │ 9. Conv view │
           │  (compose)   │
           └──────────────┘
```

**Screens 11 (About) and 12 (Toast)** can pop from any screen via
the menu and any worker event respectively, and don't have outgoing
edges that affect state — they always return to the underlying
screen.

## 7. Keyboard map

Adopting most of the research memo's recommendations. One global
rule: **the same key never means two different things on the same
screen.** Disambiguation is by context (focus = list vs. focus =
compose box).

| Context | Key | Action |
|---|---|---|
| List | `↑` / `↓` | Move focus |
| List | `Shift+↑` / `Shift+↓` | Page up / page down |
| List | `Home`, `→`, `Enter` | Open focused conversation |
| List | `n` | Jump to next conversation with `unread > 0` |
| List | `1`-`9`, `0` | Jump to conversation 1-10 |
| List | `u` | Toggle mark-unread on focused row |
| List | `p` | Toggle pin on focused row |
| List | `Menu` | Open app menu |
| Conv read | `↑` / `↓` | Scroll messages |
| Conv read | `←` | Back to list |
| Conv read | `Home` | Drop into compose |
| Conv read | `Menu` | Conversation menu (mark unread, archive, …) |
| Compose | `Home` | Send |
| Compose | `←` (when input non-empty) | Move cursor left |
| Compose | `←` (when input empty) | Back to read view |
| Compose | `Esc` | Discard draft, back to read view |
| Menu | `↑` / `↓` | Move focus |
| Menu | `Home`, `Enter` | Choose item |
| Menu | `←`, `Esc` | Close menu |
| Linking | `Home` | OK / Continue |
| Linking | `←`, `Esc` | Cancel |

All printable ASCII goes to the compose-box input when it's focused.
On the list, printable ASCII is reserved for the navigation shortcuts
(`n`, `u`, `p`, `1`-`9`); other letters are ignored. Future v2 may
type-to-search the list (Kindle-style) — defer.

## 8. Memory budget per screen

The 4 MiB app budget is mostly for libsignal + zkgroup + ML-KEM-1024
+ TLS state. UI must be a small slice. Per-screen targets (working
set when displayed):

| Screen | RAM budget | Why |
|---|---|---|
| Splash | < 1 KiB | Static text; one menu list. |
| Linking — show URL | ~4 KiB | QR codeword buffer (~100×100 b/w bits) + status string. |
| Linking — confirm/done/error | < 1 KiB | Static text + status. |
| Empty list | < 1 KiB | No data. |
| Conversation list | ~8 KiB | `Vec<DialogueSummary>` for ≤ 30 threads. The summaries are pre-computed at PDDB mount; per-display reads only update the last-message snippet. See §10. |
| Conversation view | ~32 KiB | Message bubbles for current viewport (≈ 10 messages × ~3 KiB Content struct including metadata). Older messages re-fetched from PDDB on scroll; no full-history caching. |
| Compose | + 4 KiB | Input ring buffer (max 4 KiB per Signal protocol). |
| App menu | < 1 KiB | Static items. |
| About | < 1 KiB | Static. |
| Toast | < 256 B | One string + a tick counter. |

Total when "list + worker idle + no overlay": ~10 KiB UI working set.
When in conversation view: ~40 KiB. Both fit comfortably with the
remaining ~3.95 MiB available for libsignal/TLS work.

## 9. The `libs/chat` decision

**Not adopting `libs/chat` from xous-core.** Building the UI directly
on Xous GAM primitives (`TextView`, `Canvas`, `Modal`) inside a new
`xous-app-signal-ui` crate.

The research memo recommends adding a `ChatScreen::List` state to
`libs/chat` (`xous-core/libs/chat/src/ui.rs`, 793 lines). Rejected:

1. **Decision 7 conflict.** `libs/chat` is xous-core code. Path-
   dep'ing it from our standalone workspace replays the workspace-
   merge problem (xous-core's `[patch.crates-io].aes` and friends
   apply globally, breaking libsignal). Vendoring `libs/chat` is
   plausible (~2 kLoC, similar pattern to our `libsignal-service-rs`
   vendor) but adds another fork to track against upstream.

2. **Storage-model mismatch.** `libs/chat` has its own `Dialogue` /
   `Post` / `Author` model, persisted to a `sigchat.dialogue` PDDB
   dict. Ours is `presage::Content` / `Metadata` / `Thread`,
   persisted to `signal.threads.<sha256>` per Decision 1. Sharing
   the UI library would require a translation layer at every
   render — extra code, not less.

3. **Different state machines.** `libs/chat::Chat` is a server with
   its own opcode loop; we already have `xous-signal-worker` running
   the manager state machine over `Cmd`/`Event` channels. Putting
   `libs/chat` between them adds a third loop with its own scheduling
   semantics.

4. **Different model.** `libs/chat` was designed for Matrix-style
   group rooms with author attribution. Signal 1:1 has no author
   attribution within a thread (the thread *is* the contact); group
   v2 has a different attribution model (member-list with roles).
   Our renderer can be simpler.

5. **Unfair-comparison alternative.** Writing the UI ourselves on
   raw GAM is ~2-3 kLoC (12 screens × ~200 LoC each + ~500 LoC for
   the per-row renderer + state graph + key router). That's a couple
   weeks. Vendoring `libs/chat` would save ~1 kLoC but add maintenance
   cost. Net benefit is small; the architectural cleanness benefit
   is large.

What we **do** adopt from `libs/chat` and the research:

- The per-row visual encoding (bold-for-unread, leading glyph,
  right-aligned relative timestamp, trailing filled-rect badge).
- The keyboard map (arrows + Home + `n`/`u`/`p`/`1`-`9`).
- The PDDB-backed "summary cache, render lazily" memory pattern.
- The general "single screen at a time + modal overlay for menu"
  state-machine shape.

What we **don't** adopt:

- The `Chat` server / opcode loop.
- The `Dialogue` / `Post` / `Author` storage model.
- The icontray menu (Precursor's keyboard-only model favours a
  vertical menu list; icontray's three-icon row optimises for
  Precursor's older soft-key bar which `xas` doesn't bind).

## 10. Data flow: worker → UI

Every screen reads from the manager state machine and writes by
emitting commands. The existing `xous-signal-worker` `Cmd`/`Event`
channels gain new variants (Stages 10-12 will wire these):

```rust
// xous-signal-worker/src/cmd.rs (additions)
pub enum Cmd {
    // … existing Hello, GetWhoami, Shutdown …

    // Stage 10
    LinkBegin { device_name: String },
    LinkCancel,

    // Stage 11+
    ListThreads,                // emits ThreadList event
    OpenThread(ThreadId),       // emits MessagesPage event
    ScrollThread { thread: ThreadId, before_ts: u64, count: u32 },
    MarkUnread(ThreadId, bool),
    PinThread(ThreadId, bool),

    // Stage 12
    SendMessage { thread: ThreadId, body: String },
}

pub enum Event {
    // … existing Pong, Whoami, ShuttingDown …

    LinkUrl(String),                    // Stage 10
    LinkConfirming,
    LinkComplete { device_name: String, aci: String, phone: String },
    LinkError(String),

    ThreadList(Vec<ThreadSummary>),     // Stage 11+
    MessagesPage { thread: ThreadId, msgs: Vec<MessageSummary> },
    NewMessage { thread: ThreadId, msg: MessageSummary },

    SendStatus { thread: ThreadId, msg_id: u64, status: SendStatus },

    ConnState(ConnectionState),         // any time
    ToastError(String),
}

pub struct ThreadSummary {
    pub id: ThreadId,
    pub display_name: String,
    pub last_msg_snippet: String,
    pub last_msg_ts: u64,
    pub last_msg_outgoing: bool,
    pub last_msg_status: Option<SendStatus>,
    pub unread_count: u32,
    pub pinned: bool,
    pub muted: bool,
}
```

The conversation list lives entirely off `Event::ThreadList` (or
incremental `NewMessage` updates). The worker pre-computes
`ThreadSummary` from `presage::ContentsStore::messages` + the per-
thread metadata at PDDB-mount-time and again on every send/receive,
so the UI does no PDDB I/O on the render path.

## 11. Crate decomposition

A new crate joins the workspace at Stage 9c:

```
crates/
├── xous-app-signal/          binary "xas" (existing)
├── xous-app-signal-ui/       NEW — all UI, ~2-3 kLoC
├── xous-signal-worker/       Manager worker (existing; +Cmd/Event variants)
├── xous-net-bridge/          TLS + WS + HTTP (existing)
└── presage-store-pddb/       storage trait surface (existing)
```

`xous-app-signal-ui` exports:

- `Ui::new(cmd_tx, event_rx) -> Ui` — constructor.
- `Ui::run(self) -> !` — the main UI loop. Blocks on either GAM
  rawkey events or `event_rx.recv_blocking()`. Routes to the
  current screen. Returns only on `Quit` cmd.
- One module per screen (`splash.rs`, `link.rs`, `list.rs`,
  `conversation.rs`, `menu.rs`).

The binary's `main.rs` shrinks to:

```rust
fn main() {
    let store = PddbStore::new(/* pddb backend */);
    let (cmd_tx, cmd_rx) = bounded(16);
    let (event_tx, event_rx) = bounded(16);
    let _worker = run_signal_worker(store, cmd_rx, event_tx);

    let ui = Ui::new(cmd_tx, event_rx);
    ui.run();  // never returns
}
```

## 12. Mapping to ROADMAP stages

The ROADMAP currently doesn't have a UI-specific stage; UI is
implicit in Stages 10-12. Proposed addition: **Stage 9c — UI
scaffolding** between Stage 9b (xtask + Renode boot) and Stage 10:

- New `crates/xous-app-signal-ui` crate skeleton.
- Splash, About, App menu screens implemented.
- Conversation list renders an empty state.
- Status bar + hint footer + toast overlay primitives.
- Renode test asserts on the UART-logged "splash shown" + "menu
  opened" + "menu closed" lines.

Stages 10/11/12 then add screen content rather than rebuilding the
shell:

- **Stage 10 — link as secondary device:** screens 2/3/4/5 plus the
  `Cmd::LinkBegin` / `Event::LinkUrl` plumbing.
- **Stage 11 — receive a message:** the conversation list (#7) and
  conversation read view (#8) plus `ThreadList` / `MessagesPage` /
  `NewMessage` events.
- **Stage 12 — send a message:** compose view (#9) plus
  `Cmd::SendMessage` / `Event::SendStatus`.

After Stage 12 the MVP is complete.

## 13. What's deferred (not v1)

- **Group chats.** Signal Group v2 has a separate ceremony (zkgroup
  credentials, group-server interactions). Listed in the MVP but
  realistically v2.
- **Avatars.** Photo avatars don't dither well to monochrome. v1
  uses initials in a bordered square if anything; that itself is
  v2 polish.
- **Type-to-filter list (Kindle-style).** Letter keys are reserved
  for `n`/`u`/`p`/digits in v1. v2 can add a `/` to enter
  filter mode.
- **Archive view.** Signal mobile has it; defer.
- **Read receipts UI.** The protocol-level read receipts work via
  presage; making them visible in the conversation view (other
  side's `✓✓` vs `✓`) is straightforward but defer until Stage 12+1.
- **Typing indicators.** Cost too high on a refresh-on-event LCD.
- **Pull-to-refresh-as-filter** (Signal Android pulls down on the
  list to filter to unread). Cute, defer; `n` covers it.

## 14. Open questions (need user input before Stage 9c)

1. **Vendor `libs/chat` or build from scratch?** Recommended above:
   build from scratch in `xous-app-signal-ui`. If you want the
   `libs/chat` route instead (tight coupling to xous-core but ~1
   kLoC less code), say so and the ROADMAP changes.

2. **QR rendering.** `qrcode = "0.14"` is the obvious crate
   (cryptography-adjacent; pure-Rust; `default-features = false`
   drops the `image` dep). Confirm it's acceptable or pick an
   alternative.

3. **Splash screen logo.** Plain `xas` text vs. an actual rendered
   logo. Logo would need a 1-bit BMP asset shipped in the binary.

4. **Compose box max length.** Signal's per-message limit is
   ~2000 characters. Precursor's input UX won't enjoy that — the
   compose box at 80 chars × 5 lines is ~400 chars on screen. Is
   400 enough as a soft limit? Hard limit at 2000?

5. **Pinned-section behaviour.** Memo recommends "preserve pin-add
   order, don't auto-resort" (Signal Android). Confirm this is the
   shape we want, or fall back to "auto-resort by activity within
   pinned section" (Telegram BB).

6. **About screen verifiability list.** Is "every upstream version"
   sufficient, or should we also display the SHA-256 of the running
   binary (for tamper-evidence on a re-build)? The hash takes a few
   ms to compute on a Precursor; storing it at build time is simpler.

## TL;DR

12 screens, all monochrome, 50-char-wide, keyboard-only. Conversation
list is the centerpiece (Signal mobile's per-row spec adapted for
1-bit + bold-for-unread + leading `●` + filled-rect count badge). 
Build the UI in a new `xous-app-signal-ui` crate on Xous GAM
primitives (don't path-dep `libs/chat` — Decision 7 conflict). Add
Stage 9c to the ROADMAP for UI scaffolding before Stages 10-12.
Keyboard map adopts `↑↓` / `Home` / `←` / `n`/`u`/`p`/`1`-`9`. Memory
budget ~10 KiB UI working set (out of 4 MiB app budget); the rest
is libsignal + TLS state.

We're on a *similar* path to the research memo — same per-row
conventions, same key bindings, same memory discipline — but on a
different *foundation* (presage's ContentsStore + our own UI crate
on GAM, not libs/chat + libs/chat's Dialogue model). Stages 4-8 of
our work are preserved unchanged.
