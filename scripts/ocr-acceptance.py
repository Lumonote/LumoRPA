#!/usr/bin/env python3
"""Measure real, offline local OCR through the shipped Lumo CLI.

Uses an existing model home, never downloads models, and writes no run history.
Python standard library only; Linux/macOS process-tree RSS is sampled via ps.
See docs/testing/local-ocr.md for the manifest and interpretation of results.
"""

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unicodedata


class AcceptanceError(RuntimeError):
    pass


class MeasurementError(AcceptanceError):
    def __init__(self, message, elapsed_ms, peak_rss):
        super().__init__(message)
        self.elapsed_ms = elapsed_ms
        self.peak_rss = peak_rss


def normalized_text(text, ignore_whitespace=False):
    value = unicodedata.normalize("NFC", text.replace("\r\n", "\n").replace("\r", "\n"))
    return "".join(value.split()) if ignore_whitespace else value


def edit_distance(reference, actual):
    """Exact Unicode Levenshtein distance using Myers' integer bit vectors."""
    start = 0
    while start < min(len(reference), len(actual)) and reference[start] == actual[start]:
        start += 1
    reference, actual = reference[start:], actual[start:]
    end = 0
    while end < min(len(reference), len(actual)) and reference[-1 - end] == actual[-1 - end]:
        end += 1
    if end:
        reference, actual = reference[:-end], actual[:-end]
    if len(reference) > len(actual):
        reference, actual = actual, reference
    if not reference:
        return len(actual)
    masks = {}
    for index, char in enumerate(reference):
        masks[char] = masks.get(char, 0) | (1 << index)
    mask = (1 << len(reference)) - 1
    high_bit = 1 << (len(reference) - 1)
    positive, negative, distance = mask, 0, len(reference)
    for char in actual:
        equal = masks.get(char, 0)
        vertical = equal | negative
        horizontal = (((equal & positive) + positive) ^ positive) | equal
        plus = negative | ~(horizontal | positive)
        minus = positive & horizontal
        distance += bool(plus & high_bit) - bool(minus & high_bit)
        plus, minus = (plus << 1) | 1, minus << 1
        positive = (minus | ~(vertical | plus)) & mask
        negative = (plus & vertical) & mask
    return distance


def finite_number(value, name, minimum=0):
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or value < minimum:
        raise AcceptanceError(f"{name} must be a finite number >= {minimum}")
    return value


def load_cases(path, default_max_cer, ignore_whitespace):
    document = json.loads(path.read_text(encoding="utf-8"))
    if (not isinstance(document, dict) or type(document.get("version")) is not int
            or document.get("version") != 1 or not isinstance(document.get("cases"), list) or not document["cases"]):
        raise AcceptanceError("manifest must contain version: 1 and a nonempty cases array")
    identifiers = set()
    cases = []
    for index, raw in enumerate(document["cases"]):
        if not isinstance(raw, dict):
            raise AcceptanceError(f"case {index + 1} must be an object")
        identifier = raw.get("id")
        if not isinstance(identifier, str) or not identifier.strip() or identifier in identifiers:
            raise AcceptanceError("each case needs a unique, nonempty string id")
        identifiers.add(identifier)
        if not isinstance(raw.get("image"), str):
            raise AcceptanceError(f"{identifier}: image must be a local file path")
        image = (path.parent / raw["image"]).resolve(strict=True)
        if not image.is_file() or image.stat().st_size == 0:
            raise AcceptanceError(f"{identifier}: image is not a nonempty regular file")
        if ("expectedText" in raw) == ("expectedFile" in raw):
            raise AcceptanceError(f"{identifier}: provide exactly one of expectedText or expectedFile")
        if "expectedFile" in raw:
            if not isinstance(raw["expectedFile"], str):
                raise AcceptanceError(f"{identifier}: expectedFile must be a path")
            expected = (path.parent / raw["expectedFile"]).read_text(encoding="utf-8")
        else:
            expected = raw["expectedText"]
        if not isinstance(expected, str):
            raise AcceptanceError(f"{identifier}: expectedText must be a string")
        normalized = normalized_text(expected, ignore_whitespace)
        if not normalized or len(normalized) > 25000:
            raise AcceptanceError(f"{identifier}: normalized reference must contain 1..25000 characters")
        case = {**raw, "image": image, "expected": expected,
                "maxCer": finite_number(raw.get("maxCer", default_max_cer), f"{identifier}.maxCer")}
        for name in ("maxElapsedMs", "maxPeakRssMb", "timeoutMs"):
            if name in raw:
                case[name] = finite_number(raw[name], f"{identifier}.{name}", minimum=1)
        cases.append(case)
    return document.get("model"), cases


