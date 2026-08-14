#!/usr/bin/env python3
"""Summarize wisp JSON transfer telemetry without third-party dependencies."""

from __future__ import annotations

import argparse
import json
import math
import statistics
import sys
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path
from typing import TextIO


TELEMETRY_TARGET = "wisp_transfer_telemetry"
KNOWN_PATHS = frozenset({"aoa", "direct", "relay", "custom", "unknown"})
KNOWN_CONGESTION_CONTROLLERS = frozenset({"cubic", "bbr", "unknown"})
KNOWN_BUILD_PROFILES = frozenset({"debug", "release", "unknown"})
KNOWN_CONFIG_SOURCES = frozenset(
    {"measured", "configured", "assumed_upstream_default", "unknown"}
)
KNOWN_PHASE_ROLES = frozenset({"sender", "receiver"})
KNOWN_PHASES = frozenset(
    {
        "prepare_total",
        "walk_metadata",
        "import_hash",
        "collection_store",
        "saf_read_copy",
        "dial",
        "control_handshake",
        "decision_wait",
        "blob_setup",
        "fetch_store",
        "export",
        "background_save",
        "final_ack",
    }
)
KNOWN_PHASE_OUTCOMES = frozenset({"complete", "failed", "cancelled", "skipped"})
DEFAULT_MAX_INPUT_MIB = 512
DEFAULT_MAX_LINE_CHARS = 1024 * 1024
DEFAULT_MAX_SAMPLES = 1_000_000
DEFAULT_MAX_TRANSFERS = 10_000
THROUGHPUT_WINDOW_MS = 1_000
MIN_STABILITY_WINDOWS = 3
# ACK/control traffic is far smaller than this. Sustained receive traffic above
# the threshold while application bytes remain flat means the transport is
# active, so the episode is a delivery/HOL gap rather than a network-idle stall.
UDP_ACTIVE_RATE_THRESHOLD_BYTES_PER_SEC = 64 * 1024
MAX_REPORTED_STALL_EPISODES = 100
MAX_PROVIDER_TIMELINE_SKEW_MS = 1_000
MAX_U64 = (1 << 64) - 1


class AnalysisError(Exception):
    """Raised when telemetry input is unusable or exceeds configured limits."""


@dataclass(frozen=True)
class Sample:
    sample_ms: int
    bytes_per_sec: int
    bytes_total: int
    bytes_delta: int
    elapsed_ms: int
    warmup: bool
    first_byte_seen: bool
    time_to_first_byte_ms: int
    stalled_for_ms: int
    terminal_sample: bool
    path: str
    application_path: str
    path_stats_available: bool
    path_counter_discontinuity: bool
    rtt_us: int
    local_cwnd_bytes: int
    udp_rx_bytes_delta: int
    local_lost_packets_delta: int
    local_lost_bytes_delta: int
    local_congestion_events_delta: int
    current_mtu: int
    local_plpmtud_probe_loss_delta: int
    # Connection-scoped counters (schema v6). Absent in older logs, where they
    # read as zero/False and must not be mistaken for measured zeros.
    connection_stats_available: bool = False
    connection_udp_rx_bytes_delta: int = 0
    connection_udp_tx_bytes_delta: int = 0
    stream_data_blocked_rx_delta: int = 0
    stream_data_blocked_tx_delta: int = 0
    path_count: int = 0
    active_path_count: int = 0
    all_paths_udp_rx_bytes_delta: int = 0
    all_paths_lost_packets_delta: int = 0
    direct_path_udp_bytes_delta: int = 0
    relay_path_udp_bytes_delta: int = 0
    aoa_path_udp_bytes_delta: int = 0


@dataclass(frozen=True)
class TransferSummary:
    outcome: str
    elapsed_ms: int
    bytes_total: int
    average_bytes_per_sec: int
    first_byte_seen: bool
    time_to_first_byte_ms: int
    warmup_ms: int
    finalization_pause_ms: int
    stall_count: int
    stall_total_ms: int
    longest_stall_ms: int
    path: str
    path_udp_rx_bytes_total: int = 0
    connection_udp_rx_bytes_total: int = 0
    connection_samples_without_stats: int = 0
    stream_data_blocked_rx_total: int = 0


@dataclass(frozen=True)
class TransferConfig:
    known: bool
    source: str
    stream_receive_window_bytes: int
    connection_receive_window_bytes: int
    send_window_bytes: int
    congestion_controller: str
    build_profile: str
    sample_interval_ms: int
    stall_threshold_ms: int


@dataclass(frozen=True)
class ProviderSample:
    sample_ms: int
    elapsed_ms: int
    terminal_sample: bool
    path: str
    path_stats_available: bool
    path_counter_discontinuity: bool
    rtt_us: int
    cwnd_bytes: int
    udp_tx_bytes_delta: int
    udp_rx_bytes_delta: int
    lost_packets_delta: int
    lost_bytes_delta: int
    congestion_events_delta: int
    current_mtu: int
    plpmtud_probe_loss_delta: int
    connection_stats_available: bool = False
    connection_udp_tx_bytes_delta: int = 0
    connection_udp_rx_bytes_delta: int = 0
    stream_data_blocked_tx_delta: int = 0
    path_count: int = 0
    active_path_count: int = 0
    all_paths_udp_tx_bytes_delta: int = 0
    all_paths_lost_packets_delta: int = 0
    direct_path_udp_bytes_delta: int = 0
    relay_path_udp_bytes_delta: int = 0
    aoa_path_udp_bytes_delta: int = 0


@dataclass(frozen=True)
class ProviderSummary:
    outcome: str
    elapsed_ms: int
    udp_tx_bytes_total: int
    lost_packets_total: int
    lost_bytes_total: int
    congestion_events_total: int
    path_counter_discontinuity_count: int
    path: str
    connection_udp_tx_bytes_total: int = 0
    connection_udp_rx_bytes_total: int = 0
    connection_samples_without_stats: int = 0
    stream_data_blocked_tx_total: int = 0


@dataclass(frozen=True)
class PhaseEvent:
    role: str
    benchmark_run_id: int | None
    phase: str
    outcome: str
    elapsed_ms: int
    bytes_total: int
    file_count: int


@dataclass
class TransferEvents:
    samples: list[Sample] = field(default_factory=list)
    summary: TransferSummary | None = None
    config: TransferConfig | None = None
    benchmark_run_id: int | None = None


@dataclass
class ProviderEvents:
    samples: list[ProviderSample] = field(default_factory=list)
    summary: ProviderSummary | None = None
    config: TransferConfig | None = None
    benchmark_run_id: int | None = None


@dataclass
class ParsedTelemetry:
    transfers: dict[tuple[int, int], TransferEvents] = field(default_factory=dict)
    providers: dict[tuple[int, int], ProviderEvents] = field(default_factory=dict)
    phases: list[tuple[int, PhaseEvent]] = field(default_factory=list)
    skipped_lines: int = 0
    malformed_telemetry_lines: int = 0
    sample_count: int = 0
    phase_count: int = 0


def _event_group(
    parsed: ParsedTelemetry,
    role: str,
    key: tuple[int, int],
    max_transfers: int,
) -> TransferEvents | ProviderEvents:
    groups = parsed.transfers if role == "receiver" else parsed.providers
    if key not in groups:
        if len(parsed.transfers) + len(parsed.providers) >= max_transfers:
            raise AnalysisError(f"transfer limit exceeded ({max_transfers:,})")
        groups[key] = TransferEvents() if role == "receiver" else ProviderEvents()
    return groups[key]


def _optional_count(fields: dict[str, object], name: str) -> int:
    """Read a counter that only exists from schema v6 onwards.

    Older logs simply lack the field. Treat a missing or malformed value as
    zero rather than rejecting the whole record: the connection-scoped counters
    are additive diagnostics, and dropping historical samples over them would
    lose throughput and stall data that is still perfectly valid.
    """

    value = _non_negative_int(fields.get(name, 0))
    return value if value is not None else 0


def _non_negative_int(value: object) -> int | None:
    if (
        isinstance(value, bool)
        or not isinstance(value, int)
        or value < 0
        or value > MAX_U64
    ):
        return None
    return value


def _non_negative_int_or_decimal(value: object) -> int | None:
    # Dart cannot represent every Rust u64 as a native signed integer. Mobile
    # phase JSONL therefore uses bounded, canonical decimal strings for run IDs
    # and byte counters. Keep every other event on strict JSON integers.
    if isinstance(value, str):
        if (
            not value
            or len(value) > 20
            or not value.isascii()
            or not value.isdigit()
            or (len(value) > 1 and value[0] == "0")
        ):
            return None
        value = int(value, 10)
    return _non_negative_int(value)


def _known_path(value: object) -> str:
    return value if isinstance(value, str) and value in KNOWN_PATHS else "unknown"


def _known_label(value: object, allowed: frozenset[str]) -> str:
    return value if isinstance(value, str) and value in allowed else "unknown"


def _strict_bool(value: object) -> bool | None:
    return value if isinstance(value, bool) else None


