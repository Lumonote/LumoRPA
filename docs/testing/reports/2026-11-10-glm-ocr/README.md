# GLM-OCR real-weight device acceptance, 2026-11-10

The shipped `target/debug/lumo` executed `doc.ocr` with the official
`ZhipuAI/GLM-OCR` weights from ModelScope. All downloaded assets matched their
published sizes and SHA-256 digests in [model-files.json](model-files.json).
Weights and the isolated Python environment remain outside the repository.

All five cases passed. Each case starts a separate CLI/model process; elapsed
time includes model loading, with operating-system caches potentially warm.
Whitespace was excluded when calculating CER. Recognition returned
`source: ocr_local` and `left_machine: false`; cloud fallback, network-dependent
model loading and run-history storage were disabled.

| Case | Process elapsed | Sampled process-tree RSS | CER |
| --- | ---: | ---: | ---: |
| English invoice text | 4.74 s | 2.723 GiB | 0 |
| Chinese order text | 4.31 s | 2.726 GiB | 0 |
| Numbers and aligned columns | 4.68 s | 2.732 GiB | 0 |
| Degraded English scan | 4.15 s | 2.720 GiB | 0 |
| Degraded Chinese scan | 3.73 s | 2.722 GiB | 0 |

[report.json](report.json) and [logs](logs) are the original captured results,
including temporary device paths. [environment.json](environment.json) records
Python/library versions and host architecture. Device placement uses the
worker's automatic selection; the report did not capture the selected device.
RSS is sampled CLI/descendant resident memory and excludes GPU memory.

The fixtures contain synthetic public test text rendered with macOS STHeiti
Medium at 34 px on a white background. The two degraded images use 70% resizing,
0.8-degree rotation, 0.4-pixel Gaussian blur and JPEG quality 65. The images and
references are preserved for reproducibility. These small samples establish a
real local inference baseline, including modest degradation; they do not
establish accuracy for handwriting, complex tables or private business scans,
nor for the other model presets.

To repeat with an installed model and compatible Python environment:

```sh
LUMO_OCR_MAX_TOKENS=512 LUMO_OCR_TIMEOUT_SECS=600 \
python3 scripts/ocr-acceptance.py \
  docs/testing/reports/2026-11-10-glm-ocr/manifest.json \
  --home /absolute/path/to/lumo-home \
  --python /absolute/path/to/ocr-venv/bin/python \
  --lumo target/debug/lumo --ignore-whitespace --output /tmp/glm-ocr-repeat
```

The first attempt without `torchvision` failed in the real GLM processor. This
led to a runtime-probe fix so GLM/Paddle dependencies report missing or broken
`torchvision` before inference. The successful report includes that dependency.
The captured CLI predates the chat-router warning cleanup: its stdout warns
that the supported `local` profile is an unknown chat-provider kind. OCR still
ran through the local worker; the router now recognizes that configuration
without trying to use it as a chat backend.
