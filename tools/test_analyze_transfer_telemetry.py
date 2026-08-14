import io
import json
import sys
import unittest
from pathlib import Path
from unittest import mock


sys.path.insert(0, str(Path(__file__).parent))
import analyze_transfer_telemetry as telemetry  # noqa: E402


def event(name, transfer_id, role="receiver", **fields):
    return json.dumps(
        {
            "target": telemetry.TELEMETRY_TARGET,
            "fields": {
                "event": name,
                "role": role,
                "transfer_id": transfer_id,
                **fields,
            },
        }
    )


def sample(
    transfer_id,
    rate,
    elapsed_ms,
    stalled_for_ms=0,
    path="direct",
    *,
    sample_ms=250,
    bytes_delta=None,
    warmup=False,
    first_byte_seen=True,
    time_to_first_byte_ms=100,
    terminal_sample=False,
    rtt_us=2_000,
    benchmark_run_id_available=False,
    benchmark_run_id=0,
    application_path=None,
    udp_rx_bytes_delta=None,
    path_counter_discontinuity=False,
):
    if bytes_delta is None:
        bytes_delta = rate * sample_ms // 1_000
    if udp_rx_bytes_delta is None:
        udp_rx_bytes_delta = rate
    return event(
        "blob_sample",
        transfer_id,
        benchmark_run_id_available=benchmark_run_id_available,
        benchmark_run_id=benchmark_run_id,
        sample_ms=sample_ms,
        app_bytes_per_sec=rate,
        bytes_total=max(bytes_delta, 0),
        bytes_delta=bytes_delta,
        elapsed_ms=elapsed_ms,
        warmup=warmup,
        first_byte_seen=first_byte_seen,
        time_to_first_byte_ms=time_to_first_byte_ms,
        stalled_for_ms=stalled_for_ms,
        terminal_sample=terminal_sample,
        path=path,
        application_path=application_path or path,
        path_stats_available=True,
        path_counter_discontinuity=path_counter_discontinuity,
        rtt_us=rtt_us,
        local_cwnd_bytes=1_000_000,
        udp_rx_bytes_delta=udp_rx_bytes_delta,
        local_lost_packets_delta=0,
        local_lost_bytes_delta=0,
        local_congestion_events_delta=0,
        current_mtu=1_200,
        local_plpmtud_probe_loss_delta=0,
    )


def summary(transfer_id, **overrides):
    fields = {
        "outcome": "complete",
        "elapsed_ms": 1_000,
        "bytes_total": 1_000,
        "average_bytes_per_sec": 1_000,
        "first_byte_seen": True,
        "time_to_first_byte_ms": 100,
        "warmup_ms": 100,
        "finalization_pause_ms": 50,
        "stall_count": 0,
        "stall_total_ms": 0,
        "longest_stall_ms": 0,
        "path": "direct",
    }
    fields.update(overrides)
    return event("blob_summary", transfer_id, **fields)


def config(transfer_id, role="receiver", **overrides):
    fields = {
        "config_known": True,
        "config_source": "configured",
        "send_window_bytes": 8_000_000,
        "congestion_controller": "cubic",
        "build_profile": "release",
        "sample_interval_ms": 250,
        "stall_threshold_ms": 500,
    }
    if role == "provider":
        fields.update(
            local_stream_receive_window_bytes=1_000_000,
            local_connection_receive_window_bytes=(1 << 62) - 1,
        )
    else:
        fields.update(
            stream_receive_window_bytes=1_000_000,
            connection_receive_window_bytes=(1 << 62) - 1,
        )
    fields.update(overrides)
    return event("blob_config", transfer_id, role=role, **fields)


def provider_sample(
    transfer_id,
    *,
    benchmark_run_id=42,
    sample_ms=250,
    elapsed_ms=250,
    rtt_us=2_000,
    lost_packets_delta=2,
    lost_bytes_delta=2_400,
    congestion_events_delta=1,
    path_counter_discontinuity=False,
):
    return event(
        "blob_sample",
        transfer_id,
        role="provider",
        benchmark_run_id_available=True,
        benchmark_run_id=benchmark_run_id,
        sample_ms=sample_ms,
        elapsed_ms=elapsed_ms,
        terminal_sample=False,
        path="direct",
        path_stats_available=True,
        path_counter_discontinuity=path_counter_discontinuity,
        rtt_us=rtt_us,
        cwnd_bytes=2_000_000,
        udp_tx_bytes_delta=250_000,
        udp_rx_bytes_delta=10_000,
        lost_packets_delta=lost_packets_delta,
        lost_bytes_delta=lost_bytes_delta,
        congestion_events_delta=congestion_events_delta,
        current_mtu=1_200,
        plpmtud_probe_loss_delta=0,
    )