def _benchmark_run_id(
    fields: dict[str, object], *, allow_decimal_string: bool = False
) -> int | None:
    available = fields.get("benchmark_run_id_available")
    parser = _non_negative_int_or_decimal if allow_decimal_string else _non_negative_int
    value = parser(fields.get("benchmark_run_id"))
    return value if available is True and value is not None else None


def _parse_sample(fields: dict[str, object]) -> tuple[int, Sample] | None:
    if fields.get("role") != "receiver":
        return None
    transfer_id = _non_negative_int(fields.get("transfer_id"))
    sample_ms = _non_negative_int(fields.get("sample_ms"))
    rate = _non_negative_int(fields.get("app_bytes_per_sec"))
    bytes_total = _non_negative_int(fields.get("bytes_total"))
    bytes_delta = _non_negative_int(fields.get("bytes_delta"))
    elapsed_ms = _non_negative_int(fields.get("elapsed_ms"))
    warmup = _strict_bool(fields.get("warmup"))
    first_byte_seen = _strict_bool(fields.get("first_byte_seen"))
    time_to_first_byte_ms = _non_negative_int(fields.get("time_to_first_byte_ms"))
    stalled_for_ms = _non_negative_int(fields.get("stalled_for_ms"))
    terminal_sample = _strict_bool(fields.get("terminal_sample"))
    path_counter_discontinuity = _strict_bool(
        fields.get("path_counter_discontinuity", False)
    )
    rtt_us = _non_negative_int(fields.get("rtt_us"))
    local_cwnd_bytes = _non_negative_int(fields.get("local_cwnd_bytes"))
    udp_rx_bytes_delta = _non_negative_int(fields.get("udp_rx_bytes_delta"))
    local_lost_packets_delta = _non_negative_int(fields.get("local_lost_packets_delta"))
    local_lost_bytes_delta = _non_negative_int(fields.get("local_lost_bytes_delta"))
    local_congestion_events_delta = _non_negative_int(
        fields.get("local_congestion_events_delta")
    )
    current_mtu = _non_negative_int(fields.get("current_mtu"))
    local_plpmtud_probe_loss_delta = _non_negative_int(
        fields.get("local_plpmtud_probe_loss_delta")
    )
    if None in (
        transfer_id,
        sample_ms,
        rate,
        bytes_total,
        bytes_delta,
        elapsed_ms,
        warmup,
        first_byte_seen,
        time_to_first_byte_ms,
        stalled_for_ms,
        terminal_sample,
        path_counter_discontinuity,
        rtt_us,
        local_cwnd_bytes,
        udp_rx_bytes_delta,
        local_lost_packets_delta,
        local_lost_bytes_delta,
        local_congestion_events_delta,
        current_mtu,
        local_plpmtud_probe_loss_delta,
    ):
        return None
    return transfer_id, Sample(
        sample_ms=sample_ms,
        bytes_per_sec=rate,
        bytes_total=bytes_total,
        bytes_delta=bytes_delta,
        elapsed_ms=elapsed_ms,
        warmup=warmup,
        first_byte_seen=first_byte_seen,
        time_to_first_byte_ms=time_to_first_byte_ms,
        stalled_for_ms=stalled_for_ms,
        terminal_sample=terminal_sample,
        path=_known_path(fields.get("path")),
        application_path=_known_path(fields.get("application_path")),
        path_stats_available=fields.get("path_stats_available") is True,
        path_counter_discontinuity=path_counter_discontinuity,
        rtt_us=rtt_us,
        local_cwnd_bytes=local_cwnd_bytes,
        udp_rx_bytes_delta=udp_rx_bytes_delta,
        local_lost_packets_delta=local_lost_packets_delta,
        local_lost_bytes_delta=local_lost_bytes_delta,
        local_congestion_events_delta=local_congestion_events_delta,
        current_mtu=current_mtu,
        local_plpmtud_probe_loss_delta=local_plpmtud_probe_loss_delta,
        connection_stats_available=fields.get("connection_stats_available") is True,
        connection_udp_rx_bytes_delta=_optional_count(
            fields, "connection_udp_rx_bytes_delta"
        ),
        connection_udp_tx_bytes_delta=_optional_count(
            fields, "connection_udp_tx_bytes_delta"
        ),
        stream_data_blocked_rx_delta=_optional_count(
            fields, "stream_data_blocked_rx_delta"
        ),
        stream_data_blocked_tx_delta=_optional_count(
            fields, "stream_data_blocked_tx_delta"
        ),
        path_count=_optional_count(fields, "path_count"),
        all_paths_udp_rx_bytes_delta=_optional_count(
            fields, "all_paths_udp_rx_bytes_delta"
        ),
        active_path_count=_optional_count(fields, "active_path_count"),
        all_paths_lost_packets_delta=_optional_count(
            fields, "all_paths_lost_packets_delta"
        ),
        direct_path_udp_bytes_delta=_optional_count(
            fields, "direct_path_udp_bytes_delta"
        ),
        relay_path_udp_bytes_delta=_optional_count(
            fields, "relay_path_udp_bytes_delta"
        ),
        aoa_path_udp_bytes_delta=_optional_count(fields, "aoa_path_udp_bytes_delta"),
    )


def _parse_summary(fields: dict[str, object]) -> tuple[int, TransferSummary] | None:
    if fields.get("role") != "receiver":
        return None
    transfer_id = _non_negative_int(fields.get("transfer_id"))
    elapsed_ms = _non_negative_int(fields.get("elapsed_ms"))
    bytes_total = _non_negative_int(fields.get("bytes_total"))
    average = _non_negative_int(fields.get("average_bytes_per_sec"))
    first_byte_seen = _strict_bool(fields.get("first_byte_seen"))
    time_to_first_byte_ms = _non_negative_int(fields.get("time_to_first_byte_ms"))
    warmup_ms = _non_negative_int(fields.get("warmup_ms"))
    finalization_pause_ms = _non_negative_int(fields.get("finalization_pause_ms"))
    stall_count = _non_negative_int(fields.get("stall_count"))
    stall_total_ms = _non_negative_int(fields.get("stall_total_ms"))
    longest_stall_ms = _non_negative_int(fields.get("longest_stall_ms"))
    if None in (
        transfer_id,
        elapsed_ms,
        bytes_total,
        average,
        first_byte_seen,
        time_to_first_byte_ms,
        warmup_ms,
        finalization_pause_ms,
        stall_count,
        stall_total_ms,
        longest_stall_ms,
    ):
        return None
    outcome = fields.get("outcome")
    if outcome not in {"complete", "failed"}:
        outcome = "unknown"
    return transfer_id, TransferSummary(
        outcome=outcome,
        elapsed_ms=elapsed_ms,
        bytes_total=bytes_total,
        average_bytes_per_sec=average,
        first_byte_seen=first_byte_seen,
        time_to_first_byte_ms=time_to_first_byte_ms,
        warmup_ms=warmup_ms,
        finalization_pause_ms=finalization_pause_ms,
        stall_count=stall_count,
        stall_total_ms=stall_total_ms,
        longest_stall_ms=longest_stall_ms,
        path=_known_path(fields.get("path")),
        path_udp_rx_bytes_total=_optional_count(fields, "path_udp_rx_bytes_total"),
        connection_udp_rx_bytes_total=_optional_count(
            fields, "connection_udp_rx_bytes_total"
        ),
        connection_samples_without_stats=_optional_count(
            fields, "connection_samples_without_stats"
        ),
        stream_data_blocked_rx_total=_optional_count(
            fields, "stream_data_blocked_rx_total"
        ),
    )


def _parse_config(
    fields: dict[str, object], expected_role: str
) -> tuple[int, TransferConfig] | None:
    if fields.get("role") != expected_role:
        return None
    transfer_id = _non_negative_int(fields.get("transfer_id"))
    known = _strict_bool(fields.get("config_known"))
    stream_window_field = (
        "local_stream_receive_window_bytes"
        if expected_role == "provider"
        else "stream_receive_window_bytes"
    )
    connection_window_field = (
        "local_connection_receive_window_bytes"
        if expected_role == "provider"
        else "connection_receive_window_bytes"
    )
    # Schema <= 4 used receiver-style names for provider-local windows. Accept
    # those historical logs, but all newly emitted provider records are explicit.
    stream_window = _non_negative_int(
        fields.get(stream_window_field, fields.get("stream_receive_window_bytes"))
    )
    connection_window = _non_negative_int(
        fields.get(
            connection_window_field,
            fields.get("connection_receive_window_bytes"),
        )
    )
    send_window = _non_negative_int(fields.get("send_window_bytes"))
    sample_interval_ms = _non_negative_int(fields.get("sample_interval_ms"))
    stall_threshold_ms = _non_negative_int(fields.get("stall_threshold_ms"))
    if None in (
        transfer_id,
        known,
        stream_window,
        connection_window,
        send_window,
        sample_interval_ms,
        stall_threshold_ms,
    ):
        return None
    source_value = fields.get("config_source")
    if source_value is None:
        source_value = "configured" if known else "unknown"
    return transfer_id, TransferConfig(
        known=known,
        source=_known_label(source_value, KNOWN_CONFIG_SOURCES),
        stream_receive_window_bytes=stream_window,
        connection_receive_window_bytes=connection_window,
        send_window_bytes=send_window,
        congestion_controller=_known_label(
            fields.get("congestion_controller"), KNOWN_CONGESTION_CONTROLLERS
        ),
        build_profile=_known_label(fields.get("build_profile"), KNOWN_BUILD_PROFILES),
        sample_interval_ms=sample_interval_ms,
        stall_threshold_ms=stall_threshold_ms,
    )


