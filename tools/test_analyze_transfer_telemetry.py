import io
import json
import sys
import unittest
from pathlib import Path


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
):
    if bytes_delta is None:
        bytes_delta = rate * sample_ms // 1_000
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
        rtt_us=rtt_us,
        local_cwnd_bytes=1_000_000,
        udp_rx_bytes_delta=rate,
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
        "stream_receive_window_bytes": 1_000_000,
        "connection_receive_window_bytes": (1 << 62) - 1,
        "send_window_bytes": 8_000_000,
        "congestion_controller": "cubic",
        "build_profile": "release",
        "sample_interval_ms": 250,
        "stall_threshold_ms": 500,
    }
    fields.update(overrides)
    return event("blob_config", transfer_id, role=role, **fields)


def provider_sample(transfer_id, *, benchmark_run_id=42):
    return event(
        "blob_sample",
        transfer_id,
        role="provider",
        benchmark_run_id_available=True,
        benchmark_run_id=benchmark_run_id,
        sample_ms=250,
        elapsed_ms=250,
        terminal_sample=False,
        path="direct",
        path_stats_available=True,
        rtt_us=2_000,
        cwnd_bytes=2_000_000,
        udp_tx_bytes_delta=250_000,
        udp_rx_bytes_delta=10_000,
        lost_packets_delta=2,
        lost_bytes_delta=2_400,
        congestion_events_delta=1,
        current_mtu=1_200,
        plpmtud_probe_loss_delta=0,
    )


def provider_summary(transfer_id, *, benchmark_run_id=42):
    return event(
        "blob_summary",
        transfer_id,
        role="provider",
        benchmark_run_id_available=True,
        benchmark_run_id=benchmark_run_id,
        outcome="complete",
        elapsed_ms=250,
        udp_tx_bytes_total=250_000,
        lost_packets_total=2,
        lost_bytes_total=2_400,
        congestion_events_total=1,
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
        self.assertEqual(run["full_throughput_window_count"], 3)
        self.assertTrue(run["measurement_valid"])
        self.assertEqual(run["rtt_p50_us"], 2_000.0)
        self.assertEqual(run["path_mtu_min"], 1_200)
        self.assertFalse(run["passes_p10_70pct_median"])
        self.assertFalse(run["passes_no_stall"])

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
            ),
            sample(5, 200, 1_500, sample_ms=500, bytes_delta=100, path="direct"),
            summary(5, path="direct"),
        ]
        parsed = telemetry.parse_stream(io.StringIO("\n".join(lines) + "\n"))
        run = telemetry.build_report(parsed)["runs"][0]

        self.assertEqual(run["relay_bytes_ratio"], 0.5)
        self.assertEqual(run["direct_bytes_ratio"], 0.5)
        self.assertEqual(run["path_migration_count"], 1)
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
        self.assertEqual(run["stream_receive_window_bytes"], 1_000_000)
        self.assertAlmostEqual(run["bdp_window_ratio_p50"], 0.8)
        self.assertEqual(run["congestion_controller"], "cubic")

    def test_provider_stats_join_receiver_by_anonymous_run_id(self):
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

    def test_phase_timings_join_receiver_by_anonymous_run_id(self):
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

        self.assertEqual(report["schema_version"], 3)
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
