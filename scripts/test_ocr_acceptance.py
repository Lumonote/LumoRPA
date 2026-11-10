"""Acceptance runner contracts, not a substitute for real model recognition."""

import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import random
import subprocess
import sys
import tempfile
import time
import unittest


SPEC = importlib.util.spec_from_file_location("ocr_acceptance", Path(__file__).with_name("ocr-acceptance.py"))
OCR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(OCR)


def slow_distance(left, right):
    row = list(range(len(right) + 1))
    for index, char in enumerate(left, 1):
        current = [index]
        for column, other in enumerate(right, 1):
            current.append(min(current[-1] + 1, row[column] + 1, row[column - 1] + (char != other)))
        row = current
    return row[-1]


class TextMetricsTests(unittest.TestCase):
    def test_unicode_and_whitespace_normalization(self):
        self.assertEqual(OCR.normalized_text("e\u0301\r\n 合同\r金额"), "é\n 合同\n金额")
        self.assertEqual(OCR.normalized_text("e\u0301\r\n 合 同\t", True), "é合同")

    def test_exact_distance_matches_independent_reference(self):
        randomizer = random.Random(1977)
        alphabet = "合同金额abcé🙂"
        for _ in range(500):
            left = "".join(randomizer.choices(alphabet, k=randomizer.randrange(40)))
            right = "".join(randomizer.choices(alphabet, k=randomizer.randrange(40)))
            self.assertEqual(OCR.edit_distance(left, right), slow_distance(left, right), (left, right))
        self.assertEqual(OCR.edit_distance("kitten", "sitting"), 3)
        self.assertEqual(OCR.edit_distance("合同金额100000", "合同金额10000"), 1)

    def test_document_length_scoring(self):
        self.assertEqual(OCR.edit_distance("甲" * 25000, "乙" * 25000), 25000)
        self.assertEqual(OCR.edit_distance("甲" + "共同" * 12000, "乙" + "共同" * 12000), 1)

    def test_provenance_and_output_shape_are_required(self):
        local = {"text": "合同", "source": "ocr_local", "left_machine": False}
        encoded = "✓ run=test state=ok\noutputs: " + json.dumps({"recognize": {"result": local}})
        self.assertEqual(OCR.extract_local_result(encoded), local)
        for value in ([], {"recognize": []}, {"recognize": {"result": None}},
                      {"recognize": {"result": {**local, "source": "ocr_remote"}}},
                      {"recognize": {"result": {**local, "left_machine": True}}},
                      {"recognize": {"result": {"text": "合同", "source": "ocr_local"}}}):
            with self.subTest(value=value), self.assertRaises(OCR.AcceptanceError):
                OCR.extract_local_result("outputs: " + json.dumps(value))


class ManifestFixture(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="lumo-ocr-test-")
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name).resolve()
        (self.root / "image.png").write_bytes(b"test image fixture")
        self.manifest = self.root / "manifest.json"

    def load(self, document):
        self.manifest.write_text(json.dumps(document), encoding="utf-8")
        return OCR.load_cases(self.manifest, 0.1, False)


class ManifestTests(ManifestFixture):
    def test_reference_file_and_manifest_relative_paths(self):
        (self.root / "reference.txt").write_text("合同\n金额", encoding="utf-8")
        model, cases = self.load({"version": 1, "model": "modelscope/owner/model", "cases": [
            {"id": "invoice", "image": "image.png", "expectedFile": "reference.txt", "timeoutMs": 1000.5},
        ]})
        self.assertEqual(model, "modelscope/owner/model")
        self.assertEqual(cases[0]["expected"], "合同\n金额")
        self.assertEqual(cases[0]["image"], self.root / "image.png")

    def test_malformed_cases_fail(self):
        valid = {"id": "document", "image": "image.png", "expectedText": "合同"}
        invalid = [[], {"version": True, "cases": [valid]}, {"version": 1, "cases": []},
                   {"version": 1, "cases": [valid, valid]}]
        for changes in ({"expectedText": ""}, {"maxCer": float("nan")}, {"maxPeakRssMb": -1},
                        {"timeoutMs": False}, {"expectedFile": "reference.txt"}, {"image": "."}):
            invalid.append({"version": 1, "cases": [{**valid, **changes}]})
        for document in invalid:
            with self.subTest(document=document), self.assertRaises(OCR.AcceptanceError):
                self.load(document)