def _parse_provider_sample(
    fields: dict[str, object],
) -> tuple[int, ProviderSample] | None:
    if fields.get("role") != "provider":
        return None
    names = (
        "transfer_id",
        "sample_ms",
        "elapsed_ms",
        "rtt_us",
        "cwnd_bytes",
        "udp_tx_bytes_delta",
        "udp_rx_bytes_delta",
        "lost_packets_delta",
        "lost_bytes_delta",
        "congestion_events_delta",
        "current_mtu",
        "plpmtud_probe_loss_delta",
    )
    values = {name: _non_negative_int(fields.get(name)) for name in names}
    terminal_sample = _strict_bool(fields.get("terminal_sample"))
    path_counter_discontinuity = _strict_bool(
        fields.get("path_counter_discontinuity", False)
    )
    if (
        terminal_sample is None
        or path_counter_discontinuity is None
        or any(value is None for value in values.values())
    ):
        return None
    transfer_id = values.pop("transfer_id")
    assert transfer_id is not None
    return transfer_id, ProviderSample(
        sample_ms=values["sample_ms"],
        elapsed_ms=values["elapsed_ms"],
        terminal_sample=terminal_sample,
        path=_known_path(fields.get("path")),
        path_stats_available=fields.get("path_stats_available") is True,
        path_counter_discontinuity=path_counter_discontinuity,
        rtt_us=values["rtt_us"],
        cwnd_bytes=values["cwnd_bytes"],
        udp_tx_bytes_delta=values["udp_tx_bytes_delta"],
        udp_rx_bytes_delta=values["udp_rx_bytes_delta"],
        lost_packets_delta=values["lost_packets_delta"],
        lost_bytes_delta=values["lost_bytes_delta"],
        congestion_events_delta=values["congestion_events_delta"],
        current_mtu=values["current_mtu"],
        plpmtud_probe_loss_delta=values["plpmtud_probe_loss_delta"],
        connection_stats_available=fields.get("connection_stats_available") is True,
        connection_udp_tx_bytes_delta=_optional_count(
            fields, "connection_udp_tx_bytes_delta"
        ),
        connection_udp_rx_bytes_delta=_optional_count(
            fields, "connection_udp_rx_bytes_delta"
        ),
        stream_data_blocked_tx_delta=_optional_count(
            fields, "stream_data_blocked_tx_delta"
        ),
        path_count=_optional_count(fields, "path_count"),
        all_paths_udp_tx_bytes_delta=_optional_count(
            fields, "all_paths_udp_tx_bytes_delta"
        ),
        active_path_count=_optional_count(fields, "active_path_count"),
        all_paths_lost_packets_delta=_optional_count(
            fields, "all_paths_lost_packets_delta"
        ),
        direct_path_udp_bytes_delta=_optional_count(
            fields, "direct_path_udp_bytes_delta"
        ),
        relay_path_udp_bytes_delta=_optional_count(
            fields, "relay_path_udp_bytes_delta"
        ),
        aoa_path_udp_bytes_delta=_optional_count(fields, "aoa_path_udp_bytes_delta"),
    )


def _parse_provider_summary(
    fields: dict[str, object],
) -> tuple[int, ProviderSummary] | None:
    if fields.get("role") != "provider":
        return None
    names = (
        "transfer_id",
        "elapsed_ms",
        "udp_tx_bytes_total",
        "lost_packets_total",
        "lost_bytes_total",
        "congestion_events_total",
        "path_counter_discontinuity_count",
    )
    values = {
        name: _non_negative_int(
            fields.get(name, 0 if name == "path_counter_discontinuity_count" else None)
        )
        for name in names
    }
    if any(value is None for value in values.values()):
        return None
    outcome = fields.get("outcome")
    if outcome not in {"complete", "failed"}:
        outcome = "unknown"
    transfer_id = values.pop("transfer_id")
    assert transfer_id is not None
    return transfer_id, ProviderSummary(
        outcome=outcome,
        elapsed_ms=values["elapsed_ms"],
        udp_tx_bytes_total=values["udp_tx_bytes_total"],
        lost_packets_total=values["lost_packets_total"],
        lost_bytes_total=values["lost_bytes_total"],
        congestion_events_total=values["congestion_events_total"],
        path_counter_discontinuity_count=values[
            "path_counter_discontinuity_count"
        ],
        path=_known_path(fields.get("path")),
        connection_udp_tx_bytes_total=_optional_count(
            fields, "connection_udp_tx_bytes_total"
        ),
        connection_udp_rx_bytes_total=_optional_count(
            fields, "connection_udp_rx_bytes_total"
        ),
        connection_samples_without_stats=_optional_count(
            fields, "connection_samples_without_stats"
        ),
        stream_data_blocked_tx_total=_optional_count(
            fields, "stream_data_blocked_tx_total"
        ),
    )


def _parse_phase(fields: dict[str, object]) -> PhaseEvent | None:
    role = _known_label(fields.get("role"), KNOWN_PHASE_ROLES)
    phase = _known_label(fields.get("phase"), KNOWN_PHASES)
    outcome = _known_label(fields.get("outcome"), KNOWN_PHASE_OUTCOMES)
    benchmark_run_id = _benchmark_run_id(fields, allow_decimal_string=True)
    elapsed_ms = _non_negative_int(fields.get("elapsed_ms"))
    bytes_total = _non_negative_int_or_decimal(fields.get("bytes_total"))
    file_count = _non_negative_int(fields.get("file_count"))
    if (
        role == "unknown"
        or phase == "unknown"
        or outcome == "unknown"
        or (
            fields.get("benchmark_run_id_available") is True
            and benchmark_run_id is None
        )
        or elapsed_ms is None
        or bytes_total is None
        or file_count is None
    ):
        return None
    return PhaseEvent(
        role=role,
        benchmark_run_id=benchmark_run_id,
        phase=phase,
        outcome=outcome,
        elapsed_ms=elapsed_ms,
        bytes_total=bytes_total,
        file_count=file_count,
    )


def parse_stream(
    stream: TextIO,
    parsed: ParsedTelemetry | None = None,
    *,
    source_id: int = 0,
    max_line_chars: int = DEFAULT_MAX_LINE_CHARS,
    max_samples: int = DEFAULT_MAX_SAMPLES,
    max_transfers: int = DEFAULT_MAX_TRANSFERS,
    max_input_chars: int = DEFAULT_MAX_INPUT_MIB * 1024 * 1024,
) -> ParsedTelemetry:
    """Parse one JSONL stream, skipping non-telemetry output and bad lines."""
    if (
        max_line_chars < 1
        or max_samples < 1
        or max_transfers < 1
        or max_input_chars < 1
    ):
        raise ValueError("parser limits must be positive")
    result = parsed or ParsedTelemetry()
    input_chars = 0

    while True:
        line = stream.readline(max_line_chars + 1)
        if not line:
            break
        input_chars += len(line)
        if input_chars > max_input_chars:
            raise AnalysisError("input stream exceeds configured size limit")
        if len(line) > max_line_chars:
            while line and not line.endswith("\n"):
                line = stream.readline(max_line_chars + 1)
                input_chars += len(line)
                if input_chars > max_input_chars:
                    raise AnalysisError("input stream exceeds configured size limit")
            result.skipped_lines += 1
            continue
        try:
            entry = json.loads(line)
        except (ValueError, RecursionError, TypeError):
            result.skipped_lines += 1
            continue
        if not isinstance(entry, dict) or entry.get("target") != TELEMETRY_TARGET:
            result.skipped_lines += 1
            continue
        fields = entry.get("fields")
        if not isinstance(fields, dict):
            result.malformed_telemetry_lines += 1
            continue

        event = fields.get("event")
        role = fields.get("role")
        if event == "blob_phase":
            phase = _parse_phase(fields)
            if phase is None:
                result.malformed_telemetry_lines += 1
                continue
            if result.sample_count + result.phase_count >= max_samples:
                raise AnalysisError(f"telemetry event limit exceeded ({max_samples:,})")
            result.phases.append((source_id, phase))
            result.phase_count += 1
        elif event == "blob_config":
            config = _parse_config(fields, role) if role in {"receiver", "provider"} else None
            if config is None:
                result.malformed_telemetry_lines += 1
                continue
            transfer_id, value = config
            events = _event_group(
                result, role, (source_id, transfer_id), max_transfers
            )
            events.config = value
            run_id = _benchmark_run_id(fields)
            if run_id is not None:
                events.benchmark_run_id = run_id
        elif event == "blob_sample":
            sample = (
                _parse_sample(fields)
                if role == "receiver"
                else _parse_provider_sample(fields) if role == "provider" else None
            )
            if sample is None:
                result.malformed_telemetry_lines += 1
                continue
            if result.sample_count + result.phase_count >= max_samples:
                raise AnalysisError(f"telemetry event limit exceeded ({max_samples:,})")
            transfer_id, value = sample
            key = (source_id, transfer_id)
            events = _event_group(result, role, key, max_transfers)
            events.samples.append(value)
            run_id = _benchmark_run_id(fields)
            if run_id is not None:
                events.benchmark_run_id = run_id
            result.sample_count += 1
        elif event == "blob_summary":
            summary = (
                _parse_summary(fields)
                if role == "receiver"
                else _parse_provider_summary(fields) if role == "provider" else None
            )
            if summary is None:
                result.malformed_telemetry_lines += 1
                continue
            transfer_id, value = summary
            events = _event_group(
                result, role, (source_id, transfer_id), max_transfers
            )
            events.summary = value
            run_id = _benchmark_run_id(fields)
            if run_id is not None:
                events.benchmark_run_id = run_id
        else:
            result.skipped_lines += 1
    return result


