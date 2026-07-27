import argparse
import copy
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import run_apple_nef_fusion_qualification as qualification


class FullFrameQualificationTests(unittest.TestCase):
    def args(self) -> argparse.Namespace:
        return argparse.Namespace(
            expected_width=8280,
            expected_height=5520,
            full_frame=True,
            memory_budget_mib=4096,
            max_pagein_amplification=1.25,
            minimum_free_disk_mib=2048,
            max_wall_p95_seconds=300.0,
            max_rss_mib=8192.0,
            max_footprint_mib=6144.0,
            max_energy_joules=1500.0,
            max_thermal_state="fair",
            maximum_regression=0.15,
            baseline=None,
            runs=3,
            warmups=0,
            quality="ultra",
            jobs=4,
            dev_license=True,
        )

    def observation(self) -> dict:
        decoded = 21 * 8280 * 5520 / 1_000_000.0
        return {
            "duration_seconds": 120.0,
            "performance": {
                "maximum_resident_set_size_bytes": 5 * qualification.MIB,
                "peak_physical_footprint_bytes": 4 * qualification.MIB,
                "counters": {
                    "energy_nj": 100_000_000_000,
                    "performance_energy_nj": 90_000_000_000,
                    "disk_bytes_read": 1024,
                    "disk_bytes_written": 2048,
                    "pageins": 60,
                },
                "maximum_thermal_state": "nominal",
                "low_power_mode_observed": False,
            },
            "group_performance": [
                {
                    "decode_seconds": 20.0,
                    "fusion_seconds": 70.0,
                    "demosaic_and_postprocess_seconds": 10.0,
                    "processing_before_export_seconds": 100.0,
                }
            ],
            "group_geometry": [
                {
                    "width": 8280,
                    "height": 5520,
                    "crop_origin_x": 0,
                    "crop_origin_y": 0,
                    "frame_count": 21,
                    "decoded_megapixels": decoded,
                    "admitted_peak_memory_bytes": 3 * 1024 * qualification.MIB,
                    "native_input_bytes": 21 * 8280 * 5520 * 2,
                    "input_arena_released_before_postprocess": 21
                    * 8280
                    * 5520
                    * 2,
                    "major_page_faults": 0,
                }
            ],
            "artifact_hashes": {"artifact-00.tiff": "a" * 64},
            "artifact_bytes": {"artifact-00.tiff": 1024},
            "fusion_semantics": [{"schema": "trueshot.fusion.provenance.v2"}],
            "groups": 1,
            "artifacts": 1,
            "demosaic_adapters": ["Apple M1"],
        }

    def aggregate(
        self, observations: list[dict], source_clean: bool = True
    ) -> tuple[dict, list[str]]:
        return qualification.aggregate(
            observations,
            self.args(),
            {"architecture": "arm64", "page_size_bytes": 16_384},
            "c" * 64,
            21,
            1_000_000,
            "d" * 40,
            source_clean,
            3 * 1024 * qualification.MIB,
        )

    def test_exact_full_frame_extent_passes(self) -> None:
        observation = self.observation()
        record, failures = self.aggregate([copy.deepcopy(observation) for _ in range(3)])
        self.assertEqual(failures, [])
        self.assertTrue(record["passed"])
        self.assertTrue(record["determinism"]["geometry_exact"])
        self.assertTrue(record["determinism"]["decoded_extent_exact"])
        self.assertEqual(
            record["schema"], "trueshot.apple-nef-fusion-qualification.v2"
        )

    def test_roi_cannot_masquerade_as_full_frame(self) -> None:
        observation = self.observation()
        geometry = observation["group_geometry"][0]
        geometry["width"] = 1310
        geometry["height"] = 1304
        geometry["decoded_megapixels"] = 21 * 1310 * 1304 / 1_000_000.0
        record, failures = self.aggregate([copy.deepcopy(observation) for _ in range(3)])
        self.assertFalse(record["passed"])
        self.assertIn(
            "decoded output did not match the expected native geometry", failures
        )

    def test_extent_memory_and_fault_claims_fail_closed(self) -> None:
        observation = self.observation()
        geometry = observation["group_geometry"][0]
        geometry["decoded_megapixels"] -= 1.0
        geometry["admitted_peak_memory_bytes"] = 4096 * qualification.MIB + 1
        geometry["input_arena_released_before_postprocess"] = 0
        geometry["major_page_faults"] = 100_000
        record, failures = self.aggregate([copy.deepcopy(observation) for _ in range(3)])
        self.assertFalse(record["passed"])
        self.assertIn(
            "reported decoded megapixels did not match frame count and geometry",
            failures,
        )
        self.assertIn(
            "admitted peak memory exceeded the configured budget", failures
        )
        self.assertIn(
            "full-frame input arena overlapped RGB postprocessing", failures
        )
        self.assertIn(
            "source page-in amplification exceeded the declared ceiling", failures
        )

    def test_dirty_tracked_source_fails_closed(self) -> None:
        observation = self.observation()
        record, failures = self.aggregate(
            [copy.deepcopy(observation) for _ in range(3)], source_clean=False
        )
        self.assertFalse(record["passed"])
        self.assertIn(
            "tracked source tree was not clean at qualification start", failures
        )


if __name__ == "__main__":
    unittest.main()
