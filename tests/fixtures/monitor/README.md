# monitor-scan payload fixtures

- `v4.json`: a payload posted by fetter 4.0.1 (built from the `v4.0.1` tag) running `monitor-scan` with two executables (`python`, `python3`) of one virtual environment, sharing one site, holding `requests` 2.31.0 and a git-installed `dill` 0.3.8. Captured from the POST body; the username, hostname and paths were then replaced with generic values. The format is the same from fetter 3.0.0 to 4.0.1. It is read as schema 0, with all data as the Python ecosystem.
- `schema1.json`: a schema 1 payload (fetter 5), with a Python and an npm origin, and hooks for one site. It pins the field names of schema 1: a change that breaks reading it breaks fetter.io.
