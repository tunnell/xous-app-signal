"""UART log access and structured parsing of the iter-1 ``perf/*`` lines.

The Pi maintains a long-running ``screen`` session against the
Precursor's UART that writes to ``~/uart-logs/precursor-uart.log``.
:func:`read_uart` snapshots that log; :func:`tail_uart` streams it.
:func:`parse_uart_perf` walks captured text and extracts the
key=value tracing lines emitted by the iter-1 instrumentation
(``perf/net``, ``perf/store``, ``perf/cold-send``, ``perf/send``).
"""

from __future__ import annotations

import re
import shlex
import subprocess
from collections.abc import Callable, Iterator
from dataclasses import dataclass, field
from typing import Any

from .config import Config, load_config
from .ssh import filter_pq_warning, ssh_pi

__all__ = [
    "PerfEntry",
    "read_uart",
    "tail_uart",
    "parse_uart_perf",
]


# The instrumented build emits lines like:
#   "perf/net: http_req exit method=GET url=https://x status=200 total_ms=59"
# A standard tracing/log prefix may sit ahead of "perf/...":
#   "2026-05-14T08:32:11.234Z  INFO xous_net_bridge::http: perf/net: ..."
# We match `perf/<topic>:` anywhere on the line and split out the rest.
_PERF_LINE = re.compile(
    r"perf/(?P<topic>[A-Za-z0-9_-]+):\s+(?P<payload>.+?)\s*$"
)
# Extract key=value pairs from the payload. Values can be:
#   - bare token   (status=200, kind=Send)
#   - quoted       (url="https://x?y=1")
#   - URLs containing '=' chars  (url=https://x?y=1 — handled by lazy
#     match that stops at the next ` <ident>=` boundary)
_KV_PAIR = re.compile(
    r"(?P<k>[A-Za-z_][A-Za-z0-9_]*)\s*=\s*"
    r"(?P<v>\"[^\"]*\"|'[^']*'|[^\s]+)"
)


@dataclass
class PerfEntry:
    """One ``perf/...`` line parsed into structured form."""

    topic: str
    raw: str
    payload: str
    fields: dict[str, str] = field(default_factory=dict)
    ts: str | None = None  # leading timestamp if we could spot one

    def to_dict(self) -> dict[str, Any]:
        return {
            "topic": self.topic,
            "raw": self.raw,
            "payload": self.payload,
            "fields": self.fields,
            "ts": self.ts,
        }


def read_uart(
    *,
    config: Config | None = None,
    lines: int = 200,
    timeout_sec: int = 30,
) -> str:
    """Hardcopy the tail of the Pi's UART capture. Returns plain text."""
    cfg = config or load_config()
    host = cfg.require_pi_host()
    chk = ssh_pi(host, f"test -f {shlex.quote(cfg.pi_uart_log)}", timeout_sec=10)
    if not chk.ok:
        raise RuntimeError(
            f"UART log {cfg.pi_uart_log!r} does not exist on {host!r}. "
            f"Start the persistent capture once with:\n"
            f"  ssh {host} 'mkdir -p ~/uart-logs && screen -dmS {cfg.pi_uart_screen} "
            f"-L -Logfile {cfg.pi_uart_log} /dev/ttyAMA0 115200'"
        )
    res = ssh_pi(
        host,
        f"tail -n {int(lines)} {shlex.quote(cfg.pi_uart_log)}",
        timeout_sec=timeout_sec,
    )
    if not res.ok:
        raise RuntimeError(f"tail UART log on {host} failed: {res.stderr.strip()}")
    return res.stdout


def tail_uart(
    callback: Callable[[str], bool | None] | None = None,
    *,
    config: Config | None = None,
    until_pattern: str | None = None,
    max_lines: int | None = None,
) -> Iterator[str]:
    """Stream UART live via ``ssh host 'tail -F <log>'``.

    Yields each line as it arrives. If ``callback`` is provided, it's
    invoked per line; returning ``False`` (not just falsy) stops the
    stream. If ``until_pattern`` is provided, the stream stops once a
    line matching it is seen. If ``max_lines`` is provided, the stream
    stops after that many lines.

    The underlying ``tail -F`` is killed by closing the ssh process'
    stdin when the iterator is exhausted or the caller `break`s out.
    """
    cfg = config or load_config()
    host = cfg.require_pi_host()
    cmd = ["ssh", host, f"tail -F {shlex.quote(cfg.pi_uart_log)}"]
    proc = subprocess.Popen(
        cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
    )
    pat = re.compile(until_pattern) if until_pattern else None
    seen = 0
    try:
        assert proc.stdout is not None
        for raw in proc.stdout:
            line = filter_pq_warning(raw).rstrip("\n")
            if not line:
                continue
            yield line
            seen += 1
            if callback is not None:
                if callback(line) is False:
                    break
            if pat is not None and pat.search(line):
                break
            if max_lines is not None and seen >= max_lines:
                break
    finally:
        try:
            proc.terminate()
            proc.wait(timeout=2)
        except subprocess.TimeoutExpired:
            proc.kill()


def _extract_ts(line: str) -> str | None:
    """Pull an ISO-8601-ish timestamp from the head of a log line, if present."""
    m = re.match(
        r"^\s*(\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?Z?)",
        line,
    )
    return m.group(1) if m else None


def parse_uart_perf(
    text: str,
    *,
    prefix: str = "perf/",
    topics: list[str] | None = None,
) -> dict[str, list[dict[str, Any]]]:
    """Walk UART text, return ``{topic: [entry, ...]}`` keyed by full topic.

    ``prefix`` defaults to ``"perf/"`` so the iter-1 instrumentation
    (perf/net, perf/store, perf/cold-send, perf/send) all flow into
    one structured dump. ``topics`` is an optional allow-list — pass
    e.g. ``["net", "store"]`` (no prefix) to filter.

    Each entry is a dict ready for JSON serialization::

        {
          "topic": "net",
          "raw": "perf/net: http_req exit method=GET url=https://...",
          "payload": "http_req exit method=GET url=https://...",
          "fields": {"method": "GET", "url": "https://...", ...},
          "ts": "2026-05-14T08:32:11.234Z"  # or None
        }
    """
    allow: set[str] | None = set(topics) if topics is not None else None
    out: dict[str, list[dict[str, Any]]] = {}
    for raw in text.splitlines():
        m = _PERF_LINE.search(raw)
        if not m:
            continue
        topic = m.group("topic")
        # Only honour ``prefix`` for the substring being matched —
        # the regex already strips it, but callers passing prefix=""
        # see every match.
        full_topic = f"{prefix}{topic}" if prefix == "perf/" else f"{prefix}{topic}"
        if allow is not None and topic not in allow:
            continue
        payload = m.group("payload")
        fields: dict[str, str] = {}
        for kv in _KV_PAIR.finditer(payload):
            v = kv.group("v")
            if (len(v) >= 2 and v[0] == v[-1] and v[0] in ("'", '"')):
                v = v[1:-1]
            fields[kv.group("k")] = v
        entry = PerfEntry(
            topic=topic,
            raw=raw,
            payload=payload,
            fields=fields,
            ts=_extract_ts(raw),
        )
        out.setdefault(full_topic, []).append(entry.to_dict())
    return out
