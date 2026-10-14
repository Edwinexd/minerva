# Architecture diagrams

The SVGs in this directory are rendered from [`build.py`](build.py). It
talks to `graphviz` directly via the `graphviz` Python package; a handful
of icons are pulled from the `diagrams` package's bundled resources.

To regenerate after editing `build.py`:

```bash
sudo apt-get install graphviz   # or: brew install graphviz
python3 -m venv venv
venv/bin/pip install -r docs/diagrams/requirements.txt
venv/bin/python docs/diagrams/build.py
```

The script writes `system-overview.svg`, `ingest-pipeline.svg`,
`chat-pipeline.svg`, and `visual-extraction.svg` into this directory, inlining any referenced raster
assets as base64 so the SVGs are self-contained when served from GitHub.