def process_tree_rss(pid):
    result = subprocess.run(["ps", "-A", "-o", "pid=,ppid=,rss="], capture_output=True, text=True, check=True, timeout=5)
    records = []
    for line in result.stdout.splitlines():
        parts = line.split()
        if len(parts) == 3:
            records.append(tuple(map(int, parts)))
    children = {pid}
    while True:
        found = {child for child, parent, _rss in records if parent in children}
        if found <= children:
            break
        children |= found
    return sum(rss * 1024 for child, _parent, rss in records if child in children)


def stop_process_group(process):
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        process.wait(timeout=3)
    except subprocess.TimeoutExpired:
        pass
    # The CLI may have exited on TERM while its model worker was still alive.
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait(timeout=3)


def run_measured(command, environment, timeout_ms, stdout_path, stderr_path):
    start = time.monotonic()
    peak_rss = 0
    with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
        process = subprocess.Popen(command, env=environment, stdout=stdout, stderr=stderr, start_new_session=True)
        try:
            while process.poll() is None:
                peak_rss = max(peak_rss, process_tree_rss(process.pid))
                if (time.monotonic() - start) * 1000 > timeout_ms:
                    raise AcceptanceError(f"local OCR exceeded its {timeout_ms:g} ms deadline")
                time.sleep(0.05)
        except (AcceptanceError, OSError, subprocess.SubprocessError) as error:
            stop_process_group(process)
            raise MeasurementError(str(error), round((time.monotonic() - start) * 1000, 2), peak_rss) from error
        except BaseException:
            stop_process_group(process)
            raise
    elapsed_ms = round((time.monotonic() - start) * 1000, 2)
    if process.returncode != 0:
        reason = stderr_path.read_text(encoding="utf-8", errors="replace")[-4000:]
        stop_process_group(process)
        raise MeasurementError(f"CLI exited {process.returncode}: {reason}", elapsed_ms, peak_rss)
    if peak_rss == 0:
        raise MeasurementError("could not measure CLI/model worker RSS", elapsed_ms, peak_rss)
    return elapsed_ms, peak_rss


def extract_local_result(stdout):
    # The current CLI prints a terminal summary before its JSON outputs.
    match = re.search(r"(?m)^outputs:\s*", stdout)
    if not match:
        raise AcceptanceError("CLI returned no persisted step output")
    outputs, _end = json.JSONDecoder().raw_decode(stdout[match.end():])
    recognize = outputs.get("recognize") if isinstance(outputs, dict) else None
    result = recognize.get("result") if isinstance(recognize, dict) else None
    if not isinstance(result, dict) or not isinstance(result.get("text"), str):
        raise AcceptanceError("CLI output has no recognize.result.text")
    if result.get("source") != "ocr_local" or result.get("left_machine") is not False:
        raise AcceptanceError("OCR did not prove local provenance (ocr_local and left_machine=false)")
    return result


def flow_document(image, model, timeout_ms):
    return {
        "apiVersion": "lumorpa.io/v1", "kind": "Flow",
        "metadata": {"id": "local-ocr-device-acceptance", "ai": {
            "enabled": True, "diagnose_on_failure": False, "budget": {"max_calls_per_run": 2},
        }},
        "spec": {
            "capabilities": {"fs.read": [str(image)]},
            "steps": [{"id": "recognize", "action": "doc.ocr", "with": {
                "image": str(image), "ocr": {"engine": "local", "model": model,
                                               "allow_cloud": False, "timeout_ms": int(timeout_ms)},
            }}],
        },
    }


