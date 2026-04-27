# Architecture diagrams

The PNGs in this directory are rendered from [`build.py`](build.py) using the
[mingrammer `diagrams`](https://diagrams.mingrammer.com/) Python package
(graphviz under the hood).

To regenerate after editing `build.py`:

```bash
sudo apt-get install graphviz
python3 -m venv /tmp/diags
/tmp/diags/bin/pip install diagrams
/tmp/diags/bin/python docs/diagrams/build.py
```

The script writes `system-overview.png`, `ingest-pipeline.png`, and
`chat-pipeline.png` into this directory.
