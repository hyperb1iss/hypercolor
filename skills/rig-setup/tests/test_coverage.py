"""Offline coverage for how support requests and coverage rows name a device."""

import importlib.util
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
SPEC = importlib.util.spec_from_file_location("coverage", ROOT / "scripts/coverage.py")
coverage = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(coverage)


class VendorAndModelTests(unittest.TestCase):
    def test_model_leading_with_the_vendor_is_not_doubled(self):
        self.assertEqual(coverage.vendor_and_model("Razer", "Razer Kraken Ultimate"), "Razer Kraken Ultimate")
        self.assertEqual(coverage.vendor_and_model("razer", "Razer Kraken Ultimate"), "Razer Kraken Ultimate")

    def test_model_without_the_vendor_gets_it_prefixed(self):
        self.assertEqual(coverage.vendor_and_model("Razer", "Base Station V2 Chroma"), "Razer Base Station V2 Chroma")

    def test_vendor_must_end_on_a_word_boundary(self):
        self.assertEqual(coverage.vendor_and_model("Razer", "RazerBlade"), "Razer RazerBlade")

    def test_missing_halves_fall_back_to_the_other(self):
        self.assertEqual(coverage.vendor_and_model(None, "Universal Screen"), "Universal Screen")
        self.assertEqual(coverage.vendor_and_model("VID 1CBE", ""), "VID 1CBE")
        self.assertEqual(coverage.vendor_and_model("  ", None), "")


if __name__ == "__main__":
    unittest.main()