def percentile(values: list[int | float], fraction: float) -> float:
    """Return a linearly-interpolated percentile for 0 <= fraction <= 1."""
    if not values:
        raise ValueError("percentile requires at least one value")
    if not 0.0 <= fraction <= 1.0:
        raise ValueError("percentile fraction must be between zero and one")
    ordered = sorted(values)
    position = (len(ordered) - 1) * fraction
    lower = math.floor(position)
    upper = math.ceil(position)
    if lower == upper:
        return float(ordered[lower])
    weight = position - lower
    return ordered[lower] * (1.0 - weight) + ordered[upper] * weight


def _low_speed_episodes(values: list[int], threshold: float) -> int:
    if threshold <= 0:
        return 0
    episodes = 0
    was_low = False
    for value in values:
        is_low = value < threshold
        if is_low and not was_low:
            episodes += 1
        was_low = is_low
    return episodes


def _observed_stalls(samples: list[Sample]) -> tuple[int, int]:
    episodes = 0
    longest_ms = 0
    was_stalled = False
    for sample in samples:
        is_stalled = sample.stalled_for_ms >= 500
        if is_stalled and not was_stalled:
            episodes += 1
        if is_stalled:
            longest_ms = max(longest_ms, sample.stalled_for_ms)
        was_stalled = is_stalled
    return episodes, longest_ms


def _classify_stall_episodes(
    samples: list[Sample],
    *,
    provider_samples: list[ProviderSample] | None = None,
    provider_timeline_offset_ms: int | None = None,
) -> tuple[list[dict[str, object]], Counter[str], int]:
    """Classify sampled stalls without treating all flat app counters alike.

    Receiver UDP counters include protocol overhead, so this is deliberately a
    coarse diagnostic rather than a payload-throughput replacement. A bounded
    64 KiB/s threshold keeps ACK/keepalive traffic from being mistaken for an
    active bulk receive path. When a uniquely matched provider timeline is
    available, overlapping payload loss identifies a loss-recovery/HOL gap.
    """

    episodes: list[dict[str, object]] = []
    kind_counts: Counter[str] = Counter()
    episode_count = 0
    current: list[Sample] = []

    def finish_episode() -> None:
        nonlocal episode_count
        if not current:
            return
        duration_ms = max(sample.stalled_for_ms for sample in current)
        end_ms = max(sample.elapsed_ms for sample in current)
        network_samples = [sample for sample in current if sample.path_stats_available]
        receiver_path_counter_discontinuity = any(
            sample.path_counter_discontinuity for sample in current
        )
        sampled_ms = sum(sample.sample_ms for sample in network_samples)
        udp_rx_bytes = sum(sample.udp_rx_bytes_delta for sample in network_samples)
        udp_rx_rate = (
            udp_rx_bytes * 1_000.0 / sampled_ms if sampled_ms > 0 else None
        )
        start_ms = max(0, end_ms - duration_ms)
        provider_overlap = (
            [
                sample
                for sample in provider_samples
                if sample.path_stats_available
                and sample.elapsed_ms - provider_timeline_offset_ms > start_ms
                and sample.elapsed_ms
                - sample.sample_ms
                - provider_timeline_offset_ms
                < end_ms
            ]
            if provider_samples is not None
            and provider_timeline_offset_ms is not None
            else []
        )
        provider_lost_packets = sum(
            sample.lost_packets_delta for sample in provider_overlap
        )
        provider_lost_bytes = sum(sample.lost_bytes_delta for sample in provider_overlap)
        provider_congestion_events = sum(
            sample.congestion_events_delta for sample in provider_overlap
        )
        provider_path_counter_discontinuity = any(
            sample.path_counter_discontinuity for sample in provider_overlap
        )
        provider_loss_evidence_available = bool(provider_overlap)

        if receiver_path_counter_discontinuity:
            kind = "unknown"
        elif udp_rx_rate is None:
            kind = "unknown"
        elif udp_rx_rate >= UDP_ACTIVE_RATE_THRESHOLD_BYTES_PER_SEC:
            if provider_loss_evidence_available and (
                provider_lost_packets > 0 or provider_lost_bytes > 0
            ):
                kind = "transport_active_loss_recovery"
            elif provider_path_counter_discontinuity:
                # A zero delta at a path boundary means "not observable", not
                # proof that no provider-side loss occurred in this episode.
                kind = "unknown"
            else:
                kind = "transport_active_delivery_gap"
        else:
            kind = "transport_idle_stall"
        episode_count += 1
        kind_counts[kind] += 1
        if len(episodes) < MAX_REPORTED_STALL_EPISODES:
            episodes.append(
                {
                    "kind": kind,
                    "start_ms": start_ms,
                    "end_ms": end_ms,
                    "duration_ms": duration_ms,
                    "sample_count": len(current),
                    "network_sample_count": len(network_samples),
                    "receiver_path_counter_discontinuity": (
                        receiver_path_counter_discontinuity
                    ),
                    "udp_rx_bytes": udp_rx_bytes if network_samples else None,
                    "udp_rx_average_bytes_per_sec": udp_rx_rate,
                    "provider_loss_evidence_available": (
                        provider_loss_evidence_available
                    ),
                    "provider_sample_count": len(provider_overlap),
                    "provider_path_counter_discontinuity": (
                        provider_path_counter_discontinuity
                    ),
                    "provider_lost_packets": (
                        provider_lost_packets
                        if provider_loss_evidence_available
                        else None
                    ),
                    "provider_lost_bytes": (
                        provider_lost_bytes if provider_loss_evidence_available else None
                    ),
                    "provider_congestion_events": (
                        provider_congestion_events
                        if provider_loss_evidence_available
                        else None
                    ),
                }
            )

    for sample in samples:
        if sample.stalled_for_ms >= 500:
            current.append(sample)
        elif current:
            finish_episode()
            current = []
    finish_episode()
    return episodes, kind_counts, episode_count


def _provider_timeline_offset_ms(
    receiver: TransferEvents, provider: ProviderEvents
) -> int | None:
    """Align sampler elapsed times only when both terminal clocks are close."""
    if receiver.summary is None or provider.summary is None:
        return None
    offset_ms = provider.summary.elapsed_ms - receiver.summary.elapsed_ms
    return (
        offset_ms
        if abs(offset_ms) <= MAX_PROVIDER_TIMELINE_SKEW_MS
        else None
    )


def _samples_through_last_payload_progress(samples: list[Sample]) -> list[Sample]:
    """Drop the trailing finalization tail after the last payload byte delta."""
    last_payload_index = next(
        (index for index in range(len(samples) - 1, -1, -1) if samples[index].bytes_delta > 0),
        None,
    )
    return samples[: last_payload_index + 1] if last_payload_index is not None else []


def resample_throughput(
    samples: list[Sample], window_ms: int = THROUGHPUT_WINDOW_MS
) -> tuple[list[float], int, int]:
    """Return rates in non-overlapping windows, full-window count, and used ms.

    The first byte-bearing telemetry interval is marked as warm-up by the
    producer and excluded. Delayed sampler ticks are weighted by `sample_ms`;
    byte deltas that span a window boundary are split proportionally. A lone
    partial window is returned only to keep very short runs inspectable, but it
    never satisfies the stability gate because its full-window count is zero.
    """
    if window_ms < 1:
        raise ValueError("throughput window must be positive")

    steady_samples = [
        sample
        for sample in _samples_through_last_payload_progress(samples)
        if not sample.warmup and sample.sample_ms > 0
    ]
    rates: list[float] = []
    window_elapsed_ms = 0.0
    window_bytes = 0.0
    analyzed_ms = 0

    for sample in steady_samples:
        remaining_ms = float(sample.sample_ms)
        remaining_bytes = float(sample.bytes_delta)
        analyzed_ms += sample.sample_ms
        while remaining_ms > 0:
            capacity_ms = float(window_ms) - window_elapsed_ms
            take_ms = min(capacity_ms, remaining_ms)
            take_bytes = remaining_bytes * (take_ms / remaining_ms)
            window_elapsed_ms += take_ms
            window_bytes += take_bytes
            remaining_ms -= take_ms
            remaining_bytes -= take_bytes

            if window_elapsed_ms >= window_ms:
                rates.append(window_bytes * 1_000.0 / float(window_ms))
                window_elapsed_ms = 0.0
                window_bytes = 0.0

    full_window_count = len(rates)
    if not rates and window_elapsed_ms > 0:
        rates.append(window_bytes * 1_000.0 / window_elapsed_ms)
    return rates, full_window_count, analyzed_ms