def execute_case(case, number, arguments, binary, home, model, environment, output):
    slug = re.sub(r"[^A-Za-z0-9_-]+", "-", case["id"]).strip("-")[:60] or "document"
    prefix = f"{number:03d}-{slug}"
    stdout_path, stderr_path = output / f"{prefix}.stdout.log", output / f"{prefix}.stderr.log"
    result = {"id": case["id"], "image": str(case["image"]),
              "status": "failed", "stdoutLog": stdout_path.name, "stderrLog": stderr_path.name}
    try:
        result["imageSha256"] = hashlib.sha256(case["image"].read_bytes()).hexdigest()
        with tempfile.TemporaryDirectory(prefix="lumo-ocr-acceptance-") as temporary:
            directory = Path(temporary)
            image = directory / ("document" + case["image"].suffix.lower())
            shutil.copyfile(case["image"], image)
            flow = directory / "recognize.yaml"
            timeout_ms = case.get("timeoutMs", arguments.timeout_ms)
            flow.write_text(json.dumps(flow_document(image, model, timeout_ms), ensure_ascii=False), encoding="utf-8")
            elapsed, rss = run_measured([str(binary), "--home", str(home), "run", str(flow), "--no-store"],
                                        environment, timeout_ms, stdout_path, stderr_path)
            result.update({"elapsedMs": elapsed, "sampledPeakRssBytes": rss})
            actual = extract_local_result(stdout_path.read_text(encoding="utf-8", errors="replace"))
        expected = normalized_text(case["expected"], arguments.ignore_whitespace)
        recognized = normalized_text(actual["text"], arguments.ignore_whitespace)
        if len(recognized) > 25000:
            raise AcceptanceError("normalized recognized text exceeds the 25000-character scoring limit")
        edits = edit_distance(expected, recognized)
        cer = edits / len(expected)
        result.update({"text": actual["text"], "referenceCharacters": len(expected), "editDistance": edits,
                       "cer": cer, "maxCer": case["maxCer"], "elapsedMs": elapsed,
                       "sampledPeakRssBytes": rss, "source": actual["source"], "leftMachine": actual["left_machine"]})
        failures = []
        if cer > case["maxCer"]:
            failures.append(f"CER {cer:.4f} exceeds {case['maxCer']:.4f}")
        if "maxElapsedMs" in case and elapsed > case["maxElapsedMs"]:
            failures.append(f"elapsed {elapsed:g} ms exceeds {case['maxElapsedMs']:g} ms")
        if "maxPeakRssMb" in case and rss > case["maxPeakRssMb"] * 1024 * 1024:
            failures.append(f"sampled process-tree RSS exceeds {case['maxPeakRssMb']:g} MiB")
        if failures:
            raise AcceptanceError("; ".join(failures))
        result["status"] = "passed"
    except (AcceptanceError, OSError, ValueError, subprocess.SubprocessError) as error:
        if isinstance(error, MeasurementError):
            result.update({"elapsedMs": error.elapsed_ms, "sampledPeakRssBytes": error.peak_rss})
        result["error"] = str(error)
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path, help="JSON manifest with local images and reference text")
    parser.add_argument("--home", type=Path, required=True, help="existing Lumo home containing models/ocr")
    parser.add_argument("--model", help="ModelScope model id; overrides manifest.model")
    parser.add_argument("--lumo", default="target/debug/lumo", help="path to a built Lumo CLI")
    parser.add_argument("--python", help="existing OCR Python interpreter (sets LUMO_OCR_PYTHON)")
    parser.add_argument("--output", type=Path, required=True, help="directory for report.json and per-case logs")
    parser.add_argument("--timeout-ms", type=int, default=180000)
    parser.add_argument("--max-cer", type=float, default=0.10)
    parser.add_argument("--ignore-whitespace", action="store_true", help="exclude whitespace from both sides when computing CER")
    arguments = parser.parse_args(argv)
    try:
        arguments.output.mkdir(parents=True, exist_ok=True)
    except OSError as error:
        print(f"cannot create report directory: {error}", file=sys.stderr)
        return 1
    report = {"status": "failed", "cases": [], "normalization": "NFC + LF" + (" + remove whitespace" if arguments.ignore_whitespace else ""),
              "rssMeasurement": "sampled sum of CLI and descendant resident memory, roughly every 50 ms; GPU memory excluded"}
    try:
        if sys.platform not in ("darwin", "linux") or not shutil.which("ps"):
            raise AcceptanceError("process-tree RSS measurement requires macOS/Linux with ps")
        manifest = arguments.manifest.resolve(strict=True)
        home = arguments.home.resolve(strict=True)
        if not home.is_dir() or not (home / "models" / "ocr").is_dir():
            raise AcceptanceError("--home must contain an existing models/ocr directory; no models will be downloaded")
        executable = shutil.which(arguments.lumo)
        binary = Path(executable or arguments.lumo).resolve(strict=True)
        if not binary.is_file() or not os.access(binary, os.X_OK):
            raise AcceptanceError("--lumo must point to a built executable")
        finite_number(arguments.max_cer, "--max-cer")
        finite_number(arguments.timeout_ms, "--timeout-ms", minimum=1)
        manifest_model, cases = load_cases(manifest, arguments.max_cer, arguments.ignore_whitespace)
        model = arguments.model or manifest_model
        if not isinstance(model, str) or not re.fullmatch(r"modelscope/[^/\s]+/[^/\s]+", model):
            raise AcceptanceError("provide an explicit local modelscope/owner/model id")
        report.update({"model": model, "home": str(home), "manifest": str(manifest), "cli": str(binary)})
        # A local-only temporary profile avoids reading/changing the user's
        # selected provider and cannot make a diagnostic cloud request.
        with tempfile.TemporaryDirectory(prefix="lumo-ocr-profile-") as temporary:
            providers = Path(temporary) / "providers.toml"
            providers.write_text('active = "acceptance-local"\n[[providers]]\nname = "acceptance-local"\nkind = "local"\nocr_model = ' + json.dumps(model) + "\n", encoding="utf-8")
            environment = dict(os.environ, LUMO_HOME=str(home), LUMO_PROVIDERS_PATH=str(providers),
                               HF_HUB_OFFLINE="1", TRANSFORMERS_OFFLINE="1", HF_DATASETS_OFFLINE="1", MODELSCOPE_OFFLINE="1",
                               LUMO_STEP_TIMEOUT_MS=str(int(max(arguments.timeout_ms, max(case.get("timeoutMs", 0) for case in cases))) + 1000))
            if arguments.python:
                environment["LUMO_OCR_PYTHON"] = arguments.python
            for number, case in enumerate(cases, 1):
                result = execute_case(case, number, arguments, binary, home, model, environment, arguments.output)
                report["cases"].append(result)
                print(f"{result['id']}: {result['status']}" + (f" ({result['error']})" if "error" in result else ""), flush=True)
        total_characters = sum(case.get("referenceCharacters", 0) for case in report["cases"])
        if total_characters:
            report["microCerOfScoredCases"] = sum(case.get("editDistance", 0) for case in report["cases"]) / total_characters
        report["status"] = "passed" if all(case["status"] == "passed" for case in report["cases"]) else "failed"
    except (AcceptanceError, OSError, ValueError, subprocess.SubprocessError) as error:
        report["error"] = str(error)
    report_path = arguments.output / "report.json"
    try:
        report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    except OSError as error:
        print(f"cannot write report: {error}", file=sys.stderr)
        return 1
    print(f"report: {report_path.resolve()}", flush=True)
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
