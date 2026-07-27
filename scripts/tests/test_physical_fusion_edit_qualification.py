import subprocess
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import run_physical_fusion_edit_qualification as qualification


def report() -> dict:
    return {
        "schema": qualification.REPORT_SCHEMA,
        "capture_group_id": "a" * 64,
        "width": 1310,
        "height": 1304,
        "crop_origin": {"x": 2890, "y": 3088},
        "frame_count": 21,
        "archival_policy": "measured_sources_only_no_generative_reconstruction",
        "demosaic": {
            "backend": "metal_ahd",
            "fallback": None,
            "generative_reconstruction": False,
        },
        "glare_physical_scale": True,
        "glare_affected_pixels": 0,
        "trimap_physical_scale": False,
        "mixed_boundary_pixels": 0,
    }


class PhysicalFusionEditQualificationTests(unittest.TestCase):
    def test_expected_real_base_and_documents_pass(self) -> None:
        base = report()
        qualification.validate_base_report(base, 1310, 1304)
        edit = qualification.edit_document(
            base, "b" * 64, "glare", "glare_affected"
        )
        self.assertEqual(edit["schema"], qualification.EDIT_SCHEMA)
        self.assertEqual(edit["operations"][0]["selector"], "glare_affected")
        self.assertEqual(edit["operations"][0]["rect"]["width"], 1310)

    def test_base_validation_fails_closed(self) -> None:
        cases = [
            ("glare_physical_scale", False),
            ("glare_affected_pixels", 1),
            ("trimap_physical_scale", True),
            ("mixed_boundary_pixels", 1),
        ]
        for key, value in cases:
            with self.subTest(key=key):
                invalid = report()
                invalid[key] = value
                with self.assertRaises(RuntimeError):
                    qualification.validate_base_report(invalid, 1310, 1304)

    def test_rejection_requires_nonzero_exit_and_exact_reason(self) -> None:
        qualification.validate_rejection(
            subprocess.CompletedProcess(
                ["trueshot"], 1, stdout=f"error: {qualification.GLARE_REJECTION}"
            ),
            qualification.GLARE_REJECTION,
        )
        with self.assertRaises(RuntimeError):
            qualification.validate_rejection(
                subprocess.CompletedProcess(["trueshot"], 0, stdout=""),
                qualification.GLARE_REJECTION,
            )
        with self.assertRaises(RuntimeError):
            qualification.validate_rejection(
                subprocess.CompletedProcess(["trueshot"], 1, stdout="different failure"),
                qualification.GLARE_REJECTION,
            )


if __name__ == "__main__":
    unittest.main()