def _resample_rtt(
    samples: list[Sample], window_ms: int = THROUGHPUT_WINDOW_MS
) -> list[float | None]:
    """Return time-weighted RTT values on the same windows as throughput."""
    if window_ms < 1:
        raise ValueError("throughput window must be positive")

    steady_samples = [
        sample
        for sample in _samples_through_last_payload_progress(samples)
        if not sample.warmup and sample.sample_ms > 0
    ]
    rtts: list[float | None] = []
    window_elapsed_ms = 0.0
    weighted_rtt = 0.0
    network_ms = 0.0

    for sample in steady_samples:
        remaining_ms = float(sample.sample_ms)
        while remaining_ms > 0:
            take_ms = min(float(window_ms) - window_elapsed_ms, remaining_ms)
            window_elapsed_ms += take_ms
            remaining_ms -= take_ms
            if sample.path_stats_available and sample.rtt_us > 0:
                weighted_rtt += float(sample.rtt_us) * take_ms
                network_ms += take_ms

            if window_elapsed_ms >= window_ms:
                rtts.append(weighted_rtt / network_ms if network_ms > 0 else None)
                window_elapsed_ms = 0.0
                weighted_rtt = 0.0
                network_ms = 0.0

    if not rtts and window_elapsed_ms > 0:
        rtts.append(weighted_rtt / network_ms if network_ms > 0 else None)
    return rtts


def _path_metrics(samples: list[Sample]) -> dict[str, object]:
    bytes_by_path = Counter()
    path_migrations = 0
    previous_path: str | None = None
    time_to_direct_ms: int | None = None

    for sample in samples:
        bytes_by_path[sample.application_path] += sample.bytes_delta
        if sample.path != "unknown":
            if previous_path is not None and sample.path != previous_path:
                path_migrations += 1
            previous_path = sample.path
        if sample.path == "direct" and time_to_direct_ms is None:
            time_to_direct_ms = sample.elapsed_ms

    payload_bytes = sum(bytes_by_path.values())

    def ratio(path: str) -> float | None:
        return bytes_by_path[path] / payload_bytes if payload_bytes > 0 else None

    dominant_path = (
        bytes_by_path.most_common(1)[0][0] if payload_bytes > 0 else "unknown"
    )
    return {
        "dominant_path": dominant_path,
        "path_migration_count": path_migrations,
        "time_to_direct_ms": time_to_direct_ms,
        "never_direct": time_to_direct_ms is None,
        "direct_bytes_ratio": ratio("direct"),
        "relay_bytes_ratio": ratio("relay"),
        "aoa_bytes_ratio": ratio("aoa"),
        "path_accounted_payload_bytes": payload_bytes,
    }


def _wire_path_attribution(samples: list) -> dict[str, object]:
    """Split wire bytes by the kind of path that carried them.

    This is the only measurement that can answer "how much went over the
    relay" on a multipath connection. The payload-based `relay_bytes_ratio`
    attributes application bytes to whichever path was *selected*, so a
    transfer whose payload partly rides a relay path that is never selected
    reports a relay ratio of zero. Measured on a real Wi-Fi transfer the
    selected path carried 42.6% of the wire bytes and a relay path carried
    25.7%, which the payload-based ratio reported as entirely direct.
    """

    direct = sum(sample.direct_path_udp_bytes_delta for sample in samples)
    relay = sum(sample.relay_path_udp_bytes_delta for sample in samples)
    aoa = sum(sample.aoa_path_udp_bytes_delta for sample in samples)
    total = direct + relay + aoa
    if total == 0:
        return {
            "wire_path_bytes_available": False,
            "wire_direct_bytes": None,
            "wire_relay_bytes": None,
            "wire_aoa_bytes": None,
            "wire_relay_bytes_ratio": None,
        }
    return {
        "wire_path_bytes_available": True,
        "wire_direct_bytes": direct,
        "wire_relay_bytes": relay,
        "wire_aoa_bytes": aoa,
        "wire_relay_bytes_ratio": relay / total,
    }


