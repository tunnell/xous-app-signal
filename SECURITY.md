# Security policy

xas is a **prototype** Signal client for Precursor/Xous (see the
README's "Not for production use" banner). This document states the
threat model reviews are conducted against, what is in and out of
scope for reports, and how to report. Modeled on rustls's policy,
adapted to a pocket device.

## Reporting a vulnerability

Use GitHub's private vulnerability reporting on this repository
(Security → Report a vulnerability). If that is unavailable, open a
plain issue saying only "security contact requested" — no details —
and the maintainer will arrange a private channel.

Please do not put reproduction details, captured traffic, or account
identifiers (phone numbers, ACIs) in public issues. PCAPs of real
Signal traffic carry phone numbers and ACI UUIDs and must never be
attached publicly.

There is no bug bounty. Reports are triaged best-effort by a single
maintainer; expect days, not hours.

## Threat model

Attacker tiers, strongest claim first. "Protects" means: by design,
with the code in this tree plus its trust assumptions below.

1. **Network attacker** (Wi-Fi AP, ISP, on-path). Protected. All
   traffic is TLS 1.3 to Signal's endpoints, verified against
   Signal's pinned production CA only (`crates/xous-net-bridge/
   certs/`); the device's public CA bundle is not trusted for this.
   Message content is additionally end-to-end encrypted by the
   Signal Protocol. Residual: traffic analysis (timing, sizes,
   endpoints) — same as every Signal client.
2. **Malicious peer** (hostile Signal contact). Protected by the
   upstream Signal Protocol implementation (libsignal) for
   cryptographic properties. Residual: parsing/handling of
   attacker-controlled message content in xas's own UI code — in
   scope, report it.
3. **Signal server compromise**. Metadata visible (who/when/sizes);
   content protected end-to-end; identity impersonation prevented by
   identity keys. A malicious prekey bundle can attempt a
   future-secrecy attack, same as official clients. Out of scope for
   xas except where xas weakens the upstream posture.
4. **Physical access, no bus probing** (borrowed/seized device,
   powered off or locked). Protected by PDDB encryption at rest
   under the user's PDDB password and Precursor's sealed key
   hierarchy. The PDDB password is the primary disclosure surface:
   with it, an attacker gets registration, sessions, and (when
   history persistence lands) messages. In scope: any path where xas
   writes secret material outside PDDB, or logs it to UART (see
   Known gaps).
5. **Physical access with bus probing / side channels** (RAM
   readout, timing, suspend-state capture). Partially addressed:
   this is the tier the hardening backlog targets
   (docs/REFACTOR-PROPOSALS-2026-07.md §4, formerly issue #37) —
   zeroization, constant-time compares, secret wrapping. Reports
   welcome, fixes prioritized behind tier 1-4 issues.
6. **Compromised Xous kernel, malicious gateware, or a compromised
   xas binary**. Out of scope: game over by assumption. The
   project's mitigation is verifiability (reproducible builds,
   rev-pinned auditable sources — see docs/FORKS.md), not runtime
   defense.

## Trust assumptions

- `signalapp/libsignal`, `whisperfish/libsignal-service-rs`,
  `whisperfish/presage` for all Signal Protocol cryptography and
  framing. Vulnerabilities there should be reported upstream; xas
  consumes fixes via fork-pin bumps (`docs/FORKS.md`).
- `rustls` + the pinned Signal production CA for TLS.
- The Xous kernel, its services (net, PDDB, GAM), and Precursor's
  hardware key management. Kernel-side issues belong in
  `betrusted-io/xous-core`'s tracker — see their `SECURITY.md` if
  present, otherwise their issue tracker.
- The Rust toolchain pinned by `rust-toolchain.toml`.

## Scope

**In scope:** the five first-party crates (`crates/*`), the deltas
carried on the dependency fork branches (the compare URLs in
`docs/FORKS.md`), the build and
release procedure (`BUILDING.md`, `RELEASING.md`), and any way the
above weakens the upstream stack's guarantees.

**Out of scope:** vulnerabilities wholly inside upstream code
(report upstream; a heads-up here is still appreciated so the fork
pin can move), Signal protocol design questions, xous-core and
hardware issues (report to betrusted-io), and denial-of-service by
someone holding the device.

## Known gaps (accepted, tracked)

The maintainer's hardening backlog is public:
docs/REFACTOR-PROPOSALS-2026-07.md §4, formerly issue #37 (log
redaction of PII, SecretBox/zeroization of message bodies and key
material, logout wipe completeness — also #9, panic handling on the
send path, typed errors, lint tiers). A report that duplicates a
backlog item will be linked there rather than treated as new. The
in-RAM message buffer and UART logging discipline are the two areas
where the current prototype knowingly falls short of the tier-4/5
story above.

## Verification conventions

Security-sensitive crates follow the rustdoc conventions in
AGENTS.md (`# Trust boundary` / `# Security` / `# Errors` /
`# Platform constraints` sections; zeroize + constant-time
discipline). `docs/ARCHITECTURE.md` §7 holds the longer-form trust
narrative and §12 the mechanically checkable invariants.