def provider_summary(
    transfer_id,
    *,
    benchmark_run_id=42,
    elapsed_ms=250,
    lost_packets_total=2,
    lost_bytes_total=2_400,
    congestion_events_total=1,
    path_counter_discontinuity_count=0,
):
    return event(
        "blob_summary",
        transfer_id,
        role="provider",
        benchmark_run_id_available=True,
        benchmark_run_id=benchmark_run_id,
        outcome="complete",
        elapsed_ms=elapsed_ms,
        udp_tx_bytes_total=250_000,
        lost_packets_total=lost_packets_total,
        lost_bytes_total=lost_bytes_total,
        congestion_events_total=congestion_events_total,
        path_counter_discontinuity_count=path_counter_discontinuity_count,
        path="direct",
    )


def phase_event(
    benchmark_run_id,
    phase,
    elapsed_ms,
    *,
    role="sender",
    outcome="complete",
    string_counters=False,
):
    return event(
        "blob_phase",
        0,
        role=role,
        benchmark_run_id_available=True,
        benchmark_run_id=(
            str(benchmark_run_id) if string_counters else benchmark_run_id
        ),
        phase=phase,
        outcome=outcome,
        elapsed_ms=elapsed_ms,
        bytes_total="1000" if string_counters else 1_000,
        file_count=1,
    )


