# fetter-scope: Specification (preliminary)

A terminal app that runs `fetter monitor-diff`, reads its event stream, and shows what is changing on a system:
- packages, sites and origins;
- new vulnerabilities, including advisories newly published against packages that did not change;
- new hooks, downgrades and allow-list violations.

It also emails a digest at its own frequency, independent of how often fetter scans and reports, and can email alerts as they happen.

fetter does the scanning, diffing and evaluation; fetter-scope only launches, consumes, displays, accumulates and delivers. It is modeled on `../assistant-bom`, with the same packaging (uv + hatchling, Python 3.14), TUI library (Textual), configuration style (`config.toml` read with `tomllib`, secrets in `.env`), plain-file storage, and Postmark delivery through `urllib`.

This document is preliminary: it fixes the scope and the contract with fetter, and leaves layout details to the first milestones.

**Principles**
- **fetter is the source of truth.** Every finding shown or mailed comes from a fetter event. fetter-scope never re-derives vulnerabilities, hooks or versions. It only folds reports together over time (see Digest).
- **The stream is the only interface.** fetter-scope reads `monitor-diff json` events and never fetter's cache or baseline files. Unknown events and fields are ignored, the rule fetter's own schema 1 readers follow.
- **No silent gaps.** A fetter crash, a failed scan, an audit fetter reports as unavailable, or a failed email is shown and recorded. None of them is ever shown as "unchanged".
- **Read-only toward the system.** fetter-scope never installs, removes or "accepts" anything except through fetter's own commands. For example, a fixed baseline is written by `fetter monitor-diff save`.
- **Local and private.** Reports contain user names, host names and paths. They go only to the configured recipient, and nothing is sent anywhere else.

## Contract with fetter

Requires fetter 5.0.0 or later (the first release with `monitor-diff`, its per-ecosystem counts, and its control channel). At start, fetter-scope runs `fetter --version` and refuses an older binary with a clear message.

**Launch:** one long-running child process:
```
fetter <global args> monitor-diff --period P --report-period R [--report always]
       [--baseline FILE] [--bound FILE ...] [--cvss=N] --control stdin json
```
- stdout is read line by line, as JSON Lines.
- stdin is a pipe held open by fetter-scope for the control channel (below).
- stderr is captured into the event log as `stderr` lines. With `--log`, it becomes fetter's diagnostic log.

**Events** are fetter `monitor_diff::Event`, at schema 1. Each is a JSON object with `schema`, `event`, `time` (`{secs, nanos}`) and `system`.

| Event | Fields used | Use |
|---|---|---|
| `start` | `baseline` (`rolling` or a path), `baseline_time`, `period`, `report_period`, `ecosystems`, `control` | header; scan schedule; whether rescan keys are enabled |
| `scan` | `trigger` (`start`, `period`, `request`), `origins`, `sites`, `packages`, `counts` (per ecosystem: `{origins, sites, packages}`, as `fetter count`), `changes` (a `ScanDiff` since the previous scan) | heartbeat, counts history, live change feed; acknowledges a rescan request |
| `report` | `report`: a `DiffReport` (`status`, `baseline_time`, `time`, `changes`, `new_vulns`, `resolved_vulns`, `audit_error`, `new_violations`, `resolved_violations`) | alerts, digest input |
| `error` | `message` | shown in the event log; counted in the digest |

A line that is not JSON, or an event with a `schema` greater than 1, is kept in the raw log. Its known fields are used, and the header shows a "newer fetter" notice.

**Process lifecycle:**
- **Unexpected exit:** fetter-scope restarts fetter with exponential backoff (5 s doubling to 10 min) and records a `fetter-exit` entry. Three exits in a row with no `scan` event in between stop the restarts and raise a visible error.
- **Exit code 3:** this means an alert in run-once mode, which fetter-scope never uses. It is still treated as an unexpected exit.
- **Stopping** (quit, or a configuration change that needs a restart): fetter-scope writes `stop`, waits up to 30 s for the process to exit (a scan in progress finishes first), then terminates it. A child killed mid-report does not advance the rolling baseline, so either way nothing is lost.

