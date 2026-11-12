#!/usr/bin/env python3
"""Offline tests. Network and subprocess calls are forbidden/mocked."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("spotify_api", Path(__file__).with_name("spotify-api.py"))
api = importlib.util.module_from_spec(spec)
spec.loader.exec_module(api)


class MaintenanceTests(unittest.TestCase):
    def test_source_changes_are_not_claimed_as_protocol_breaks(self):
        before = {path: b"original" for path in api.WATCH}
        after = dict(before)
        after[next(iter(api.WATCH))] = b"changed"
        changes = api.compare_sources(before, after)
        self.assertEqual(sum(change["changed"] for change in changes), 1)

    def test_upstream_check_fetches_only_review_sources_with_mocked_network(self):
        with patch.object(api, "fetch", return_value=b"source") as fetch:
            report = api.upstream_report("a" * 40)
        self.assertIn("NOT TESTED", report)
        self.assertIn("not proof", report)
        self.assertTrue(all("github" in call.args[0] for call in fetch.call_args_list))

    def test_reports_never_include_payload_string_values(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = Path(directory) / "capture.jsonl"
            capture.write_text(json.dumps({"uri":"private-account", "payloads":[{"secret":"NEVER-PRINT", "identifier":"PRIVATE-TRACK"}]}) + "\n")
            with patch.object(api.subprocess, "run", return_value=SimpleNamespace(returncode=0)):
                report = api.capture_report(capture)
            self.assertNotIn("NEVER-PRINT", report)
            self.assertNotIn("PRIVATE-TRACK", report)
            self.assertNotIn("private-account", report)
            self.assertIn("PASS", report)

    def test_artifacts_are_private_and_never_overwritten(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "report.md"
            api.write_private(path, "first")
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            with self.assertRaises(FileExistsError):
                api.write_private(path, "overwrite")


if __name__ == "__main__":
    with patch("urllib.request.urlopen", side_effect=AssertionError("Tests must never access the network")):
        unittest.main()