def summarize_transfer(
    source_id: int,
    transfer_id: int,
    events: TransferEvents,
    provider_events: ProviderEvents | None = None,
) -> dict[str, object]:
    summary = events.summary
    speeds, full_window_count, analyzed_ms = resample_throughput(events.samples)
    rtt_windows = _resample_rtt(events.samples)
    if not speeds:
        fallback_rate = (
            summary.average_bytes_per_sec
            if summary is not None
            else max((sample.bytes_per_sec for sample in events.samples), default=0)
        )
        speeds = [float(fallback_rate)]

    p10 = percentile(speeds, 0.10)
    median = percentile(speeds, 0.50)
    p90 = percentile(speeds, 0.90)
    mean = statistics.fmean(speeds)
    coefficient_of_variation = statistics.pstdev(speeds) / mean if mean > 0 else None
    p10_to_median = p10 / median if median > 0 else None
    low_threshold = median * 0.10
    payload_samples = _samples_through_last_payload_progress(events.samples)
    observed_stall_count, observed_longest_stall_ms = _observed_stalls(payload_samples)
    provider_timeline_offset_ms = (
        _provider_timeline_offset_ms(events, provider_events)
        if provider_events is not None
        else None
    )
    stall_episodes, stall_kind_counts, observed_stall_episode_count = (
        _classify_stall_episodes(
            payload_samples,
            provider_samples=(
                provider_events.samples if provider_events is not None else None
            ),
            provider_timeline_offset_ms=provider_timeline_offset_ms,
        )
    )
    path_metrics = _path_metrics(events.samples)

    final_path = summary.path if summary is not None else events.samples[-1].path
    path = str(path_metrics["dominant_path"])
    if path == "unknown":
        path = final_path
    stall_count = summary.stall_count if summary is not None else observed_stall_count
    longest_stall_ms = (
        summary.longest_stall_ms if summary is not None else observed_longest_stall_ms
    )
    transport_active_delivery_gap_count = stall_kind_counts[
        "transport_active_delivery_gap"
    ]
    transport_active_loss_recovery_count = stall_kind_counts[
        "transport_active_loss_recovery"
    ]
    transport_idle_stall_count = stall_kind_counts["transport_idle_stall"]
    explicitly_unknown_stall_count = stall_kind_counts["unknown"]
    unclassified_stall_count = explicitly_unknown_stall_count + max(
        0, stall_count - observed_stall_episode_count
    )
    stats_samples = sum(sample.path_stats_available for sample in events.samples)
    network_samples = [sample for sample in events.samples if sample.path_stats_available]
    # Connection-scoped counters (schema v6) keep counting while no path is
    # selected and across migrations, so they are the denominator that reveals
    # how much of the payload the per-path counters actually saw.
    connection_samples = [
        sample for sample in events.samples if sample.connection_stats_available
    ]
    path_udp_rx_observed = sum(sample.udp_rx_bytes_delta for sample in network_samples)
    connection_udp_rx_total = (
        summary.connection_udp_rx_bytes_total
        if summary is not None and summary.connection_udp_rx_bytes_total > 0
        else sum(sample.connection_udp_rx_bytes_delta for sample in connection_samples)
    )
    connection_stats_present = bool(connection_samples) and connection_udp_rx_total > 0
    path_counter_coverage = (
        min(1.0, path_udp_rx_observed / connection_udp_rx_total)
        if connection_stats_present
        else None
    )
    max_path_count = max(
        (sample.path_count for sample in events.samples), default=0
    )
    wire_paths = _wire_path_attribution(events.samples)
    stream_data_blocked_rx_total = (
        summary.stream_data_blocked_rx_total
        if summary is not None and summary.stream_data_blocked_rx_total > 0
        else sum(sample.stream_data_blocked_rx_delta for sample in connection_samples)
    )
    rtts = [sample.rtt_us for sample in network_samples if sample.rtt_us > 0]
    min_rtt_us = min(rtts) if rtts else None
    rtt_inflations = (
        [rtt / min_rtt_us for rtt in rtts] if min_rtt_us is not None else []
    )
    cwnds = [sample.local_cwnd_bytes for sample in network_samples]
    mtus = [sample.current_mtu for sample in network_samples if sample.current_mtu > 0]
    config = events.config
    window_config_known = (
        config is not None
        and config.known
        and config.source in {"measured", "configured"}
        and config.stream_receive_window_bytes > 0
    )
    bdp_window_ratios = (
        [
            speed
            * (rtt_us / 1_000_000.0)
            / config.stream_receive_window_bytes
            for speed, rtt_us in zip(speeds, rtt_windows)
            if rtt_us is not None
        ]
        if window_config_known and config is not None
        else []
    )
    outcome = summary.outcome if summary is not None else "incomplete"
    measurement_valid = (
        outcome == "complete" and full_window_count >= MIN_STABILITY_WINDOWS
    )
    first_byte_seen = (
        summary.first_byte_seen
        if summary is not None
        else any(sample.first_byte_seen for sample in events.samples)
    )
    time_to_first_byte_ms = (
        summary.time_to_first_byte_ms
        if summary is not None
        else next(
            (
                sample.time_to_first_byte_ms
                for sample in events.samples
                if sample.first_byte_seen
            ),
            None,
        )
    )

    return {
        "source_id": source_id,
        "transfer_id": transfer_id,
        "benchmark_run_id": events.benchmark_run_id,
        "outcome": outcome,
        "path": path,
        "final_path": final_path,
        "sample_count": len(events.samples),
        "steady_sample_count": sum(
            not sample.warmup for sample in payload_samples
        ),
        "throughput_window_ms": THROUGHPUT_WINDOW_MS,
        "throughput_window_count": len(speeds),
        "full_throughput_window_count": full_window_count,
        "used_partial_throughput_window": full_window_count == 0,
        "throughput_analyzed_ms": analyzed_ms,
        "throughput_windows_bytes_per_sec": speeds,
        "measurement_valid": measurement_valid,
        "window_config_known": window_config_known,
        "config_source": config.source if config is not None else "unknown",
        "stream_receive_window_bytes": (
            config.stream_receive_window_bytes if config is not None else None
        ),
        "connection_receive_window_bytes": (
            config.connection_receive_window_bytes if config is not None else None
        ),
        "send_window_bytes": config.send_window_bytes if config is not None else None,
        "congestion_controller": (
            config.congestion_controller if config is not None else "unknown"
        ),
        "build_profile": config.build_profile if config is not None else "unknown",
        "bdp_window_ratio_p50": (
            percentile(bdp_window_ratios, 0.50) if bdp_window_ratios else None
        ),
        "bdp_window_ratio_p90": (
            percentile(bdp_window_ratios, 0.90) if bdp_window_ratios else None
        ),
        "bdp_window_ratio_max": max(bdp_window_ratios) if bdp_window_ratios else None,
        "elapsed_ms": summary.elapsed_ms if summary else max(s.elapsed_ms for s in events.samples),
        "bytes_total": summary.bytes_total if summary else max(s.bytes_total for s in events.samples),
        "average_bytes_per_sec": summary.average_bytes_per_sec if summary else round(mean),
        "mean_window_bytes_per_sec": mean,
        "p10_bytes_per_sec": p10,
        "p50_bytes_per_sec": median,
        "p90_bytes_per_sec": p90,
        "p10_to_median": p10_to_median,
        "coefficient_of_variation": coefficient_of_variation,
        "windows_below_10pct_median": sum(value < low_threshold for value in speeds),
        "low_speed_episodes": _low_speed_episodes(speeds, low_threshold),
        "first_byte_seen": first_byte_seen,
        "time_to_first_byte_ms": time_to_first_byte_ms,
        "warmup_ms": summary.warmup_ms if summary is not None else time_to_first_byte_ms,
        "finalization_pause_ms": (
            summary.finalization_pause_ms if summary is not None else None
        ),
        "stall_count": stall_count,
        "stall_total_ms": summary.stall_total_ms if summary is not None else None,
        "longest_stall_ms": longest_stall_ms,
        "observed_stall_episode_count": observed_stall_episode_count,
        "reported_stall_episode_count": len(stall_episodes),
        "truncated_stall_episode_count": max(
            0, observed_stall_episode_count - len(stall_episodes)
        ),
        "stall_episodes": stall_episodes,
        "stall_udp_active_threshold_bytes_per_sec": (
            UDP_ACTIVE_RATE_THRESHOLD_BYTES_PER_SEC
        ),
        "provider_timeline_alignment_offset_ms": provider_timeline_offset_ms,
        "provider_loss_timeline_aligned": provider_timeline_offset_ms is not None,
        "transport_active_delivery_gap_count": transport_active_delivery_gap_count,
        "transport_active_loss_recovery_count": (
            transport_active_loss_recovery_count
        ),
        "transport_idle_stall_count": transport_idle_stall_count,
        "unclassified_stall_count": unclassified_stall_count,
        "network_stats_coverage": stats_samples / len(events.samples),
        "path_counter_discontinuity_count": sum(
            sample.path_counter_discontinuity for sample in events.samples
        ),
        "connection_stats_present": connection_stats_present,
        "connection_udp_rx_bytes_total": (
            connection_udp_rx_total if connection_stats_present else None
        ),
        "path_counter_coverage": path_counter_coverage,
        "max_path_count": max_path_count,
        "max_active_path_count": max(
            (sample.active_path_count for sample in events.samples), default=0
        ),
        # Wire bytes attributed to the kind of path that carried them, summed
        # over every path. `relay_bytes_ratio` above is application payload
        # attributed to the *selected* path, so it reports no relay traffic
        # whenever payload rides a relay path that was never selected.
        **wire_paths,
        # STREAM_DATA_BLOCKED frames received: the sender had payload ready and
        # this receiver's advertised stream window stopped it. Non-zero settles
        # the window-bound question that `bdp_window_ratio` can only hint at.
        "stream_data_blocked_rx_total": (
            stream_data_blocked_rx_total if connection_stats_present else None
        ),
        "receive_window_bound_evidence": (
            stream_data_blocked_rx_total > 0 if connection_stats_present else None
        ),
        "rtt_min_us": min_rtt_us,
        "rtt_p50_us": percentile(rtts, 0.50) if rtts else None,
        "rtt_p90_us": percentile(rtts, 0.90) if rtts else None,
        "rtt_inflation_p50": (
            percentile(rtt_inflations, 0.50) if rtt_inflations else None
        ),
        "rtt_inflation_p90": (
            percentile(rtt_inflations, 0.90) if rtt_inflations else None
        ),
        "rtt_inflation_max": max(rtt_inflations) if rtt_inflations else None,
        "local_cwnd_p50_bytes": percentile(cwnds, 0.50) if cwnds else None,
        "udp_rx_bytes_observed": sum(sample.udp_rx_bytes_delta for sample in network_samples),
        "local_lost_packets_observed": sum(
            sample.local_lost_packets_delta for sample in network_samples
        ),
        "local_lost_bytes_observed": sum(
            sample.local_lost_bytes_delta for sample in network_samples
        ),
        "local_congestion_events_observed": sum(
            sample.local_congestion_events_delta for sample in network_samples
        ),
        "path_mtu_min": min(mtus) if mtus else None,
        "path_mtu_max": max(mtus) if mtus else None,
        "local_plpmtud_probe_losses_observed": sum(
            sample.local_plpmtud_probe_loss_delta for sample in network_samples
        ),
        **path_metrics,
        "passes_p10_70pct_median": (
            measurement_valid and p10_to_median is not None and p10_to_median >= 0.70
        ),
        "passes_p10_80pct_median": (
            measurement_valid and p10_to_median is not None and p10_to_median >= 0.80
        ),
        "passes_no_stall": stall_count == 0,
    }