**Control channel:** fetter is launched with `--control stdin`, and fetter-scope writes commands to its stdin, one per line, UTF-8, ending in `\n`:

| Command | fetter's response | Used for |
|---|---|---|
| `scan` | scans now; a `scan` event with `trigger: "request"`. A report follows only if one is due by `--report-period`. | `s`: rescan now (the Live and Overview tabs update) |
| `report` | scans now and reports whether or not one is due: a `scan` event with `trigger: "request"`, then a `report` event (unless nothing changed and `--report change`); the report timer restarts | `r`: report now; before `m` sends a digest, so it includes the current state |
| `stop` | finishes the current scan or report, then exits with 0 | quit and restart |
| anything else | an `error` event (`Unknown control command: …`); fetter keeps running | logged as a protocol error (a fetter-scope bug, or a fetter too old for a command) |

Rules for the supervisor:
- **Commands are read only between scans.** A command sent during a scan waits until it ends, and each command runs its own scan. fetter-scope therefore keeps at most one request outstanding: after writing `scan` or `report`, it ignores further `s`/`r` presses (the footer shows "rescanning…") until a `scan` event with `trigger: "request"` arrives, or 10 minutes pass.
- **Acknowledgement is the next `request` scan event.** There is no request id; with one outstanding request, the next `request` scan answers it. A `report` request is complete at the following `report` event, or at that scan event if nothing changed and no report was written.
- **A closed stdin is not a stop.** If fetter-scope's end of the pipe closes (fetter-scope crashed), fetter keeps scanning on its period. fetter-scope's next start finds no child it owns and launches a new one, so a stray fetter could run alongside. To prevent this, fetter-scope records the child's pid in `state.json` and on start stops a recorded process that is still running fetter (`stop` is impossible without its stdin, so it is terminated).
- **Write failures** (a broken pipe) mean fetter has exited; the exit handling above applies.
- `--control` requires a period above 0, which fetter-scope always uses.

**Attach mode:** `fetter-scope attach FILE` follows a stream that fetter writes itself (`monitor-diff ... json -o FILE`, e.g. under launchd or systemd), without launching fetter. There is no control channel in this mode, so rescan and report-now are disabled. (A file or socket control channel in fetter would enable them; `--control` takes a value so one can be added.)

## Configuration

`config.toml` sits beside the data directory (see Storage). It is read with `tomllib` and never written. Every key is optional.

```toml
[fetter]
binary = "fetter"                 # or an absolute path
args = []                         # global args, e.g. ["-E", "python,npm", "--root", "~/src"]
period = 300                      # seconds between scans
report_period = 300               # seconds between fetter reports (each advances fetter's rolling baseline)
baseline = ""                     # a fixed baseline file from `fetter monitor-diff save`; "" is rolling
bound = ""                        # an allow list for validation; with bound_options, subset, superset, strict
cvss = 0                          # 0 is no filter

[mail]
enabled = false
transport = "postmark"            # or "smtp"
every = "1d"                      # digest frequency: "6h", "1d", "7d", ...
at = "07:00"                      # local time of day, when `every` is a whole number of days
weekday = ""                      # e.g. "mon", when `every` is "7d"
send_unchanged = false            # send a digest even when nothing happened (a heartbeat)
alerts = "immediate"              # "immediate" mails each alert report as it arrives; "digest" waits
alert_min_interval = "15m"        # at most one immediate alert mail per interval; later ones join the next
subject_prefix = "fetter"

[tui]
theme = ""                        # a Textual theme name
feed_limit = 2000                 # live change feed entries kept in memory
```