class TelemetryAnalysisTests(unittest.TestCase):
    def test_percentile_interpolates_small_samples(self):
        values = [100, 200, 300, 400]
        self.assertEqual(telemetry.percentile(values, 0.10), 130.0)
        self.assertEqual(telemetry.percentile(values, 0.50), 250.0)
        self.assertEqual(telemetry.percentile(values, 0.90), 370.0)

    def test_parser_groups_transfers_and_skips_noise(self):
        lines = [
            "plain log output",
            sample(1, 100, 250),
            sample(2, 300, 250, path="relay"),
            sample(1, 200, 500),
            summary(1),
            summary(2, path="relay"),
        ]
        parsed = telemetry.parse_stream(io.StringIO("\n".join(lines) + "\n"))

        self.assertEqual(parsed.skipped_lines, 1)
        self.assertEqual(parsed.sample_count, 3)
        self.assertEqual(len(parsed.transfers[(0, 1)].samples), 2)
        self.assertEqual(parsed.transfers[(0, 2)].summary.path, "relay")

    def test_same_transfer_id_from_different_sources_stays_separate(self):
        parsed = telemetry.parse_stream(
            io.StringIO(sample(1, 100, 250) + "\n"), source_id=0
        )
        telemetry.parse_stream(
            io.StringIO(sample(1, 200, 250) + "\n"), parsed, source_id=1
        )
        report = telemetry.build_report(parsed)

        self.assertEqual(len(report["runs"]), 2)
        self.assertEqual(
            [(run["source_id"], run["transfer_id"]) for run in report["runs"]],
            [(0, 1), (1, 1)],
        )

    def test_report_calculates_stability_and_stalls_per_run(self):
        lines = [
            *[sample(7, 100, elapsed) for elapsed in range(250, 1_001, 250)],
            sample(7, 0, 1_250),
            sample(7, 0, 1_500),
            sample(7, 0, 1_750, stalled_for_ms=500),
            sample(7, 0, 2_000, stalled_for_ms=750),
            *[sample(7, 100, elapsed) for elapsed in range(2_250, 3_001, 250)],
            summary(
                7,
                stall_count=1,
                stall_total_ms=600,
                longest_stall_ms=600,
            ),
        ]
        parsed = telemetry.parse_stream(io.StringIO("\n".join(lines) + "\n"))
        report = telemetry.build_report(parsed)
        run = report["runs"][0]

        self.assertEqual(run["p50_bytes_per_sec"], 100.0)
        self.assertEqual(run["windows_below_10pct_median"], 1)
        self.assertEqual(run["low_speed_episodes"], 1)
        self.assertEqual(run["stall_count"], 1)
        self.assertEqual(run["transport_idle_stall_count"], 1)
        self.assertEqual(run["transport_active_delivery_gap_count"], 0)
        self.assertEqual(run["transport_active_loss_recovery_count"], 0)
        self.assertEqual(run["unclassified_stall_count"], 0)
        self.assertEqual(run["full_throughput_window_count"], 3)
        self.assertTrue(run["measurement_valid"])
        self.assertEqual(run["rtt_p50_us"], 2_000.0)
        self.assertEqual(run["network_stats_coverage"], 1.0)
        self.assertEqual(run["path_mtu_min"], 1_200)
        self.assertFalse(run["passes_p10_70pct_median"])
        self.assertFalse(run["passes_no_stall"])

    def test_stall_with_sustained_udp_rx_is_classified_as_delivery_gap(self):
        lines = [
            sample(11, 100_000, 250),
            sample(
                11,
                0,
                750,
                sample_ms=500,
                stalled_for_ms=500,
                udp_rx_bytes_delta=500_000,
            ),
            sample(
                11,
                0,
                1_250,
                sample_ms=500,
                stalled_for_ms=1_000,
                udp_rx_bytes_delta=500_000,
            ),
            sample(11, 100_000, 1_500),
            summary(
                11,
                stall_count=1,
                stall_total_ms=1_000,
                longest_stall_ms=1_000,
            ),
        ]
        run = telemetry.build_report(
            telemetry.parse_stream(io.StringIO("\n".join(lines) + "\n"))
        )["runs"][0]

        self.assertEqual(run["transport_active_delivery_gap_count"], 1)
        self.assertEqual(run["transport_active_loss_recovery_count"], 0)
        self.assertEqual(run["transport_idle_stall_count"], 0)
        self.assertEqual(run["unclassified_stall_count"], 0)
        self.assertEqual(run["stall_episodes"][0]["duration_ms"], 1_000)
        self.assertEqual(run["stall_episodes"][0]["udp_rx_bytes"], 1_000_000)
        self.assertFalse(
            run["stall_episodes"][0]["provider_loss_evidence_available"]
        )
        self.assertFalse(run["passes_no_stall"])

    def test_provider_loss_reclassifies_transport_active_gap_as_recovery(self):
        run_fields = {
            "benchmark_run_id_available": True,
            "benchmark_run_id": 42,
        }
        receiver_lines = [
            sample(13, 100_000, 250, **run_fields),
            sample(
                13,
                0,
                750,
                sample_ms=500,
                stalled_for_ms=500,
                udp_rx_bytes_delta=500_000,
                **run_fields,
            ),
            sample(
                13,
                0,
                1_250,
                sample_ms=500,
                stalled_for_ms=1_000,
                udp_rx_bytes_delta=500_000,
                **run_fields,
            ),
            sample(13, 100_000, 1_500, **run_fields),
            summary(
                13,
                elapsed_ms=1_500,
                stall_count=1,
                stall_total_ms=1_000,
                longest_stall_ms=1_000,
                **run_fields,
            ),
        ]
        parsed = telemetry.parse_stream(
            io.StringIO("\n".join(receiver_lines) + "\n"), source_id=0
        )
        provider_lines = [
            provider_sample(
                14,
                sample_ms=500,
                elapsed_ms=500,
                lost_packets_delta=0,
                lost_bytes_delta=0,
                congestion_events_delta=0,
            ),
            provider_sample(
                14,
                sample_ms=500,
                elapsed_ms=1_000,
                lost_packets_delta=7,
                lost_bytes_delta=8_400,
                congestion_events_delta=3,
            ),
            provider_sample(
                14,
                sample_ms=500,
                elapsed_ms=1_500,
                lost_packets_delta=0,
                lost_bytes_delta=0,
                congestion_events_delta=0,
            ),
            provider_summary(
                14,
                elapsed_ms=1_500,
                lost_packets_total=7,
                lost_bytes_total=8_400,
                congestion_events_total=3,
            ),
        ]
        telemetry.parse_stream(
            io.StringIO("\n".join(provider_lines) + "\n"),
            parsed,
            source_id=1,
        )

        run = telemetry.build_report(parsed)["runs"][0]
        episode = run["stall_episodes"][0]
        self.assertTrue(run["provider_loss_timeline_aligned"])
        self.assertEqual(run["provider_timeline_alignment_offset_ms"], 0)
        self.assertEqual(run["transport_active_loss_recovery_count"], 1)
        self.assertEqual(run["transport_active_delivery_gap_count"], 0)
        self.assertEqual(episode["kind"], "transport_active_loss_recovery")
        self.assertTrue(episode["provider_loss_evidence_available"])
        self.assertEqual(episode["provider_lost_packets"], 7)
        self.assertEqual(episode["provider_lost_bytes"], 8_400)
        self.assertEqual(episode["provider_congestion_events"], 3)

    def test_provider_loss_is_not_joined_when_terminal_timeline_skew_is_large(self):
        run_fields = {
            "benchmark_run_id_available": True,
            "benchmark_run_id": 42,
        }
        receiver_lines = [
            sample(15, 100_000, 250, **run_fields),
            sample(
                15,
                0,
                750,
                sample_ms=500,
                stalled_for_ms=500,
                udp_rx_bytes_delta=500_000,
                **run_fields,
            ),
            sample(15, 100_000, 1_000, **run_fields),
            summary(15, elapsed_ms=1_000, stall_count=1, **run_fields),
        ]
        parsed = telemetry.parse_stream(
            io.StringIO("\n".join(receiver_lines) + "\n"), source_id=0
        )
        telemetry.parse_stream(
            io.StringIO(
                "\n".join(
                    [
                        provider_sample(16, elapsed_ms=500),
                        provider_summary(16, elapsed_ms=3_000),
                    ]
                )
                + "\n"
            ),
            parsed,
            source_id=1,
        )

        run = telemetry.build_report(parsed)["runs"][0]
        self.assertFalse(run["provider_loss_timeline_aligned"])
        self.assertIsNone(run["provider_timeline_alignment_offset_ms"])
        self.assertEqual(run["transport_active_loss_recovery_count"], 0)
        self.assertEqual(run["transport_active_delivery_gap_count"], 1)

    def test_provider_counter_discontinuity_keeps_active_stall_unknown(self):
        run_fields = {
            "benchmark_run_id_available": True,
            "benchmark_run_id": 42,
        }
        receiver_lines = [
            sample(20, 100_000, 250, **run_fields),
            sample(
                20,
                0,
                750,
                sample_ms=500,
                stalled_for_ms=500,
                udp_rx_bytes_delta=500_000,
                **run_fields,
            ),
            sample(20, 100_000, 1_000, **run_fields),
            summary(20, elapsed_ms=1_000, stall_count=1, **run_fields),
        ]
        parsed = telemetry.parse_stream(
            io.StringIO("\n".join(receiver_lines) + "\n"), source_id=0
        )
        provider_lines = [
            provider_sample(
                21,
                sample_ms=500,
                elapsed_ms=750,
                lost_packets_delta=0,
                lost_bytes_delta=0,
                congestion_events_delta=0,
                path_counter_discontinuity=True,
            ),
            provider_summary(
                21,
                elapsed_ms=1_000,
                lost_packets_total=0,
                lost_bytes_total=0,
                congestion_events_total=0,
                path_counter_discontinuity_count=1,
            ),
        ]
        telemetry.parse_stream(
            io.StringIO("\n".join(provider_lines) + "\n"),
            parsed,
            source_id=1,
        )

        report = telemetry.build_report(parsed)
        run = report["runs"][0]
        episode = run["stall_episodes"][0]
        self.assertEqual(run["transport_active_delivery_gap_count"], 0)
        self.assertEqual(run["unclassified_stall_count"], 1)
        self.assertEqual(episode["kind"], "unknown")
        self.assertTrue(episode["provider_path_counter_discontinuity"])
        self.assertEqual(
            report["provider_runs"][0]["path_counter_discontinuity_count"], 1
        )

    def test_receiver_counter_discontinuity_keeps_stall_unknown(self):
        lines = [
            sample(22, 100_000, 250),
            sample(
                22,
                0,
                750,
                sample_ms=500,
                stalled_for_ms=500,
                udp_rx_bytes_delta=0,
                path_counter_discontinuity=True,
            ),
            sample(22, 100_000, 1_000),
            summary(22, elapsed_ms=1_000, stall_count=1),
        ]
        run = telemetry.build_report(
            telemetry.parse_stream(io.StringIO("\n".join(lines) + "\n"))
        )["runs"][0]

        episode = run["stall_episodes"][0]
        self.assertEqual(episode["kind"], "unknown")
        self.assertTrue(episode["receiver_path_counter_discontinuity"])
        self.assertEqual(run["path_counter_discontinuity_count"], 1)
        self.assertEqual(run["transport_idle_stall_count"], 0)

    def test_stall_episode_details_are_bounded(self):
        lines = []
        elapsed_ms = 0
        for _ in range(telemetry.MAX_REPORTED_STALL_EPISODES + 1):
            elapsed_ms += 500
            lines.append(
                sample(
                    12,
                    0,
                    elapsed_ms,
                    sample_ms=500,
                    stalled_for_ms=500,
                    udp_rx_bytes_delta=100_000,
                )
            )
            elapsed_ms += 250
            lines.append(sample(12, 100_000, elapsed_ms))
        samples = telemetry.parse_stream(
            io.StringIO("\n".join(lines) + "\n")
        ).transfers[(0, 12)].samples

        episodes, kind_counts, observed_count = telemetry._classify_stall_episodes(
            samples
        )

        self.assertEqual(observed_count, telemetry.MAX_REPORTED_STALL_EPISODES + 1)
        self.assertEqual(len(episodes), telemetry.MAX_REPORTED_STALL_EPISODES)
        self.assertEqual(
            kind_counts["transport_active_delivery_gap"],
            telemetry.MAX_REPORTED_STALL_EPISODES + 1,
        )

    def test_resampling_uses_non_overlapping_weighted_one_second_windows(self):
        lines = [
            sample(3, 100, 400, sample_ms=400, bytes_delta=40),
            sample(3, 200, 1_000, sample_ms=600, bytes_delta=120),
            sample(3, 300, 1_500, sample_ms=500, bytes_delta=150),
            sample(3, 100, 2_000, sample_ms=500, bytes_delta=50),
        ]
        parsed = telemetry.parse_stream(io.StringIO("\n".join(lines) + "\n"))
        windows, full_count, analyzed_ms = telemetry.resample_throughput(
            parsed.transfers[(0, 3)].samples
        )

        self.assertEqual(windows, [160.0, 200.0])
        self.assertEqual(full_count, 2)
        self.assertEqual(analyzed_ms, 2_000)

    def test_warmup_and_finalization_tail_do_not_pollute_stability(self):
        lines = [
            sample(
                4,
                50,
                1_000,
                sample_ms=1_000,
                bytes_delta=50,
                warmup=True,
                time_to_first_byte_ms=1_000,
            ),
            *[
                sample(4, 100, elapsed, time_to_first_byte_ms=1_000)
                for elapsed in range(1_250, 4_251, 250)
            ],
            sample(
                4,
                0,
                5_250,
                stalled_for_ms=1_000,
                sample_ms=1_000,
                bytes_delta=0,
                time_to_first_byte_ms=1_000,
                terminal_sample=True,
            ),
            summary(
                4,
                elapsed_ms=5_250,
                time_to_first_byte_ms=1_000,
                warmup_ms=1_000,
                finalization_pause_ms=1_000,
            ),
        ]
        parsed = telemetry.parse_stream(io.StringIO("\n".join(lines) + "\n"))
        run = telemetry.build_report(parsed)["runs"][0]

        self.assertEqual(run["p10_bytes_per_sec"], 100.0)
        self.assertEqual(run["full_throughput_window_count"], 3)
        self.assertEqual(run["stall_count"], 0)
        self.assertEqual(run["time_to_first_byte_ms"], 1_000)
        self.assertEqual(run["finalization_pause_ms"], 1_000)

    def test_path_byte_ratios_use_payload_deltas(self):
        lines = [
            sample(5, 100, 500, sample_ms=500, bytes_delta=50, path="relay"),
            sample(
                5,
                100,
                1_000,
                sample_ms=500,
                bytes_delta=50,
                path="direct",
                application_path="relay",
                path_counter_discontinuity=True,
            ),
            sample(5, 200, 1_500, sample_ms=500, bytes_delta=100, path="direct"),
            summary(5, path="direct"),
        ]
        parsed = telemetry.parse_stream(io.StringIO("\n".join(lines) + "\n"))
        run = telemetry.build_report(parsed)["runs"][0]

        self.assertEqual(run["relay_bytes_ratio"], 0.5)
        self.assertEqual(run["direct_bytes_ratio"], 0.5)
        self.assertEqual(run["path_migration_count"], 1)
        self.assertEqual(run["path_counter_discontinuity_count"], 1)
        self.assertEqual(run["time_to_direct_ms"], 1_000)

    def test_config_enables_bdp_to_stream_window_diagnostic(self):
        lines = [
            config(6),
            sample(
                6,
                8_000_000,
                1_000,
                sample_ms=1_000,
                bytes_delta=8_000_000,
                rtt_us=100_000,
            ),
            summary(6),
        ]
        parsed = telemetry.parse_stream(io.StringIO("\n".join(lines) + "\n"))
        run = telemetry.build_report(parsed)["runs"][0]

        self.assertTrue(run["window_config_known"])
        self.assertEqual(run["config_source"], "configured")
        self.assertEqual(run["stream_receive_window_bytes"], 1_000_000)
        self.assertAlmostEqual(run["bdp_window_ratio_p50"], 0.8)
        self.assertEqual(run["congestion_controller"], "cubic")

    def test_assumed_upstream_config_is_not_used_for_bdp(self):
        lines = [
            config(
                18,
                config_known=False,
                config_source="assumed_upstream_default",
            ),
            sample(
                18,
                8_000_000,
                1_000,
                sample_ms=1_000,
                bytes_delta=8_000_000,
                rtt_us=100_000,
            ),
            summary(18),
        ]
        run = telemetry.build_report(
            telemetry.parse_stream(io.StringIO("\n".join(lines) + "\n"))
        )["runs"][0]

        self.assertEqual(run["config_source"], "assumed_upstream_default")
        self.assertFalse(run["window_config_known"])
        self.assertIsNone(run["bdp_window_ratio_p50"])

    def test_bdp_and_rtt_inflation_use_time_aligned_windows(self):
        lines = [
            config(17),
            sample(17, 1_000_000, 250, rtt_us=1_000),
            sample(17, 1_000_000, 500, rtt_us=1_000),
            sample(17, 1_000_000, 750, rtt_us=3_000),
            sample(17, 1_000_000, 1_000, rtt_us=5_000),
            summary(17),
        ]
        run = telemetry.build_report(
            telemetry.parse_stream(io.StringIO("\n".join(lines) + "\n"))
        )["runs"][0]

        self.assertAlmostEqual(run["bdp_window_ratio_p50"], 0.0025)
        self.assertEqual(run["rtt_min_us"], 1_000)
        self.assertEqual(run["rtt_inflation_p50"], 2.0)
        self.assertEqual(run["rtt_inflation_p90"], 4.4)
        self.assertEqual(run["rtt_inflation_max"], 5.0)

    def test_provider_stats_join_receiver_by_pseudonymous_run_id(self):
        run_fields = {
            "benchmark_run_id_available": True,
            "benchmark_run_id": 42,
        }
        parsed = telemetry.parse_stream(
            io.StringIO(
                "\n".join(
                    [
                        config(8, **run_fields),
                        sample(8, 1_000, 1_000, sample_ms=1_000, **run_fields),
                        summary(8, **run_fields),
                    ]
                )
                + "\n"
            ),
            source_id=0,
        )
        telemetry.parse_stream(
            io.StringIO(
                "\n".join(
                    [
                        config(9, role="provider", **run_fields),
                        provider_sample(9),
                        provider_summary(9),
                    ]
                )
                + "\n"
            ),
            parsed,
            source_id=1,
        )
        report = telemetry.build_report(parsed)
        run = report["runs"][0]
        provider = run["provider"]

        self.assertEqual(run["provider_match_count"], 1)
        self.assertEqual(len(report["provider_runs"]), 1)
        self.assertEqual(provider["cwnd_p50_bytes"], 2_000_000.0)
        self.assertEqual(provider["lost_packets_total"], 2)
        self.assertEqual(provider["config_source"], "configured")
        self.assertEqual(provider["local_stream_receive_window_bytes"], 1_000_000)

    def test_connection_counters_expose_payload_the_selected_path_missed(self):
        # Reproduces the provenance failure seen on device: the provider served
        # a large payload while `selected_path()` accounted for a sliver of it,
        # because iroh reports no selected path unless its watcher holds an
        # address matching a live path. Connection-scoped counters keep
        # counting regardless, so the report can state the real figure and how
        # much the path counters saw.
        starved_sample = json.loads(provider_sample(31))
        starved_sample["fields"].update(
            udp_tx_bytes_delta=4_261,
            connection_stats_available=True,
            connection_udp_tx_bytes_delta=268_435_456,
            path_count=4,
        )
        starved_summary = json.loads(provider_summary(31))
        starved_summary["fields"].update(
            udp_tx_bytes_total=4_261,
            connection_udp_tx_bytes_total=268_435_456,
            connection_samples_without_stats=0,
            stream_data_blocked_tx_total=0,
        )
        provider = telemetry.build_report(
            telemetry.parse_stream(
                io.StringIO(
                    "\n".join(
                        [
                            config(31, role="provider"),
                            json.dumps(starved_sample),
                            json.dumps(starved_summary),
                        ]
                    )
                    + "\n"
                )
            )
        )["provider_runs"][0]

        self.assertTrue(provider["connection_stats_present"])
        self.assertEqual(provider["connection_udp_tx_bytes_total"], 268_435_456)
        self.assertEqual(provider["udp_tx_bytes_total"], 4_261)
        self.assertLess(provider["path_counter_coverage"], 0.001)
        self.assertFalse(provider["receive_window_bound_evidence"])
        # More than one live path is the ordinary explanation for coverage
        # below 1.0, and the report has to distinguish it from a run where no
        # path was selected at all.
        self.assertEqual(provider["max_path_count"], 4)

    def test_stream_data_blocked_settles_the_receive_window_question(self):
        blocked_sample = json.loads(
            sample(32, 1_000, 1_000, sample_ms=1_000)
        )
        blocked_sample["fields"].update(
            connection_stats_available=True,
            connection_udp_rx_bytes_delta=1_200,
            stream_data_blocked_rx_delta=7,
        )
        blocked_summary = json.loads(summary(32))
        blocked_summary["fields"].update(
            connection_udp_rx_bytes_total=1_200,
            path_udp_rx_bytes_total=1_000,
            stream_data_blocked_rx_total=7,
        )
        run = telemetry.build_report(
            telemetry.parse_stream(
                io.StringIO(
                    "\n".join(
                        [
                            config(32),
                            json.dumps(blocked_sample),
                            json.dumps(blocked_summary),
                        ]
                    )
                    + "\n"
                )
            )
        )["runs"][0]

        self.assertTrue(run["connection_stats_present"])
        self.assertEqual(run["stream_data_blocked_rx_total"], 7)
        self.assertTrue(run["receive_window_bound_evidence"])
        self.assertAlmostEqual(run["path_counter_coverage"], 1_000 / 1_200)

    def test_pre_v6_logs_report_no_connection_counters_rather_than_zeroes(self):
        # Older logs simply lack these fields. They must read as "unmeasured",
        # never as a measured zero — otherwise a run with no evidence either
        # way would look like proven evidence of not being window bound.
        run = telemetry.build_report(
            telemetry.parse_stream(
                io.StringIO(
                    "\n".join(
                        [
                            config(33),
                            sample(33, 1_000, 1_000, sample_ms=1_000),
                            summary(33),
                        ]
                    )
                    + "\n"
                )
            )
        )["runs"][0]

        self.assertFalse(run["connection_stats_present"])
        self.assertIsNone(run["connection_udp_rx_bytes_total"])
        self.assertIsNone(run["path_counter_coverage"])
        self.assertIsNone(run["receive_window_bound_evidence"])
        self.assertIsNone(run["stream_data_blocked_rx_total"])
        # Throughput and stall metrics stay fully usable: a missing additive
        # diagnostic must not invalidate the rest of the record.
        self.assertEqual(run["p50_bytes_per_sec"], 1_000)
        self.assertEqual(run["outcome"], "complete")

    def test_wire_path_attribution_sees_relay_bytes_payload_ratio_misses(self):
        # The payload-based relay ratio attributes application bytes to the
        # selected path, so a transfer whose payload partly rides a
        # never-selected relay path reports no relay traffic at all. Wire-byte
        # attribution across every path is what exposes it.
        relaying = json.loads(provider_sample(34))
        relaying["fields"].update(
            connection_stats_available=True,
            connection_udp_tx_bytes_delta=1_000_000,
            all_paths_udp_tx_bytes_delta=1_000_000,
            path_count=4,
            active_path_count=3,
            direct_path_udp_bytes_delta=750_000,
            relay_path_udp_bytes_delta=250_000,
        )
        provider = telemetry.build_report(
            telemetry.parse_stream(
                io.StringIO(
                    "\n".join(
                        [
                            config(34, role="provider"),
                            json.dumps(relaying),
                            provider_summary(34),
                        ]
                    )
                    + "\n"
                )
            )
        )["provider_runs"][0]

        self.assertTrue(provider["wire_path_bytes_available"])
        self.assertEqual(provider["wire_relay_bytes"], 250_000)
        self.assertEqual(provider["wire_direct_bytes"], 750_000)
        self.assertAlmostEqual(provider["wire_relay_bytes_ratio"], 0.25)
        self.assertEqual(provider["max_active_path_count"], 3)

    def test_pre_v6_logs_have_no_wire_path_attribution(self):
        provider = telemetry.build_report(
            telemetry.parse_stream(
                io.StringIO(
                    "\n".join(
                        [
                            config(35, role="provider"),
                            provider_sample(35),
                            provider_summary(35),
                        ]
                    )
                    + "\n"
                )
            )
        )["provider_runs"][0]

        self.assertFalse(provider["wire_path_bytes_available"])
        self.assertIsNone(provider["wire_relay_bytes_ratio"])

    def test_legacy_provider_window_names_remain_parseable(self):
        config_payload = json.loads(config(19, role="provider"))
        fields = config_payload["fields"]
        fields["stream_receive_window_bytes"] = fields.pop(
            "local_stream_receive_window_bytes"
        )
        fields["connection_receive_window_bytes"] = fields.pop(
            "local_connection_receive_window_bytes"
        )
        del fields["config_source"]
        lines = [
            json.dumps(config_payload),
            provider_sample(19),
            provider_summary(19),
        ]
        provider = telemetry.build_report(
            telemetry.parse_stream(io.StringIO("\n".join(lines) + "\n"))
        )["provider_runs"][0]

        self.assertEqual(provider["config_source"], "configured")
        self.assertEqual(provider["local_stream_receive_window_bytes"], 1_000_000)

    def test_provider_only_report_joins_mobile_phase_without_receiver_metrics(self):
        run_fields = {
            "benchmark_run_id_available": True,
            "benchmark_run_id": 42,
        }
        lines = [
            config(9, role="provider", **run_fields),
            provider_sample(9),
            provider_summary(9),
            phase_event(
                42,
                "background_save",
                171,
                role="receiver",
                string_counters=True,
            ),
        ]
        parsed = telemetry.parse_stream(io.StringIO("\n".join(lines) + "\n"))
        report = telemetry.build_report(parsed)
        provider = report["provider_runs"][0]
        aggregate = report["aggregate"]

        self.assertEqual(report["runs"], [])
        self.assertEqual(provider["phase_timing_count"], 1)
        self.assertEqual(provider["phase_timings_ms"]["receiver.background_save"], 171)
        self.assertFalse(aggregate["receiver_metrics_available"])
        self.assertIsNone(aggregate["p10_bytes_per_sec"])
        self.assertIsNone(aggregate["total_stall_count"])

        with (
            mock.patch.object(telemetry, "analyze_paths", return_value=report),
            mock.patch.object(telemetry, "print_human_report"),
        ):
            self.assertEqual(telemetry.main(["unused", "--fail-on-unstable"]), 1)

    def test_phase_timings_join_receiver_by_pseudonymous_run_id(self):
        run_fields = {
            "benchmark_run_id_available": True,
            "benchmark_run_id": 42,
        }
        lines = [
            sample(10, 1_000, 1_000, sample_ms=1_000, **run_fields),
            summary(10, **run_fields),
            phase_event(42, "prepare_total", 350),
            phase_event(42, "fetch_store", 1_250, role="receiver"),
            phase_event(42, "export", 125, role="receiver"),
        ]
        parsed = telemetry.parse_stream(io.StringIO("\n".join(lines) + "\n"))
        report = telemetry.build_report(parsed)
        run = report["runs"][0]

        self.assertEqual(report["schema_version"], 5)
        self.assertEqual(run["phase_timing_count"], 3)
        self.assertEqual(run["phase_timings_ms"]["sender.prepare_total"], 350)
        self.assertEqual(run["phase_timings_ms"]["receiver.fetch_store"], 1_250)
        self.assertEqual(run["phase_outcomes"]["receiver.export"], "complete")

    def test_unknown_phase_label_is_rejected(self):
        parsed = telemetry.parse_stream(
            io.StringIO(phase_event(42, "\x1b[31mhostile", 1) + "\n")
        )
        self.assertEqual(parsed.phase_count, 0)
        self.assertEqual(parsed.malformed_telemetry_lines, 1)

    def test_mobile_phases_accept_canonical_u64_decimal_strings(self):
        run_id = 18_446_744_073_709_551_615
        lines = [
            sample(
                10,
                1_000,
                1_000,
                sample_ms=1_000,
                benchmark_run_id_available=True,
                benchmark_run_id=run_id,
            ),
            summary(
                10,
                benchmark_run_id_available=True,
                benchmark_run_id=run_id,
            ),
            phase_event(
                run_id,
                "saf_read_copy",
                250,
                string_counters=True,
            ),
            phase_event(
                run_id,
                "background_save",
                500,
                role="receiver",
                string_counters=True,
            ),
        ]
        parsed = telemetry.parse_stream(io.StringIO("\n".join(lines) + "\n"))

        self.assertEqual(parsed.phase_count, 2)
        self.assertEqual(parsed.phases[0][1].benchmark_run_id, run_id)
        self.assertEqual(parsed.phases[1][1].bytes_total, 1_000)
        report = telemetry.build_report(parsed)
        run = report["runs"][0]
        self.assertEqual(run["phase_timing_count"], 2)
        self.assertEqual(run["phase_timings_ms"]["sender.saf_read_copy"], 250)
        self.assertEqual(run["phase_timings_ms"]["receiver.background_save"], 500)

    def test_noncanonical_decimal_string_is_rejected(self):
        line = phase_event(42, "saf_read_copy", 1, string_counters=True)
        payload = json.loads(line)
        payload["fields"]["benchmark_run_id"] = "00042"
        parsed = telemetry.parse_stream(io.StringIO(json.dumps(payload) + "\n"))

        self.assertEqual(parsed.phase_count, 0)
        self.assertEqual(parsed.malformed_telemetry_lines, 1)

    def test_core_sample_numeric_strings_remain_rejected(self):
        payload = json.loads(sample(10, 1_000, 1_000))
        payload["fields"]["app_bytes_per_sec"] = "1000"
        parsed = telemetry.parse_stream(io.StringIO(json.dumps(payload) + "\n"))

        self.assertEqual(parsed.sample_count, 0)
        self.assertEqual(parsed.malformed_telemetry_lines, 1)

    def test_parser_rejects_unbounded_sample_growth(self):
        stream = io.StringIO(sample(1, 100, 250) + "\n" + sample(1, 200, 500) + "\n")
        with self.assertRaises(telemetry.AnalysisError):
            telemetry.parse_stream(stream, max_samples=1)

    def test_parser_rejects_counters_larger_than_u64(self):
        parsed = telemetry.parse_stream(
            io.StringIO(sample(1 << 65, 100, 250) + "\n")
        )
        self.assertEqual(parsed.sample_count, 0)
        self.assertEqual(parsed.malformed_telemetry_lines, 1)

    def test_parser_limits_stream_input_size(self):
        with self.assertRaises(telemetry.AnalysisError):
            telemetry.parse_stream(
                io.StringIO(sample(1, 100, 250) + "\n"), max_input_chars=16
            )

    def test_parser_limits_config_only_transfer_groups(self):
        stream = io.StringIO(config(1) + "\n" + config(2) + "\n")
        with self.assertRaises(telemetry.AnalysisError):
            telemetry.parse_stream(stream, max_transfers=1)

    def test_unknown_path_is_not_reflected_to_terminal_output(self):
        line = sample(1, 100, 250, path="\x1b[31mhostile")
        parsed = telemetry.parse_stream(io.StringIO(line + "\n"))
        self.assertEqual(parsed.transfers[(0, 1)].samples[0].path, "unknown")


if __name__ == "__main__":
    unittest.main()