def summarize_provider(
    source_id: int, transfer_id: int, events: ProviderEvents
) -> dict[str, object]:
    summary = events.summary
    network_samples = [sample for sample in events.samples if sample.path_stats_available]
    rtts = [sample.rtt_us for sample in network_samples if sample.rtt_us > 0]
    min_rtt_us = min(rtts) if rtts else None
    rtt_inflations = (
        [rtt / min_rtt_us for rtt in rtts] if min_rtt_us is not None else []
    )
    cwnds = [sample.cwnd_bytes for sample in network_samples]
    mtus = [sample.current_mtu for sample in network_samples if sample.current_mtu > 0]
    config = events.config
    udp_tx_total = (
        summary.udp_tx_bytes_total
        if summary is not None
        else sum(sample.udp_tx_bytes_delta for sample in network_samples)
    )
    # Connection-scoped counters (schema v6) are immune to the two ways the
    # path-scoped ones lose payload: no path being selected, and migration
    # voiding an interval. Where both exist, their ratio is the provenance
    # check — it says outright how much of the connection's traffic the
    # per-path loss/cwnd figures were in a position to observe.
    connection_samples = [
        sample for sample in events.samples if sample.connection_stats_available
    ]
    connection_udp_tx_total = (
        summary.connection_udp_tx_bytes_total
        if summary is not None and summary.connection_udp_tx_bytes_total > 0
        else sum(sample.connection_udp_tx_bytes_delta for sample in connection_samples)
    )
    connection_stats_present = bool(connection_samples) and connection_udp_tx_total > 0
    path_counter_coverage = (
        min(1.0, udp_tx_total / connection_udp_tx_total)
        if connection_stats_present
        else None
    )
    max_path_count = max(
        (sample.path_count for sample in events.samples), default=0
    )
    wire_paths = _wire_path_attribution(events.samples)
    stream_data_blocked_total = (
        summary.stream_data_blocked_tx_total
        if summary is not None and summary.stream_data_blocked_tx_total > 0
        else sum(sample.stream_data_blocked_tx_delta for sample in connection_samples)
    )
    return {
        "source_id": source_id,
        "transfer_id": transfer_id,
        "benchmark_run_id": events.benchmark_run_id,
        "outcome": summary.outcome if summary is not None else "incomplete",
        "path": summary.path if summary is not None else events.samples[-1].path,
        "sample_count": len(events.samples),
        "elapsed_ms": (
            summary.elapsed_ms
            if summary is not None
            else max(sample.elapsed_ms for sample in events.samples)
        ),
        # Path-scoped, and a lower bound whenever `path_counter_coverage` is
        # below 1.0. Read `connection_udp_tx_bytes_total` for the real figure.
        "udp_tx_bytes_total": udp_tx_total,
        "udp_tx_average_bytes_per_sec": (
            round(udp_tx_total * 1_000 / summary.elapsed_ms)
            if summary is not None and summary.elapsed_ms > 0
            else None
        ),
        "connection_stats_present": connection_stats_present,
        "connection_udp_tx_bytes_total": (
            connection_udp_tx_total if connection_stats_present else None
        ),
        "connection_udp_tx_average_bytes_per_sec": (
            round(connection_udp_tx_total * 1_000 / summary.elapsed_ms)
            if connection_stats_present
            and summary is not None
            and summary.elapsed_ms > 0
            else None
        ),
        "path_counter_coverage": path_counter_coverage,
        "max_path_count": max_path_count,
        "max_active_path_count": max(
            (sample.active_path_count for sample in events.samples), default=0
        ),
        **wire_paths,
        "connection_samples_without_stats": (
            summary.connection_samples_without_stats if summary is not None else None
        ),
        # STREAM_DATA_BLOCKED frames this provider sent: it had payload ready
        # and the receiver's advertised stream window held it. Non-zero is
        # direct proof of a receive-window-bound transfer, which no ratio of
        # throughput to window can establish on its own.
        "stream_data_blocked_tx_total": (
            stream_data_blocked_total if connection_stats_present else None
        ),
        "receive_window_bound_evidence": (
            stream_data_blocked_total > 0 if connection_stats_present else None
        ),
        "rtt_min_us": min_rtt_us,
        "rtt_p50_us": percentile(rtts, 0.50) if rtts else None,
        "rtt_p90_us": percentile(rtts, 0.90) if rtts else None,
        "rtt_inflation_p50": (
            percentile(rtt_inflations, 0.50) if rtt_inflations else None
        ),
        "rtt_inflation_p90": (
            percentile(rtt_inflations, 0.90) if rtt_inflations else None
        ),
        "rtt_inflation_max": max(rtt_inflations) if rtt_inflations else None,
        "cwnd_p50_bytes": percentile(cwnds, 0.50) if cwnds else None,
        "cwnd_p90_bytes": percentile(cwnds, 0.90) if cwnds else None,
        "lost_packets_total": (
            summary.lost_packets_total
            if summary is not None
            else sum(sample.lost_packets_delta for sample in network_samples)
        ),
        "lost_bytes_total": (
            summary.lost_bytes_total
            if summary is not None
            else sum(sample.lost_bytes_delta for sample in network_samples)
        ),
        "congestion_events_total": (
            summary.congestion_events_total
            if summary is not None
            else sum(sample.congestion_events_delta for sample in network_samples)
        ),
        "path_counter_discontinuity_count": (
            summary.path_counter_discontinuity_count
            if summary is not None
            else sum(sample.path_counter_discontinuity for sample in events.samples)
        ),
        "path_mtu_min": min(mtus) if mtus else None,
        "path_mtu_max": max(mtus) if mtus else None,
        "config_known": config.known if config is not None else False,
        "config_source": config.source if config is not None else "unknown",
        "local_stream_receive_window_bytes": (
            config.stream_receive_window_bytes if config is not None else None
        ),
        "local_connection_receive_window_bytes": (
            config.connection_receive_window_bytes if config is not None else None
        ),
        "send_window_bytes": config.send_window_bytes if config is not None else None,
        "congestion_controller": (
            config.congestion_controller if config is not None else "unknown"
        ),
        "build_profile": config.build_profile if config is not None else "unknown",
    }


def build_report(parsed: ParsedTelemetry) -> dict[str, object]:
    provider_event_rows = [
        (source_id, transfer_id, events)
        for (source_id, transfer_id), events in sorted(parsed.providers.items())
        if events.samples
    ]
    provider_events_by_run_id: dict[int, list[ProviderEvents]] = {}
    for _, _, events in provider_event_rows:
        if isinstance(events.benchmark_run_id, int):
            provider_events_by_run_id.setdefault(events.benchmark_run_id, []).append(
                events
            )

    runs = [
        summarize_transfer(
            source_id,
            transfer_id,
            events,
            provider_events=(
                provider_events_by_run_id[events.benchmark_run_id][0]
                if isinstance(events.benchmark_run_id, int)
                and len(provider_events_by_run_id.get(events.benchmark_run_id, [])) == 1
                else None
            ),
        )
        for (source_id, transfer_id), events in sorted(parsed.transfers.items())
        if events.samples
    ]
    provider_runs = [
        summarize_provider(source_id, transfer_id, events)
        for source_id, transfer_id, events in provider_event_rows
    ]
    if not runs and not provider_runs:
        raise AnalysisError("no blob telemetry samples found")

    phase_rows = [
        {
            "source_id": source_id,
            "role": phase.role,
            "benchmark_run_id": phase.benchmark_run_id,
            "phase": phase.phase,
            "outcome": phase.outcome,
            "elapsed_ms": phase.elapsed_ms,
            "bytes_total": phase.bytes_total,
            "file_count": phase.file_count,
        }
        for source_id, phase in parsed.phases
    ]
    phases_by_run_id: dict[int, list[dict[str, object]]] = {}
    for phase in phase_rows:
        run_id = phase["benchmark_run_id"]
        if isinstance(run_id, int):
            phases_by_run_id.setdefault(run_id, []).append(phase)
    providers_by_run_id: dict[int, list[dict[str, object]]] = {}
    for provider in provider_runs:
        run_id = provider["benchmark_run_id"]
        if isinstance(run_id, int):
            providers_by_run_id.setdefault(run_id, []).append(provider)

    def attach_phase_timings(row: dict[str, object]) -> None:
        run_id = row["benchmark_run_id"]
        matches = phases_by_run_id.get(run_id, []) if isinstance(run_id, int) else []
        row["phase_timing_count"] = len(matches)
        row["phases"] = matches
        row["phase_timings_ms"] = {
            f"{phase['role']}.{phase['phase']}": phase["elapsed_ms"]
            for phase in matches
        }
        row["phase_outcomes"] = {
            f"{phase['role']}.{phase['phase']}": phase["outcome"]
            for phase in matches
        }

    for provider in provider_runs:
        attach_phase_timings(provider)
    for run in runs:
        run_id = run["benchmark_run_id"]
        matches = providers_by_run_id.get(run_id, []) if isinstance(run_id, int) else []
        run["provider_match_count"] = len(matches)
        run["provider"] = matches[0] if len(matches) == 1 else None
        attach_phase_timings(run)

    all_speeds = [
        float(speed)
        for run in runs
        for speed in run["throughput_windows_bytes_per_sec"]
    ]
    run_medians = [float(run["p50_bytes_per_sec"]) for run in runs]
    run_cvs = [
        float(run["coefficient_of_variation"])
        for run in runs
        if run["coefficient_of_variation"] is not None
    ]
    aggregate = {
        "run_count": len(runs),
        "provider_run_count": len(provider_runs),
        "receiver_metrics_available": bool(runs),
        "completed_run_count": sum(run["outcome"] == "complete" for run in runs),
        "sample_count": sum(int(run["sample_count"]) for run in runs),
        "throughput_window_count": len(all_speeds),
        "measurement_valid_run_count": sum(bool(run["measurement_valid"]) for run in runs),
        "p10_bytes_per_sec": percentile(all_speeds, 0.10) if all_speeds else None,
        "p50_bytes_per_sec": percentile(all_speeds, 0.50) if all_speeds else None,
        "median_of_run_medians_bytes_per_sec": (
            statistics.median(run_medians) if run_medians else None
        ),
        "mean_run_coefficient_of_variation": statistics.fmean(run_cvs) if run_cvs else None,
        "total_stall_count": (
            sum(int(run["stall_count"]) for run in runs) if runs else None
        ),
        "total_transport_active_delivery_gap_count": (
            sum(int(run["transport_active_delivery_gap_count"]) for run in runs)
            if runs
            else None
        ),
        "total_transport_active_loss_recovery_count": (
            sum(int(run["transport_active_loss_recovery_count"]) for run in runs)
            if runs
            else None
        ),
        "total_transport_idle_stall_count": (
            sum(int(run["transport_idle_stall_count"]) for run in runs)
            if runs
            else None
        ),
        "total_unclassified_stall_count": (
            sum(int(run["unclassified_stall_count"]) for run in runs)
            if runs
            else None
        ),
        "accepted_run_count_70pct": sum(
            bool(run["passes_p10_70pct_median"] and run["passes_no_stall"])
            for run in runs
        ),
        "accepted_run_count_80pct": sum(
            bool(run["passes_p10_80pct_median"] and run["passes_no_stall"])
            for run in runs
        ),
    }
    return {
        "schema_version": 5,
        "skipped_lines": parsed.skipped_lines,
        "malformed_telemetry_lines": parsed.malformed_telemetry_lines,
        "runs": runs,
        "provider_runs": provider_runs,
        "phase_events": phase_rows,
        "aggregate": aggregate,
    }