**Credentials** come from the environment, after loading a gitignored `.env`, never from `config.toml`:
- `REPORT_EMAIL_FROM` and `REPORT_EMAIL_TO` (comma-separated);
- for Postmark: `POSTMARK_SERVER_TOKEN`;
- for SMTP: `SMTP_HOST`, `SMTP_PORT` (default 587), `SMTP_USER`, `SMTP_PASSWORD`, and `SMTP_TLS` (`starttls` by default, or `ssl`).

If mail is enabled but not configured, the app shows "mail not configured" and keeps accumulating.

## Digest: folding reports over time

fetter compares each report with the last reported state, so a digest covering several reports has to combine them. `digest.fold(reports)` combines the reports since the last digest into one view:

- **Packages**, keyed by `(ecosystem, site, key)`:
  - keep the earliest `from` and the latest `to`;
  - if they are equal, the change cancelled out; it is listed under **Transient**, e.g. "installed then removed between 02:10 and 02:40", because a short-lived package is itself a signal;
  - otherwise the kind is recomputed from the net change. Added then removed is transient; added then upgraded is added at the final version. Upgrade versus downgrade is taken from fetter's own classification of the last step whenever the versions match it, and otherwise is `changed`, so Python never re-implements version ordering. See Open questions.
- **Sites, origins and hooks:** net added or removed, by key; anything that cancelled out goes under Transient.
- **Vulnerabilities**, keyed by `(ecosystem, package key, id)`:
  - new and later resolved within the period is listed as **Transient**, "fixed within the period";
  - otherwise net new (with the first `source` seen) or net resolved.
- **Violations:** the same as vulnerabilities, keyed by the record.
- **Problems:** audit errors, `error` events, fetter restarts, and periods with no `scan` event at all, all with times.
- **Status:** `alert` if any report in the period was an alert, even if it later cancelled out; otherwise `changed` or `unchanged`. The digest also lists each alert report it contains, with its time.

Folding is pure: report dicts in, a digest dict out. It is tested against recorded streams.

## Displays (Textual)

**Header and summary line:**
- the system (host, OS), fetter version and schema, and the baseline (rolling, or the fixed file and its time);
- last scan, next scan, and the next digest;
- a status badge (`ALERT`, `changed`, `unchanged`, `fetter not running`).

**Tabs:**
- **Overview:**
  - per-ecosystem counts of origins, sites and packages from the latest `scan` event's `counts`, highlighting an ecosystem that dropped to zero (more likely a scan problem than removals), with a sparkline of packages over the session (from the stored scans);
  - the latest report's status and section counts;
  - the pending digest's counts.
- **Alerts:** one row per alert item, its type first: `vuln`, `hook`, `downgrade`, `violation`. Then ecosystem, package or site, detail, report time, and whether it has been mailed.
  - For a vulnerability, the detail is the ID, CVSS score, source and summary.
  - Enter opens a detail screen: the full summary, severity vector, OSV URL (with copy), and the sites holding the package.
- **Changes:** every package change from reports, with the kind (added, removed, upgraded, downgraded, changed), ecosystem, site, name, from and to, and time.
  - Filters: ecosystem, kind and site substring.
  - A toggle switches between "since last digest" (the folded view) and "all reports".
- **Live:** the `scan` events' changes since the previous scan as they arrive, newest first. These come before fetter's next report, so the live view leads the report.
- **Vulnerabilities:** current new and resolved vulnerabilities for the pending digest period, sortable by CVSS, with copy for the ID and URL.
- **Hooks:** added and removed hooks, with their full detail. A hook's content is code; it is shown verbatim and never run.
- **Mail:** transport and recipients (the address only, never credentials); the last sent time and result; the next digest time; a preview of the pending digest (text body); and the history of sent mails.
- **Log:** raw events and fetter stderr, filterable by event type. Errors are highlighted.

**Keys:**

