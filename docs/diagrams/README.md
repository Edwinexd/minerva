# Architecture diagrams

The SVGs in this directory are rendered from [`build.py`](build.py). It
talks to `graphviz` directly via the `graphviz` Python package; a handful
of icons are pulled from the `diagrams` package's bundled resources.

To regenerate after editing `build.py`:

```bash
sudo apt-get install graphviz
python3 -m venv /tmp/diags
/tmp/diags/bin/pip install graphviz diagrams
/tmp/diags/bin/python docs/diagrams/build.py
```

The script writes `system-overview.svg`, `ingest-pipeline.svg`, and
`chat-pipeline.svg` into this directory, inlining any referenced raster
assets as base64 so the SVGs are self-contained when served from GitHub.
