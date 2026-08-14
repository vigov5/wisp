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
KNOWN_PHASE_ROLES = frozenset({"sender", "receiver"})
KNOWN_PHASES = frozenset(
    {
        "prepare_total",
        "walk_metadata",
        "import_hash",
        "collection_store",
        "dial",
        "control_handshake",
        "decision_wait",
        "blob_setup",
        "fetch_store",
        "export",
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
    rtt_us: int
    local_cwnd_bytes: int
    udp_rx_bytes_delta: int
    local_lost_packets_delta: int
    local_lost_bytes_delta: int
    local_congestion_events_delta: int
    current_mtu: int
    local_plpmtud_probe_loss_delta: int


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


@dataclass(frozen=True)
class TransferConfig:
    known: bool
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
    rtt_us: int
    cwnd_bytes: int
    udp_tx_bytes_delta: int
    udp_rx_bytes_delta: int
    lost_packets_delta: int
    lost_bytes_delta: int
    congestion_events_delta: int
    current_mtu: int
    plpmtud_probe_loss_delta: int


@dataclass(frozen=True)
class ProviderSummary:
    outcome: str
    elapsed_ms: int
    udp_tx_bytes_total: int
    lost_packets_total: int
    lost_bytes_total: int
    congestion_events_total: int
    path: str


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


def _non_negative_int(value: object) -> int | None:
    if (
        isinstance(value, bool)
        or not isinstance(value, int)
        or value < 0
        or value > MAX_U64
    ):
        return None
    return value


def _known_path(value: object) -> str:
    return value if isinstance(value, str) and value in KNOWN_PATHS else "unknown"


def _known_label(value: object, allowed: frozenset[str]) -> str:
    return value if isinstance(value, str) and value in allowed else "unknown"


def _strict_bool(value: object) -> bool | None:
    return value if isinstance(value, bool) else None


def _benchmark_run_id(fields: dict[str, object]) -> int | None:
    available = fields.get("benchmark_run_id_available")
    value = _non_negative_int(fields.get("benchmark_run_id"))
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
        rtt_us=rtt_us,
        local_cwnd_bytes=local_cwnd_bytes,
        udp_rx_bytes_delta=udp_rx_bytes_delta,
        local_lost_packets_delta=local_lost_packets_delta,
        local_lost_bytes_delta=local_lost_bytes_delta,
        local_congestion_events_delta=local_congestion_events_delta,
        current_mtu=current_mtu,
        local_plpmtud_probe_loss_delta=local_plpmtud_probe_loss_delta,
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
    )


def _parse_config(
    fields: dict[str, object], expected_role: str
) -> tuple[int, TransferConfig] | None:
    if fields.get("role") != expected_role:
        return None
    transfer_id = _non_negative_int(fields.get("transfer_id"))
    known = _strict_bool(fields.get("config_known"))
    stream_window = _non_negative_int(fields.get("stream_receive_window_bytes"))
    connection_window = _non_negative_int(
        fields.get("connection_receive_window_bytes")
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
    return transfer_id, TransferConfig(
        known=known,
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
    if terminal_sample is None or any(value is None for value in values.values()):
        return None
    transfer_id = values.pop("transfer_id")
    assert transfer_id is not None
    return transfer_id, ProviderSample(
        sample_ms=values["sample_ms"],
        elapsed_ms=values["elapsed_ms"],
        terminal_sample=terminal_sample,
        path=_known_path(fields.get("path")),
        path_stats_available=fields.get("path_stats_available") is True,
        rtt_us=values["rtt_us"],
        cwnd_bytes=values["cwnd_bytes"],
        udp_tx_bytes_delta=values["udp_tx_bytes_delta"],
        udp_rx_bytes_delta=values["udp_rx_bytes_delta"],
        lost_packets_delta=values["lost_packets_delta"],
        lost_bytes_delta=values["lost_bytes_delta"],
        congestion_events_delta=values["congestion_events_delta"],
        current_mtu=values["current_mtu"],
        plpmtud_probe_loss_delta=values["plpmtud_probe_loss_delta"],
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
    )
    values = {name: _non_negative_int(fields.get(name)) for name in names}
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
        path=_known_path(fields.get("path")),
    )


def _parse_phase(fields: dict[str, object]) -> PhaseEvent | None:
    role = _known_label(fields.get("role"), KNOWN_PHASE_ROLES)
    phase = _known_label(fields.get("phase"), KNOWN_PHASES)
    outcome = _known_label(fields.get("outcome"), KNOWN_PHASE_OUTCOMES)
    elapsed_ms = _non_negative_int(fields.get("elapsed_ms"))
    bytes_total = _non_negative_int(fields.get("bytes_total"))
    file_count = _non_negative_int(fields.get("file_count"))
    if (
        role == "unknown"
        or phase == "unknown"
        or outcome == "unknown"
        or elapsed_ms is None
        or bytes_total is None
        or file_count is None
    ):
        return None
    return PhaseEvent(
        role=role,
        benchmark_run_id=_benchmark_run_id(fields),
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


def summarize_transfer(
    source_id: int, transfer_id: int, events: TransferEvents
) -> dict[str, object]:
    summary = events.summary
    speeds, full_window_count, analyzed_ms = resample_throughput(events.samples)
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
    path_metrics = _path_metrics(events.samples)

    final_path = summary.path if summary is not None else events.samples[-1].path
    path = str(path_metrics["dominant_path"])
    if path == "unknown":
        path = final_path
    stall_count = summary.stall_count if summary is not None else observed_stall_count
    longest_stall_ms = (
        summary.longest_stall_ms if summary is not None else observed_longest_stall_ms
    )
    stats_samples = sum(sample.path_stats_available for sample in events.samples)
    network_samples = [sample for sample in events.samples if sample.path_stats_available]
    rtts = [sample.rtt_us for sample in network_samples]
    cwnds = [sample.local_cwnd_bytes for sample in network_samples]
    mtus = [sample.current_mtu for sample in network_samples if sample.current_mtu > 0]
    config = events.config
    window_config_known = (
        config is not None
        and config.known
        and config.stream_receive_window_bytes > 0
    )
    bdp_window_ratios = (
        [
            sample.bytes_per_sec
            * (sample.rtt_us / 1_000_000.0)
            / config.stream_receive_window_bytes
            for sample in payload_samples
            if not sample.warmup and sample.path_stats_available and sample.rtt_us > 0
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
        "network_stats_coverage": stats_samples / len(speeds),
        "rtt_p50_us": percentile(rtts, 0.50) if rtts else None,
        "rtt_p90_us": percentile(rtts, 0.90) if rtts else None,
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
    rtts = [sample.rtt_us for sample in network_samples]
    cwnds = [sample.cwnd_bytes for sample in network_samples]
    mtus = [sample.current_mtu for sample in network_samples if sample.current_mtu > 0]
    config = events.config
    udp_tx_total = (
        summary.udp_tx_bytes_total
        if summary is not None
        else sum(sample.udp_tx_bytes_delta for sample in network_samples)
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
        "udp_tx_bytes_total": udp_tx_total,
        "udp_tx_average_bytes_per_sec": (
            round(udp_tx_total * 1_000 / summary.elapsed_ms)
            if summary is not None and summary.elapsed_ms > 0
            else None
        ),
        "rtt_p50_us": percentile(rtts, 0.50) if rtts else None,
        "rtt_p90_us": percentile(rtts, 0.90) if rtts else None,
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
        "path_mtu_min": min(mtus) if mtus else None,
        "path_mtu_max": max(mtus) if mtus else None,
        "config_known": config.known if config is not None else False,
        "send_window_bytes": config.send_window_bytes if config is not None else None,
        "congestion_controller": (
            config.congestion_controller if config is not None else "unknown"
        ),
        "build_profile": config.build_profile if config is not None else "unknown",
    }


def build_report(parsed: ParsedTelemetry) -> dict[str, object]:
    runs = [
        summarize_transfer(source_id, transfer_id, events)
        for (source_id, transfer_id), events in sorted(parsed.transfers.items())
        if events.samples
    ]
    if not runs:
        raise AnalysisError("no blob telemetry samples found")

    provider_runs = [
        summarize_provider(source_id, transfer_id, events)
        for (source_id, transfer_id), events in sorted(parsed.providers.items())
        if events.samples
    ]
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
    for run in runs:
        run_id = run["benchmark_run_id"]
        matches = providers_by_run_id.get(run_id, []) if isinstance(run_id, int) else []
        run["provider_match_count"] = len(matches)
        run["provider"] = matches[0] if len(matches) == 1 else None
        phase_matches = (
            phases_by_run_id.get(run_id, []) if isinstance(run_id, int) else []
        )
        run["phase_timing_count"] = len(phase_matches)
        run["phases"] = phase_matches
        run["phase_timings_ms"] = {
            f"{phase['role']}.{phase['phase']}": phase["elapsed_ms"]
            for phase in phase_matches
        }
        run["phase_outcomes"] = {
            f"{phase['role']}.{phase['phase']}": phase["outcome"]
            for phase in phase_matches
        }

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
        "completed_run_count": sum(run["outcome"] == "complete" for run in runs),
        "sample_count": sum(int(run["sample_count"]) for run in runs),
        "throughput_window_count": len(all_speeds),
        "measurement_valid_run_count": sum(bool(run["measurement_valid"]) for run in runs),
        "p10_bytes_per_sec": percentile(all_speeds, 0.10),
        "p50_bytes_per_sec": percentile(all_speeds, 0.50),
        "median_of_run_medians_bytes_per_sec": statistics.median(run_medians),
        "mean_run_coefficient_of_variation": statistics.fmean(run_cvs) if run_cvs else None,
        "total_stall_count": sum(int(run["stall_count"]) for run in runs),
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
        "schema_version": 3,
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
        timings = run["phase_timings_ms"]
        assert isinstance(timings, dict)
        if timings:
            rendered = ", ".join(
                f"{name}={int(elapsed_ms)}ms"
                for name, elapsed_ms in sorted(timings.items())
            )
            print(f"     phases: {rendered}")
    aggregate = report["aggregate"]
    assert isinstance(aggregate, dict)
    print()
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
    if report["skipped_lines"] or report["malformed_telemetry_lines"]:
        print(
            f"Ignored lines: {report['skipped_lines']} non-telemetry, "
            f"{report['malformed_telemetry_lines']} malformed telemetry",
            file=sys.stderr,
        )


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
        if aggregate["accepted_run_count_70pct"] != aggregate["run_count"]:
            return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
