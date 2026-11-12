"""Offline coverage for support intake URLs and read-only duplicate lookup."""

import contextlib
import importlib.util
import io
import json
import sys
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch
from urllib.parse import parse_qs, urlsplit

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
SPEC = importlib.util.spec_from_file_location(
    "request_support", ROOT / "scripts/request_support.py"
)
request_support = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(request_support)

DEVICE = {
    "vendor_id": 0x1A2C,
    "product_id": 0x2C27,
    "manufacturer": "A & B",
    "product": "USB Keyboard / 2",
    "serial": "private-serial",
    "bus_path": "/private/host/path",
    "interface_classes": [3, 255],
}


class SupportRequestTests(unittest.TestCase):
    def run_main(self, *arguments, version="1.2.3", duplicates=None):
        output = io.StringIO()
        with (
            patch.object(
                sys,
                "argv",
                ["request_support.py", "--vid-pid", "1a2c:2c27", *arguments],
            ),
            patch.object(
                request_support, "lookup_device", return_value=(DEVICE, "daemon")
            ),
            patch.object(request_support, "daemon_version", return_value=version),
            patch.object(
                request_support, "existing_issues", return_value=duplicates or []
            ) as search,
            patch.object(request_support, "host_platform", return_value="Linux"),
            patch.object(
                request_support.subprocess,
                "run",
                side_effect=AssertionError("No subprocess allowed"),
            ),
            patch.object(
                request_support,
                "scan_host_usb",
                side_effect=AssertionError("No host scan allowed"),
            ),
            patch.object(
                request_support,
                "Daemon",
                side_effect=AssertionError("No network allowed"),
            ),
            contextlib.redirect_stdout(output),
        ):
            self.assertEqual(request_support.main(), 0)
            product = None
            if "--product" in arguments:
                product = arguments[arguments.index("--product") + 1].strip() or None
            search.assert_called_once_with("1A2C:2C27", product)
        return output.getvalue()

    def test_json_separates_facts_from_unanswered_intake_and_omits_private_identifiers(
        self,
    ):
        result = json.loads(self.run_main("--json"))
        query = parse_qs(urlsplit(result["url"]).query)
        self.assertEqual(query["vid-pid"], ["1A2C:2C27"])
        self.assertEqual(query["version"], ["1.2.3"])
        self.assertEqual(query["platform"], ["Linux"])
        self.assertEqual(query["template"], ["device-support.yml"])
        self.assertIn("USB vendor label: A & B", query["detected-device"][0])
        self.assertIn(
            "USB product string: USB Keyboard / 2", query["detected-device"][0]
        )
        self.assertIn("0x03, 0xFF", query["detected-device"][0])
        self.assertIn("OpenRGB support: not checked.", query["detected-device"][0])
        for field in (
            "title",
            "product",
            "lighting",
            "lighting-control",
            "control-software",
            "vendor",
            "model",
            "existing-support",
            "willing-to-test",
        ):
            self.assertNotIn(field, query)
        self.assertNotIn("private-serial", json.dumps(result))
        self.assertNotIn("/private/host/path", json.dumps(result))

    def test_only_explicit_owner_product_answer_is_prefilled(self):
        result = json.loads(
            self.run_main(
                "--json", "--product", " Retail & Model ", "--platform", "Windows"
            )
        )
        query = parse_qs(urlsplit(result["url"]).query)
        self.assertEqual(query["product"], ["Retail & Model"])
        self.assertEqual(query["platform"], ["Windows"])
        self.assertEqual(query["title"], ["[device] Retail & Model"])

    def test_blank_owner_product_does_not_use_usb_name_as_title(self):
        result = json.loads(self.run_main("--json", "--product", "  "))
        query = parse_qs(urlsplit(result["url"]).query)
        self.assertNotIn("product", query)
        self.assertNotIn("title", query)

    def test_missing_version_remains_unanswered(self):
        result = json.loads(self.run_main("--json", version=None))
        self.assertNotIn("version", parse_qs(urlsplit(result["url"]).query))

    def test_duplicate_matches_do_not_hide_form_for_a_different_product_with_same_id(
        self,
    ):
        duplicate = {
            "number": 543,
            "title": "USB Keyboard",
            "state": "OPEN",
            "url": "https://example.com/543",
        }
        output = self.run_main(duplicates=[duplicate])
        self.assertIn("https://example.com/543", output)
        self.assertIn(request_support.FORM_URL, output)
        result = json.loads(self.run_main("--json", duplicates=[duplicate]))
        self.assertEqual(result["existing_issues"], [duplicate])

    def test_direct_filing_flags_are_rejected_before_any_io(self):
        for flag in ("--file", "--force"):
            with self.subTest(flag=flag), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as error:
                    self.run_main(flag)
                self.assertEqual(error.exception.code, 2)

    def test_parse_usb_ids_checks_unsigned_16_bit_range(self):
        self.assertEqual(
            request_support.parse_vid_pid("0x1A2C:0x2C27"), (0x1A2C, 0x2C27)
        )
        for value in ("-1:2", "10000:2", "2:10000", "not-an-id"):
            with self.subTest(value=value), self.assertRaises(SystemExit):
                request_support.parse_vid_pid(value)

    def test_detected_native_status_does_not_claim_host_inventory_was_checked(self):
        self.assertIn(
            "Native driver: not checked.",
            request_support.detected_details(DEVICE, "host"),
        )
        device = {**DEVICE, "claimable_by": "nollie"}
        self.assertIn(
            "matched disabled driver nollie",
            request_support.detected_details(device, "daemon"),
        )

    def test_daemon_inventory_match_does_not_scan_host(self):
        with (
            patch.object(request_support, "Daemon") as daemon,
            patch.object(request_support, "scan_host_usb") as scan,
        ):
            daemon.return_value.get.return_value = {"data": {"items": [DEVICE]}}
            self.assertEqual(
                request_support.lookup_device("http://test", 0x1A2C, 0x2C27),
                (DEVICE, "daemon"),
            )
            daemon.return_value.get.assert_called_once_with("/devices/unclaimed")
            scan.assert_not_called()

    def test_host_lookup_runs_when_nonempty_daemon_inventory_does_not_match(self):
        with (
            patch.object(request_support, "Daemon") as daemon,
            patch.object(request_support, "scan_host_usb") as scan,
        ):
            daemon.return_value.get.return_value = {
                "data": {"items": [{"vendor_id": 1, "product_id": 2}]}
            }
            scan.return_value = [DEVICE]
            self.assertEqual(
                request_support.lookup_device("http://test", 0x1A2C, 0x2C27),
                (DEVICE, "host"),
            )

    def test_failed_inventory_reads_leave_detection_unknown(self):
        with (
            patch.object(request_support, "Daemon") as daemon,
            patch.object(request_support, "scan_host_usb") as scan,
        ):
            daemon.return_value.get.side_effect = SystemExit("offline")
            scan.side_effect = OSError("no inventory")
            self.assertEqual(
                request_support.lookup_device("http://test", 1, 2), ({}, "not found")
            )

    def test_version_reads_system_envelope_and_handles_unavailable_data(self):
        with patch.object(request_support, "Daemon") as daemon:
            for document, expected in (
                (
                    {"data": {"identity": {"version": " 1.2.3 "}, "status": None}},
                    "1.2.3",
                ),
                (
                    {
                        "data": {
                            "identity": {"version": "1.2.3"},
                            "status": {"version": "different"},
                        }
                    },
                    "1.2.3",
                ),
                (None, None),
                ({"data": {}}, None),
                ({"data": {"identity": {}}}, None),
                ({"data": {"identity": {"version": " "}}}, None),
                ({"data": {"version": "1.2.3"}}, None),
            ):
                daemon.return_value.get.return_value = document
                self.assertEqual(
                    request_support.daemon_version("http://test"), expected
                )
            daemon.return_value.get.assert_called_with("/system")
            for error in (
                SystemExit("offline"),
                TimeoutError("offline"),
                json.JSONDecodeError("bad response", "", 0),
            ):
                daemon.return_value.get.side_effect = error
                self.assertIsNone(request_support.daemon_version("http://test"))

    def test_duplicate_lookup_only_uses_read_only_gh_commands(self):
        matches = [
            {
                "number": 543,
                "title": "USB Keyboard",
                "state": "OPEN",
                "url": "https://example.com/543",
            }
        ]
        with (
            patch.object(request_support.shutil, "which", return_value="/mock/gh"),
            patch.object(
                request_support.subprocess,
                "run",
                side_effect=[
                    MagicMock(returncode=0),
                    MagicMock(returncode=0, stdout=json.dumps(matches)),
                ],
            ) as run,
        ):
            self.assertEqual(request_support.existing_issues("1A2C:2C27"), matches)
            self.assertEqual(run.call_args_list[0].args[0], ["gh", "auth", "status"])
            self.assertEqual(
                run.call_args_list[1].args[0],
                [
                    "gh",
                    "issue",
                    "list",
                    "--repo",
                    request_support.REPO,
                    "--state",
                    "all",
                    "--limit",
                    "10",
                    "--search",
                    "1A2C:2C27",
                    "--json",
                    "number,title,url,state",
                ],
            )

    def test_product_lookup_finds_requests_without_usb_ids_and_deduplicates(self):
        by_id = {"number": 543, "title": "USB Keyboard"}
        by_product = {"number": 550, "title": "Retail & Model"}
        with (
            patch.object(request_support, "gh_ready", return_value=True),
            patch.object(
                request_support.subprocess,
                "run",
                side_effect=[
                    MagicMock(returncode=0, stdout=json.dumps([by_id])),
                    MagicMock(returncode=0, stdout=json.dumps([by_id, by_product])),
                ],
            ) as run,
        ):
            self.assertEqual(
                request_support.existing_issues("1A2C:2C27", "Retail & Model"),
                [by_id, by_product],
            )
            queries = [call.args[0][10] for call in run.call_args_list]
            self.assertEqual(queries, ["1A2C:2C27", '"Retail & Model"'])

    def test_failed_id_search_does_not_hide_product_matches(self):
        matches = [{"number": 550, "title": "Retail Model"}]
        with (
            patch.object(request_support, "gh_ready", return_value=True),
            patch.object(
                request_support.subprocess,
                "run",
                side_effect=[
                    MagicMock(returncode=1),
                    MagicMock(returncode=0, stdout=json.dumps(matches)),
                ],
            ),
        ):
            self.assertEqual(
                request_support.existing_issues("1A2C:2C27", "Retail Model"), matches
            )


if __name__ == "__main__":
    unittest.main()