def analyze_paths(
    paths: list[str],
    *,
    max_input_mib: int,
    max_line_chars: int,
    max_samples: int,
    max_transfers: int,
) -> dict[str, object]:
    if paths.count("-") > 1 or ("-" in paths and len(paths) > 1):
        raise AnalysisError("stdin ('-') must be the only input")
    parsed = ParsedTelemetry()
    for source_id, raw_path in enumerate(paths):
        if raw_path == "-":
            parse_stream(
                sys.stdin,
                parsed,
                source_id=source_id,
                max_line_chars=max_line_chars,
                max_samples=max_samples,
                max_transfers=max_transfers,
                max_input_chars=max_input_mib * 1024 * 1024,
            )
            continue
        path = Path(raw_path)
        safe_path = ascii(str(path))
        try:
            size = path.stat().st_size
        except OSError as error:
            detail = error.strerror or type(error).__name__
            raise AnalysisError(f"cannot inspect {safe_path}: {detail}") from error
        max_bytes = max_input_mib * 1024 * 1024
        if size > max_bytes:
            raise AnalysisError(f"input exceeds {max_input_mib} MiB limit: {safe_path}")
        try:
            with path.open("r", encoding="utf-8", errors="replace") as stream:
                parse_stream(
                    stream,
                    parsed,
                    source_id=source_id,
                    max_line_chars=max_line_chars,
                    max_samples=max_samples,
                    max_transfers=max_transfers,
                    max_input_chars=max_input_mib * 1024 * 1024,
                )
        except OSError as error:
            detail = error.strerror or type(error).__name__
            raise AnalysisError(f"cannot read {safe_path}: {detail}") from error
    return build_report(parsed)


def _format_rate(value: object) -> str:
    return f"{float(value) / (1024 * 1024):.1f} MiB/s"


def print_human_report(report: dict[str, object]) -> None:
    runs = report["runs"]
    assert isinstance(runs, list)
    if runs:
        print("ID   path     result      windows   p10        median     p10/p50  CV     stalls")
        for run in runs:
            assert isinstance(run, dict)
            ratio = run["p10_to_median"]
            cv = run["coefficient_of_variation"]
            print(
                f"{int(run['source_id'])}:{int(run['transfer_id']):<2} "
                f"{str(run['path']):<8} "
                f"{str(run['outcome']):<11} "
                f"{int(run['throughput_window_count']):>7}   "
                f"{_format_rate(run['p10_bytes_per_sec']):<10} "
                f"{_format_rate(run['p50_bytes_per_sec']):<10} "
                f"{ratio if ratio is not None else 0:>7.1%}  "
                f"{cv if cv is not None else 0:>5.2f}  "
                f"{int(run['stall_count']):>6}"
            )
            _print_phase_timings(run)
            if int(run["stall_count"]) > 0:
                print(
                    "     stall classes: "
                    f"loss-recovery={int(run['transport_active_loss_recovery_count'])}, "
                    f"transport-active delivery={int(run['transport_active_delivery_gap_count'])}, "
                    f"transport-idle={int(run['transport_idle_stall_count'])}, "
                    f"unknown={int(run['unclassified_stall_count'])}"
                )
            if int(run["path_counter_discontinuity_count"]) > 0:
                print(
                    "     path counter discontinuities: "
                    f"{int(run['path_counter_discontinuity_count'])}; "
                    "network totals are lower bounds"
                )
            _print_transport_provenance(run, "udp rx")
    else:
        print("Receiver throughput samples unavailable; stability metrics were not computed.")

    provider_runs = report["provider_runs"]
    assert isinstance(provider_runs, list)
    if provider_runs:
        print()
        print("Provider ID   path     result      samples   elapsed    UDP tx avg   loss   congestion")
        for provider in provider_runs:
            assert isinstance(provider, dict)
            average = provider["udp_tx_average_bytes_per_sec"]
            average_label = _format_rate(average) if average is not None else "n/a"
            print(
                f"{int(provider['source_id'])}:{int(provider['transfer_id']):<8} "
                f"{str(provider['path']):<8} "
                f"{str(provider['outcome']):<11} "
                f"{int(provider['sample_count']):>7}   "
                f"{int(provider['elapsed_ms']) / 1_000:>6.2f}s   "
                f"{average_label:<12} "
                f"{int(provider['lost_packets_total']):>4}   "
                f"{int(provider['congestion_events_total']):>10}"
            )
            if int(provider["path_counter_discontinuity_count"]) > 0:
                print(
                    "     path counter discontinuities: "
                    f"{int(provider['path_counter_discontinuity_count'])}; "
                    "network totals are lower bounds"
                )
            _print_transport_provenance(provider, "udp tx")
            _print_phase_timings(provider)
    aggregate = report["aggregate"]
    assert isinstance(aggregate, dict)
    print()
    if runs:
        print(
            f"Runs: {aggregate['run_count']} | "
            f"aggregate p10/p50: {_format_rate(aggregate['p10_bytes_per_sec'])} / "
            f"{_format_rate(aggregate['p50_bytes_per_sec'])} | "
            f"stalls: {aggregate['total_stall_count']}"
        )
        print(
            f"Accepted (p10 >= 70% median, no stalls): "
            f"{aggregate['accepted_run_count_70pct']}/{aggregate['run_count']}"
        )
    else:
        print(
            f"Receiver runs: 0 | provider runs: {aggregate['provider_run_count']} | "
            "stability: unavailable"
        )
    if report["skipped_lines"] or report["malformed_telemetry_lines"]:
        print(
            f"Ignored lines: {report['skipped_lines']} non-telemetry, "
            f"{report['malformed_telemetry_lines']} malformed telemetry",
            file=sys.stderr,
        )


def _print_transport_provenance(row: dict[str, object], direction: str) -> None:
    """Say how much of the connection's traffic the path counters observed.

    Silent when there is nothing to report: full coverage needs no comment, and
    a pre-v6 log has no connection counters to compare against, which is not the
    same as a bad measurement.
    """

    coverage = row.get("path_counter_coverage")
    if not isinstance(coverage, (int, float)):
        return
    if coverage < 0.99:
        max_paths = row.get("max_path_count")
        # Two different diagnoses share the same symptom, so name which one it
        # is: several live paths means the selected path only ever carried part
        # of the traffic, while a single path means samples were taken with no
        # path selected at all.
        cause = (
            f" ({int(max_paths)} paths in use)"
            if isinstance(max_paths, int) and max_paths > 1
            else " (samples with no selected path)"
            if isinstance(max_paths, int) and max_paths > 0
            else ""
        )
        print(
            f"     path counters saw {coverage * 100:.1f}% of connection "
            f"{direction}{cause}; per-path loss/cwnd are lower bounds"
        )
    if row.get("receive_window_bound_evidence") is True:
        print("     peer reported STREAM_DATA_BLOCKED: receive window bound")
    relay_ratio = row.get("wire_relay_bytes_ratio")
    if isinstance(relay_ratio, (int, float)) and relay_ratio > 0.01:
        print(
            f"     {relay_ratio * 100:.1f}% of wire bytes went over a relay path "
            "(payload-based relay ratio cannot see this)"
        )


def _print_phase_timings(row: dict[str, object]) -> None:
    timings = row["phase_timings_ms"]
    assert isinstance(timings, dict)
    if not timings:
        return
    rendered = ", ".join(
        f"{name}={int(elapsed_ms)}ms" for name, elapsed_ms in sorted(timings.items())
    )
    print(f"     phases: {rendered}")


def build_argument_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Summarize JSON logs emitted with "
            "RUST_LOG=wisp_transfer_telemetry=debug and --log-format json."
        )
    )
    parser.add_argument("logs", nargs="+", help="JSONL log path(s), or '-' for stdin")
    parser.add_argument("--json", action="store_true", help="emit the full report as JSON")
    parser.add_argument(
        "--fail-on-unstable",
        action="store_true",
        help="exit 1 unless every run has p10 >= 70%% median and zero stalls",
    )
    parser.add_argument("--max-input-mib", type=int, default=DEFAULT_MAX_INPUT_MIB)
    parser.add_argument("--max-line-chars", type=int, default=DEFAULT_MAX_LINE_CHARS)
    parser.add_argument("--max-samples", type=int, default=DEFAULT_MAX_SAMPLES)
    parser.add_argument("--max-transfers", type=int, default=DEFAULT_MAX_TRANSFERS)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_argument_parser().parse_args(argv)
    if (
        args.max_input_mib < 1
        or args.max_line_chars < 1
        or args.max_samples < 1
        or args.max_transfers < 1
    ):
        print("error: parser limits must be positive", file=sys.stderr)
        return 2
    try:
        report = analyze_paths(
            args.logs,
            max_input_mib=args.max_input_mib,
            max_line_chars=args.max_line_chars,
            max_samples=args.max_samples,
            max_transfers=args.max_transfers,
        )
    except (AnalysisError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2

    if args.json:
        json.dump(report, sys.stdout, indent=2, sort_keys=True)
        print()
    else:
        print_human_report(report)

    if args.fail_on_unstable:
        aggregate = report["aggregate"]
        assert isinstance(aggregate, dict)
        if (
            aggregate["run_count"] == 0
            or aggregate["accepted_run_count_70pct"] != aggregate["run_count"]
        ):
            return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