| Key | Action |
|---|---|
| `s` | rescan now (`scan` on the control channel; launch mode only) |
| `r` | report now (`report` on the control channel; launch mode only) |
| `m` | send the digest now (sends `report` first in launch mode, then sends the digest when that report or scan event arrives, or after 2 minutes) |
| `p` | preview the digest's HTML in a browser (a temp file) |
| `c` | copy the selected row's package, ID or URL (OSC 52, plus the clipboard tool when present, as in assistant-bom) |
| `/` | filter |
| `l` | reload `config.toml` (mail settings only; changing fetter settings needs a restart) |
| `q` | quit; fetter is stopped |

Column headers sort, as in assistant-bom.

**Headless:** `fetter-scope run --headless` runs the same supervisor, storage and mail scheduler without the TUI, for a launchd or systemd service. The TUI can then `attach` to the stored stream to view it.

## Mail

- **Transport:** Postmark through `urllib` (as `assistant-bom/delivery.py`), or SMTP through `smtplib`. Both are stdlib; there is no mail dependency.
- **Subject:** `fetter <host> · ALERT · 3 new vulns · 1 hook · 1 downgrade · 42 changes`, with zero counts omitted, and `unchanged` for a heartbeat.
- **Body:** HTML with inline styles (no classes or SVG), plus a text alternative.
  - Section order: alerts first (new vulnerabilities by CVSS, new hooks, downgrades, new violations), then problems, transient items, and package changes grouped by ecosystem and site. Resolved items come last.
  - The body stays under Gmail's ~102 KB clipping limit. Long change lists are cut with "and N more", and the full folded digest is attached as JSON.
- **Schedule:** a digest is due at `every` (aligned to `at` and `weekday`). It is sent if anything happened in the period, or always when `send_unchanged` is set. A digest whose window passed while the app was not running is sent at the next start, covering the time since the last one.
- **Immediate alerts:** when a `report` with status `alert` arrives and `alerts = "immediate"`, a short alert mail with that report's alert items is sent, rate-limited by `alert_min_interval`. Those items are still included in the digest, marked "mailed".
- **Failure handling:** a failed send is retried at the next scheduler tick, with backoff up to an hour, and is shown in the Mail tab. The digest period does not advance until the send succeeds, so nothing is dropped.

## Storage

