"""Unit tests for xas_mcp.uart.parse_uart_perf (and a small read_uart smoke).

The fixture log below is *synthetic* — it mirrors the iter-1
instrumentation format documented in commit 4a7325b without
embedding any real run's identifiers (no Signal UUIDs, no service
hostnames specific to one operator).
"""

from __future__ import annotations

import subprocess
from pathlib import Path
from typing import Any
from unittest.mock import MagicMock

import pytest

from xas_mcp import ssh as ssh_mod
from xas_mcp import uart as uart_mod
from xas_mcp.config import load_config


SAMPLE_UART = """\
2026-05-14T08:32:11.001Z  INFO xous_signal_worker: starting main loop
2026-05-14T08:32:14.123Z  INFO xous_net_bridge::http: perf/net: http_req entry method=GET url=https://example.invalid/v1/profile/sample body_len=0
2026-05-14T08:32:14.234Z  INFO xous_net_bridge::http: perf/net: http_req exit method=GET url=https://example.invalid/v1/profile/sample req_body_len=0 status=200 resp_body_len=128 tls_ms=45 write_ms=2 read_ms=12 total_ms=59
2026-05-14T08:32:14.300Z  INFO presage_store_pddb::backend_pddb: perf/store: PddbBackend::put dict=signal-state key=registration len=512 ms=8
2026-05-14T08:32:14.301Z  INFO presage_store_pddb::buffering_backend: perf/store: BufferingBackend::commit n_entries=3 puts=2 deletes=1 ms=11
2026-05-14T08:32:14.305Z  INFO xous_signal_worker: perf/cold-send: START ts=1715712734 body_len=42 attempt=1
2026-05-14T08:32:14.501Z  INFO xous_signal_worker: perf/send: batch_scope_enter ts=1715712734 attempt=1 buffered=0
2026-05-14T08:32:14.620Z  INFO xous_signal_worker: perf/send: manager.send_message returned pipeline_ms=119 result="ok"
2026-05-14T08:32:14.622Z  INFO xous_signal_worker: perf/send: batch_scope_commit ts=1715712734 attempt=1 buffered_at_commit=2 commit_ms=12 flush_sessions_ms=4
2026-05-14T08:32:14.700Z  INFO xous_signal_worker: perf/cold-send: END ts=1715712734 handle_send_total_ms=395
2026-05-14T08:32:15.000Z  INFO xas/gam_app: chat message rendered (no perf data here)
"""


def test_parse_uart_perf_groups_by_topic() -> None:
    got = uart_mod.parse_uart_perf(SAMPLE_UART)
    # Topics seen: perf/net, perf/store, perf/cold-send, perf/send.
    assert set(got.keys()) == {"perf/net", "perf/store", "perf/cold-send", "perf/send"}
    assert len(got["perf/net"]) == 2  # entry + exit
    assert len(got["perf/store"]) == 2  # put + commit
    assert len(got["perf/cold-send"]) == 2  # START + END
    assert len(got["perf/send"]) == 3  # enter, returned, commit


def test_parse_uart_perf_field_extraction() -> None:
    got = uart_mod.parse_uart_perf(SAMPLE_UART)
    http_exit = got["perf/net"][1]
    assert http_exit["fields"]["status"] == "200"
    assert http_exit["fields"]["total_ms"] == "59"
    assert http_exit["fields"]["url"].startswith("https://example.invalid/")
    # The leading timestamp should be lifted into `ts`.
    assert http_exit["ts"] == "2026-05-14T08:32:14.234Z"


def test_parse_uart_perf_quoted_values_unquoted() -> None:
    got = uart_mod.parse_uart_perf(SAMPLE_UART)
    msg = got["perf/send"][1]
    assert msg["fields"]["result"] == "ok"  # double quotes stripped


def test_parse_uart_perf_filters_by_topic_allowlist() -> None:
    got = uart_mod.parse_uart_perf(SAMPLE_UART, topics=["net", "store"])
    assert set(got.keys()) == {"perf/net", "perf/store"}


def test_parse_uart_perf_no_matches_returns_empty() -> None:
    text = "2026-05-14T08:32:11.001Z  INFO xous_signal_worker: starting main loop\n"
    assert uart_mod.parse_uart_perf(text) == {}


def test_parse_uart_perf_raw_preserved() -> None:
    got = uart_mod.parse_uart_perf(SAMPLE_UART)
    raw = got["perf/cold-send"][0]["raw"]
    assert raw.startswith("2026-05-14T")
    assert "perf/cold-send: START" in raw


def test_parse_uart_perf_handles_free_form_payload() -> None:
    """Lines with no key=value pairs still register (empty fields)."""
    text = "perf/foo: descriptive line with no equals signs at all\n"
    got = uart_mod.parse_uart_perf(text)
    assert "perf/foo" in got
    assert got["perf/foo"][0]["fields"] == {}
    assert got["perf/foo"][0]["payload"].startswith("descriptive line")


def _stub(rc: int = 0, stdout: str = "", stderr: str = "") -> MagicMock:
    cp = MagicMock(spec=subprocess.CompletedProcess)
    cp.returncode = rc
    cp.stdout = stdout
    cp.stderr = stderr
    return cp


def test_read_uart_argv_and_returns_text(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    calls: list[list[str]] = []

    def fake_run(argv: list[str], **_kwargs: Any) -> subprocess.CompletedProcess[str]:
        calls.append(list(argv))
        joined = " ".join(argv)
        if "test -f" in joined:
            return _stub(0)
        if "tail -n" in joined:
            return _stub(0, stdout="line 1\nline 2\n")
        return _stub(0)

    monkeypatch.setattr(ssh_mod.subprocess, "run", fake_run)
    cfg = load_config(
        env={"PI_HOST": "pi@h", "PI_UART_LOG": "/var/log/uart.log"},
        dotenv_path=tmp_path / "n",
        repo_root=tmp_path,
    )
    text = uart_mod.read_uart(config=cfg, lines=50)
    # filter_pq_warning() rejoins without a trailing newline; the body is preserved.
    assert text.splitlines() == ["line 1", "line 2"]
    assert any("tail -n 50" in " ".join(c) for c in calls)
    assert any("/var/log/uart.log" in " ".join(c) for c in calls)


def test_read_uart_raises_when_log_missing(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    def fake_run(argv: list[str], **_kwargs: Any) -> subprocess.CompletedProcess[str]:
        return _stub(rc=1 if "test -f" in " ".join(argv) else 0)

    monkeypatch.setattr(ssh_mod.subprocess, "run", fake_run)
    cfg = load_config(env={"PI_HOST": "pi@h"}, dotenv_path=tmp_path / "n", repo_root=tmp_path)
    with pytest.raises(RuntimeError, match="does not exist"):
        uart_mod.read_uart(config=cfg)