@unittest.skipUnless(sys.platform in ("darwin", "linux"), "process group contract requires POSIX")
class RunnerContractTests(ManifestFixture):
    def setUp(self):
        super().setUp()
        self.home = self.root / "home"
        (self.home / "models" / "ocr").mkdir(parents=True)
        self.binary = self.root / "fixture-lumo"
        self.output = self.root / "report"
        self.load({"version": 1, "model": "modelscope/owner/model", "cases": [
            {"id": "document", "image": "image.png", "expectedText": "合同金额100000", "timeoutMs": 2000.5},
        ]})

    def fixture_binary(self, mode="local"):
        # This fixture only tests CLI/config/report contracts. It performs no OCR.
        self.binary.write_text("#!" + sys.executable + "\n" + r'''
import json, os, pathlib, sys, time
assert sys.argv[1] == "--home" and sys.argv[3] == "run" and sys.argv[5] == "--no-store"
flow = json.loads(pathlib.Path(sys.argv[4]).read_text())
parameters = flow["spec"]["steps"][0]["with"]
assert parameters["ocr"]["engine"] == "local" and parameters["ocr"]["allow_cloud"] is False
assert parameters["ocr"]["model"] == "modelscope/owner/model"
assert flow["spec"]["capabilities"] == {"fs.read": [parameters["image"]]}
assert pathlib.Path(parameters["image"]).read_bytes() == b"test image fixture"
assert flow["metadata"]["ai"]["diagnose_on_failure"] is False
providers = pathlib.Path(os.environ["LUMO_PROVIDERS_PATH"]).read_text()
assert 'kind = "local"' in providers and "api_key" not in providers
assert os.environ["LUMO_OCR_PYTHON"] == "fixture-existing-python"
assert os.environ["LUMO_STEP_TIMEOUT_MS"] == "181000"
for flag in ("HF_HUB_OFFLINE", "TRANSFORMERS_OFFLINE", "HF_DATASETS_OFFLINE", "MODELSCOPE_OFFLINE"):
    assert os.environ[flag] == "1"
time.sleep(0.2)
''' + ('''print("missing dependency: torch", file=sys.stderr)
sys.exit(2)
''' if mode == "missing" else '''print("outputs: " + json.dumps({"recognize": {"result": {
    "text": "合同金额100000", "source": "''' + ("ocr_local" if mode == "local" else "ocr_remote") + '''", "left_machine": False}}}))
'''), encoding="utf-8")
        self.binary.chmod(0o700)

    def run_main(self, extra=()):
        with contextlib.redirect_stdout(io.StringIO()):
            status = OCR.main([str(self.manifest), "--home", str(self.home), "--lumo", str(self.binary),
                               "--python", "fixture-existing-python", "--output", str(self.output), *extra])
        return status, json.loads((self.output / "report.json").read_text())

    def test_local_contract_writes_scored_report_without_changing_home(self):
        self.fixture_binary()
        sentinel = self.home / "providers.toml"
        sentinel.write_text("user configuration", encoding="utf-8")
        status, report = self.run_main()
        self.assertEqual(status, 0, report)
        result = report["cases"][0]
        self.assertEqual(result["cer"], 0)
        self.assertEqual(result["source"], "ocr_local")
        self.assertGreater(result["sampledPeakRssBytes"], 0)
        self.assertGreaterEqual(result["elapsedMs"], 200)
        self.assertEqual(sentinel.read_text(), "user configuration")
        self.assertFalse((self.home / "data.db").exists())

    def test_dependency_failure_is_failed_and_retains_logs_and_measurements(self):
        self.fixture_binary("missing")
        status, report = self.run_main()
        self.assertEqual(status, 1)
        result = report["cases"][0]
        self.assertEqual(result["status"], "failed")
        self.assertIn("missing dependency: torch", result["error"])
        self.assertGreater(result["elapsedMs"], 0)
        self.assertIn("missing dependency", (self.output / result["stderrLog"]).read_text())

    def test_remote_provenance_cannot_pass(self):
        self.fixture_binary("remote")
        status, report = self.run_main()
        self.assertEqual(status, 1)
        self.assertIn("local provenance", report["cases"][0]["error"])

    def test_threshold_failures_retain_scores(self):
        self.fixture_binary()
        self.load({"version": 1, "model": "modelscope/owner/model", "cases": [
            {"id": "document", "image": "image.png", "expectedText": "合同金额100001",
             "maxCer": 0, "maxElapsedMs": 1, "maxPeakRssMb": 1},
        ]})
        status, report = self.run_main()
        self.assertEqual(status, 1)
        result = report["cases"][0]
        self.assertEqual(result["editDistance"], 1)
        self.assertGreater(result["cer"], 0)
        for reason in ("CER", "elapsed", "RSS"):
            self.assertIn(reason, result["error"])

    def test_missing_model_home_fails_instead_of_skipping(self):
        self.fixture_binary()
        (self.home / "models" / "ocr").rmdir()
        status, report = self.run_main()
        self.assertEqual(status, 1)
        self.assertEqual(report["status"], "failed")
        self.assertIn("no models will be downloaded", report["error"])

    def test_timeout_terminates_descendant_worker(self):
        pid_file = self.root / "worker.pid"
        source = "import subprocess,sys,time; from pathlib import Path; " + \
                 "child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)']); " + \
                 f"Path({str(pid_file)!r}).write_text(str(child.pid)); time.sleep(60)"
        with self.assertRaises(OCR.MeasurementError) as caught:
            OCR.run_measured([sys.executable, "-c", source], dict(os.environ), 300,
                             self.root / "timeout.stdout", self.root / "timeout.stderr")
        self.assertIn("deadline", str(caught.exception))
        self.assertGreater(caught.exception.peak_rss, 0)
        worker = int(pid_file.read_text())
        for _ in range(20):
            state = subprocess.run(["ps", "-o", "stat=", "-p", str(worker)], capture_output=True, text=True).stdout.strip()
            if not state or state.startswith("Z"):
                break
            time.sleep(0.05)
        else:
            self.fail("timed out OCR descendant is still running")


if __name__ == "__main__":
    unittest.main()