The data directory is `~/.local/state/fetter-scope/` (macOS: `~/Library/Application Support/fetter-scope/`; Windows: `%LOCALAPPDATA%\fetter-scope\`), or `--data-dir`. All files are plain:

```
events/YYYY-MM-DD.jsonl     every event as received, plus fetter-scope's own entries (fetter-exit, stderr), one per line; daily files
state.json                  last digest time and result, last alert-mail time, mail backoff
mail/YYYY-MM-DDTHHMM.json   each digest sent (the folded dict), with .html and .txt beside it
config.toml, .env           (if not given by --config / the environment)
```

- On start, the pending period's reports are rebuilt from `events/` since `state.json`'s last digest time. A restart therefore loses nothing.
- Retention: `events/` older than 90 days is removed (configurable). Files under `mail/` are kept.
- Writes are append-and-flush for events, and temp-file-and-rename for `state.json`.

## Repository layout

A separate repository, `fetter-scope`. It is a sibling of `fetter-rs`, not part of the Rust crate.

```
pyproject.toml          textual, python-dotenv; dev: pytest, pytest-asyncio
fetter_scope/
  __main__.py           CLI: (default) tui, run [--headless], attach FILE, digest [--send|--preview], check
  config.py             config.toml + .env → frozen dataclasses
  events.py             event parsing (tolerant), the schema 1 event types as TypedDicts
  supervisor.py         launch, read, restart and stop fetter (asyncio subprocess); control channel (one outstanding request); attach (follow a file)
  store.py              events/, state.json, mail/
  digest.py             fold(reports) → digest; pure
  render.py             digest → HTML (inline styles) and text
  delivery.py           Postmark and SMTP; send_digest, send_alert → sent | failed | skipped
  scheduler.py          digest and alert timing; pure given "now"
  tui.py                the Textual app
tests/
  fixtures/streams/     recorded monitor-diff streams (user, host and paths made generic, as fetter's tests/fixtures/monitor)
  fake_fetter.py        prints a fixture stream with delays, honors --version, can exit early, answers scan/report/stop on stdin
```

`fetter-scope check` validates the configuration, the fetter binary and version, and the mail credentials (a dry run with no send), then prints what would run.

## Testing

No test runs a real fetter scan, sends mail, or touches the network.
- **Events:** parsing every fixture, plus an unknown event, a newer schema, extra fields, and a non-JSON line.
- **Folding:**
  - upgrade then downgrade back (transient);
  - added then removed;
  - added then upgraded;
  - a vulnerability introduced then resolved;
  - a hook added then removed;
  - alert status kept after cancellation;
  - audit errors and missing scans as problems.
- **Scheduler:** `every`/`at`/`weekday` alignment, catch-up after downtime, alert rate limiting, and backoff. All are driven by an injected clock.
- **Supervisor:** `fake_fetter.py` for reading, restart with backoff, the three-exit stop, stopping on quit (`stop`, then terminate after the timeout), the version check, and attach-mode following of an appended file. Control channel: `s` is ignored while a request is outstanding, the next `request` scan acknowledges it, an unknown-command `error` is logged, a broken pipe is handled as an exit, and a stale pid in `state.json` is stopped.
- **Integration (optional, marked):** one test runs the real fetter on a temporary `Cargo.lock` project with a stubbed `--cache-directory` and drives `scan`, `report` and `stop`; skipped when fetter is not on `PATH` or the network is unavailable.
- **Store:** restart rebuilds the pending period, plus retention.
- **Render:** a golden HTML and text for a fixture digest; the size limit with "and N more".
- **Delivery:** Postmark via a stubbed `urlopen`, and SMTP via a stubbed `smtplib.SMTP`. A failure does not advance the period.
- **TUI:** Textual pilot tests: tabs populate from a fixture stream, filters, the detail screen, copy, and the `m` and `s` keys with a fake supervisor and delivery.

## Milestones

1. [ ] Skeleton: packaging, config, `check`, event parsing, fixture streams recorded from fetter.
2. [ ] Supervisor and store: launch, attach, restart, control channel, `events/` log, headless `run` (log only).
3. [ ] Digest folding and render (text and HTML), `digest --preview`.
4. [ ] Delivery and scheduler: Postmark and SMTP, digests and immediate alerts, catch-up, `state.json`.
5. [ ] TUI: header, Overview, Alerts (with detail), Changes, Live, Log.
6. [ ] TUI: Vulnerabilities, Hooks, Mail tabs; keys; sorting and copy.
7. [ ] launchd and systemd examples, README.

## Out of scope for now

- Acting on findings: purging packages or accepting a baseline. These remain fetter commands, run by the user.
- Multiple systems. That is fetter.io's job (`monitor-scan`); fetter-scope is one system, one stream.
- An LLM-written summary, which assistant-bom has. This could be added later under the same rule: it only explains numbers it is given.
- Notifications other than email (Slack, webhooks, desktop). The delivery interface should allow adding them.

## Open questions

- **Transient changes in fetter:** fetter's reports already miss changes that appear and disappear between two of its reports. Should fetter-scope set `report_period = period` by default so that every scan can be reported (the default above), or should fetter accumulate them itself (fetter SPEC milestone 27 follow-up)?
- **Net classification after folding:** for the net kind of a folded package change (first `from` → last `to`), should fetter add a `compare` field, or a small `fetter compare-versions` command, rather than relying on the last step's kind?
- **Control in attach mode:** should fetter add a file or socket control channel (`--control fifo:PATH`?) so a TUI attached to a service's stream can rescan? Until then, attach mode is read-only.
- **Settled:** a rescan trigger (fetter's `--control stdin`) and per-ecosystem counts (`scan.counts`) were added to fetter before 5.0.0.
- **Data directory:** a repository directory, as assistant-bom commits its snapshots, or a state directory? This spec proposes a state directory, since the events contain local paths.
