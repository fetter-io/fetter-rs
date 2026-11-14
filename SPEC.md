# fetter v6 Specification: Multi-Ecosystem Support

Status: **draft**. This is the working spec for the v6 major release. It is meant to be read at the start of each work session and updated at the end: tick milestones, record decisions, and close open questions.

## How to use this document

- Section 7 (Milestones) is the source of truth for progress. Each milestone must leave `cargo test` green.
- When a decision is made, move it out of section 9 (Open questions) into section 10 (Decision log) with the date.
- Breaking changes are allowed (v6 is a major release), but every one must be listed in section 11 (Breaking changes) for the changelog.
- Update "Picking this up" (next section) at the end of every session.

## Picking this up

*Last updated 2026-11-13, at the end of milestone 7.*

### Where we are

- **Branch** `207/polye`. Milestones **1–7 are done** (§7): spec, core types, scanner trait + walker, CLI rework, OSV multi-ecosystem (including pagination and error handling), and the Cargo and npm scanners.
- **Milestone 7 (npm) may not be committed yet.** Check `git status`; if `src/eco/npm.rs` is untracked, review and commit it before starting milestone 8. That change also raises the MSRV to 1.89 (`Cargo.toml`, `.github/workflows/ci.yml`) and replaces `serde_yaml` with `serde-saphyr`.
- **Health**: `cargo fmt --check` and `cargo clippy --all-targets` are clean; `cargo test` reports 378 passed and 4 ignored. The ignored tests hit live services:
  - `test_osv_ecosystem_names_live`
  - `test_osv_pagination_live` (≈80s)
  - `test_query_pypi_project_live`
  - `test_from_dep_manifest_live`

  Run them with `cargo test <name> -- --ignored`. `tests/windows_session.rs` runs only on Windows and has not been run since milestone 4 changed `count` output and `--origin`. It was updated by hand; verify it in CI.
- **Implemented scanners**: Python (`src/eco/python.rs`), Cargo (`src/eco/cargo.rs`), and npm (`src/eco/npm.rs`). `-E gha|vscode` prints a "not yet supported" warning and is skipped.
- **Python-only isolation in shared commands** (remove as each generalizes):
  - `validate`, `derive`, `purge-invalid` → Python packages only (`scan_fs.rs`, until milestone 11)
  - `unpack-*`, `purge-*` → `supports_artifacts` (Python only, until milestone 13)
  - `inspect` → Python sites only (until milestone 10)
  - `lookup-*` → forced `-E python` (until milestone 12)

### Code map

| Concern | Where |
|---|---|
| Ecosystem enum, OSV names, key normalization | `src/ecosystem.rs` |
| `Origin`, `Site`, `SiteKind` | `src/origin_site.rs` |
| Scanner trait, `Detect`, `DirItem`, registry, `infer_origins` | `src/eco/mod.rs` |
| Per-ecosystem scanners | `src/eco/{python,cargo,npm}.rs` |
| YAML parsing (into `serde_json::Value`) | `serde_saphyr::from_str`; see `eco/npm.rs`, `lock_file.rs` |
| Table rendering, column fitting | `src/table.rs` (`optimize_widths_for` is the testable core) |
| Single filesystem walk (parallel; prune / skip_children / excludes) | `src/walk.rs` |
| `ScanFS`: assembly (`from_parts`, the only constructor path), serialization, cache, report entry points | `src/scan_fs.rs` |
| `ScanConfig` (ecosystems, roots; hashed for the cache) | `src/util.rs` |
| CLI, `resolve_ecosystems`, `python` namespace | `src/cli.rs` |
| OSV batch query, pagination, `is_osv_queryable` | `src/osv_query.rs` |
| Audit report (`unaudited`, errors) | `src/audit_report.rs` |

### Next: milestone 8, the GitHub Actions scanner

The plan:
1. **Survey real data first**: workflow files (`find ~ -path '*/.github/workflows/*' \( -name '*.yml' -o -name '*.yaml' \)`) and local `action.yml`/`action.yaml` files. Tally the `uses:` forms in use:
   - semver tags (`@v4.1.0`)
   - floating major tags (`@v4`)
   - SHA pins (`@11bd719…`)
   - branches (`@main`)
   - subpath actions (`github/codeql-action/init@v3`)
   - reusable workflows (`owner/repo/.github/workflows/x.yml@ref`)
   - local (`./.github/actions/x`) and `docker://`, both skipped

   Also check whether any `.github` directories sit somewhere non-projects live, the way `Cargo.lock` files sat in `~/.cargo/registry`.
2. **Probe OSV for package naming before writing code**: for subpath actions, is the OSV package `github/codeql-action` or `github/codeql-action/init`? Query both live and record the answer in §6.
3. **Origins and sites**:
   - The origin is the repo root (a directory containing `.github/workflows`).
   - Each workflow file is a site (`SiteKind::Manifest`), as are local composite `action.yml` files.
   - The walker visits `.github` (only `.git` is skipped).
   - `--origin` accepts a repo dir, a `.github/workflows` dir, or a single workflow file.
4. **Packages**:
   - Each `uses: owner/repo[/path]@ref` gives the name `owner/repo` (lowercase key) and `ref` as the version.
   - YAML is parsed with `serde-saphyr` into `serde_json::Value`. Walk `jobs.*.steps[*].uses` and `jobs.*.uses` (reusable workflows), plus `runs.steps[*].uses` in composite `action.yml`.
5. **Auditing requires client-side version matching** (§6: OSV does no server-side matching for GHA):
   - Add an ecosystem capability, e.g. `Ecosystem::osv_server_side_versions()`.
   - For GHA, query `/v1/query` by package name only and parse `affected[].ranges` (type `ECOSYSTEM`, events `introduced`/`fixed`/`last_affected`) in `osv_vulns.rs`, where `affected` is currently commented out.
   - Compare the ref's version locally. That needs semver-like ordering with an optional `v` prefix; this is the first real need for `PackageVersion`/range matching (§3). Decide between the `semver` crate and extending `VersionSpec`.
   - Floating major tags and SHAs cannot be placed in a range without resolving them (GitHub API; open question §9), so they go in `AuditReport.unaudited`.
   - Keep the batch path for the other ecosystems and merge the results.
6. **Verify** as before: other ecosystems unchanged (baselines below), deterministic `scan`, timing, and a live `audit` against a known-affected control. `actions/download-artifact@v4.0.0` falls in GHSA-cxww-7g56-2vh6 (`introduced 4.0.0`, `fixed 4.1.3`), and `tj-actions/changed-files@v35.0.0` in GHSA-mrrh-fwg8-r2c3.

### Baselines for regression checks (dev machine, release build, warm cache)

- Full home scan, `count`: python 54 origins / 57 sites / 764 packages; npm 40 / 40 / 5,840 (36 / 36 / 5,803 before `bun.lock` support); cargo 27 / 25 / 1,747; ≈4.0s. (Cargo was 1,737 before this repo's own `Cargo.lock` gained 12 crates from the `serde-saphyr` swap; `Cargo.lock` is gitignored here.)
- Full home `scan`: byte-identical across runs.
- `fetter -E npm audit` (full home): 372 npm packages with advisories, ≈68s uncached (mostly per-advisory detail fetches).

### Habits that have paid off

- **Survey the real filesystem before writing a scanner**: the 680 non-project `Cargo.lock` files were found this way. For npm, the survey found 65 lockfiles shipped inside `node_modules` packages, 28 VS Code extensions with bundled `node_modules`, and that hidden lockfiles are unreliable.
- **Cross-check found vs. expected with an independent script.** For npm, a Python `os.walk` reimplementing the origin rule found exactly the scanner's 37 project origins. The 2 that had no packages turned out to be Next.js `.next` build output.
- **Run the release binary's table output, not just `write` and tests.** The milestone 7 `optimize_widths` underflow appeared only when rendering a table with long npm names on a narrow terminal.
- **Probe the live API before trusting docs or assumptions.** This is how we learned that GHA has no server-side version matching, that empty versions match everything, that one bad query fails a whole batch, and that pagination can return empty pages carrying tokens.
- **Diff against the previous commit**, built in a scratch `git worktree`, after any change to discovery.
- **Make anything that reaches output deterministic**: sort at construction, tie-break equal `Package`s.

### Known issues outside the milestones

- fetter's own dependencies have OSV advisories (`ring` 0.17.8, `rustls` 0.23.27, `bytes` 1.10.1, `rustix`, `crossbeam-epoch`, …), mostly via `ureq`; run `fetter --origin . audit`. A `cargo update` likely clears most of them. Not done, because dependency changes are the maintainer's call.
- README examples are stale (`-e`, top-level `site-install`); deferred to milestone 14.
- The `unsafe-libyaml` advisory (RUSTSEC-2024-0449) is resolved: `serde_yaml` was replaced with `serde-saphyr`.

---

## 1. Goals and non-goals

**Goals**

- Discover packages from multiple ecosystems: Python, Rust/crates, JS/TS/npm, GitHub Actions, and VS Code extensions. The design should make adding more ecosystems later straightforward.
- Run **one filesystem walk** that discovers every enabled ecosystem, so no search work is wasted.
- Support `scan`, `search`, `count`, `inspect`, `audit`, `derive`, and `validate` across all ecosystems.
- Send one OSV batch query covering mixed ecosystems.
- Provide one **cross-ecosystem allow list** for `validate`/`derive`.

**Non-goals for v6.0**

- `site-install` / `site-uninstall` for non-Python ecosystems. (Unpack and purge *are* generalized; see milestone 13.)
- Resolving GitHub Action SHA pins or moving tags (`@v4`) to concrete releases.
- Fetching remote action/extension sources to inspect them.

## 2. Terminology: generalizing "exe" and "site"

v5 is built around *exe → site-packages → packages*. That model doesn't carry over to most other ecosystems; you can't go from an exe to its packages in Rust or TS. v6 generalizes it:

- **Origin** (replaces *exe*): the thing that consumes and resolves a set of dependencies. For Python this is still the interpreter.
- **Site**: a concrete location that holds resolved packages for an origin. Every site has a `SiteKind`:
  - `Installed`: artifacts on disk (site-packages, node_modules, extensions dir, cargo-installed bins)
  - `Lockfile`: a resolved lockfile (Cargo.lock, package-lock.json, …)
  - `Manifest`: a declaration file that is itself the resolution (a GHA workflow yml)

| Ecosystem | Origin | Site(s) | Package source |
|---|---|---|---|
| python | interpreter exe | site-packages dirs (Installed) | `*.dist-info` (existing logic) |
| npm | dir with `package.json` and `node_modules` or a lockfile; or an install dir (a `node_modules` with no sibling `package.json`, e.g. global) | `node_modules` (Installed), else the first of `npm-shrinkwrap.json` / `package-lock.json` / `pnpm-lock.yaml` / `yarn.lock` / `bun.lock` (Lockfile) | walk of `node_modules/**/package.json` (incl. `.pnpm`); lockfile parsers (implemented) |
| cargo | dir containing `Cargo.lock` (workspace root); also `$CARGO_HOME` | `Cargo.lock` (Lockfile); `$CARGO_HOME/.crates2.json`, else `.crates.toml` (Installed bins) | TOML / JSON parse (implemented) |
| gha | repo root containing `.github/workflows` | each workflow file and local `action.yml` (Manifest) | `uses: owner/repo[/path]@ref`; skip `./local` and `docker://` |
| vscode | editor profile dir (`~/.vscode`, `~/.vscode-insiders`, `~/.cursor`, `~/.vscode-oss`, …) | its `extensions/` dir (Installed) | `extensions.json`, or each extension's `package.json` (`publisher.name@version`) |

fetter reports **both installed and lockfile** sources and records the `SiteKind`, so reports show where each package came from. For Cargo they come from different origins: projects have only lockfiles, and `$CARGO_HOME` has only installs. For npm, a project can have both; per the original decision (installed where it exists, else lockfile), `node_modules` takes precedence and the lockfile is read only when nothing is installed.

## 3. Core data model

- `enum Ecosystem { Python, Npm, Cargo, Gha, VsCode }`
  - `osv_name()`: `PyPI`, `npm`, `crates.io`, `GitHub Actions`, `VSCode`
  - `cli_name()`: `python`, `npm`, `cargo`, `gha`, `vscode`; implements clap `ValueEnum`
- `Package` (`src/package.rs`) gains `ecosystem: Ecosystem`.
  - Key normalization moves from `util::name_to_key` (PEP 503 only) to per-ecosystem rules:
    - Python: PEP 503
    - npm: lowercase
    - cargo: fold `-`/`_`
    - gha: lowercase `owner/repo`
    - vscode: lowercase `publisher.name`
- Version: `enum PackageVersion { Pep440(VersionSpec), Semver(..), GitRef(String) }`. *Still deferred*: the Cargo scanner did not need it. `VersionSpec` splits on `.` with numeric/text parts, so it orders and displays semver strings and refs well enough for scanning, but it is not sufficient for range matching. It will be needed for GHA client-side range checks (milestone 8) and for allow-list validation (milestone 11).
  - Ordering and display work for every variant.
  - `validate` needs range matching per ecosystem. Each ecosystem gets its own requirement type behind `trait Requirement { fn matches(&self, &Package) -> bool }`:
    - PEP 440: existing `VersionSpec`/`DepSpec`
    - Cargo semver
    - npm semver ranges
    - exact/glob match for GHA refs and VS Code
- `Origin { ecosystem, path: PathShared }` and `Site { ecosystem, kind: SiteKind, path: PathShared }`.
  - Both are `Hash + Eq + Ord`, sorted by path.
- `ScanFS` (`src/scan_fs.rs`) becomes:
  ```rust
  pub struct ScanFS {
      pub origin_to_sites: HashMap<Origin, Vec<Site>>,
      pub package_to_sites: HashMap<Package, Vec<Site>>,
      pub site_to_origins: HashMap<Site, Vec<Origin>>,     // derived from origin_to_sites
      pub origin_to_ems: Option<HashMap<Origin, EnvMarkerState>>,  // Python origins only
      config: ScanConfig,                  // force_usite, all_users, ecosystems, roots
      origins_hash: String,
  }
  ```
  - As implemented (milestone 2): Python-only state is the single `origin_to_ems` field rather than a `PythonExtras` struct. Wrap it in a struct if more Python-only state appears.
  - All constructors end in `ScanFS::from_parts`, which derives `site_to_origins` and sorts every inner Vec. That makes `ScanFS: PartialEq` a plain derive.
  - **Determinism rule**: every inner `Vec` is sorted at construction. HashMap iteration order must never leak into output. A v5 bug of exactly this kind was fixed in `from_exe_to_sites`.
  - Serialization uses a new v6 tuple: `(paths, origins: [(eco, path_i)], sites: [(eco, kind, path_i)], origin_to_sites: [(origin_i, [site_i])], package_to_sites: [(Package, [site_i])], config, origins_hash)`. Tables are assigned indices in sorted order. `site_to_origins` is not stored; it is rebuilt on load. The cache file prefix is `scan_fs_v6_`. The full `ScanConfig` (including ecosystems and roots) is serialized and included in the cache hash. See §5 for the `monitor-scan` form.

## 4. Scanner architecture (single walk)

As implemented in `src/eco/mod.rs` (milestone 3):

```rust
trait EcosystemScanner: Sync {
    /// Extra (dir, recursive) roots beyond home dirs: PATH bins for Python.
    fn roots(&self) -> Vec<(PathBuf, bool)> { vec![] }
    /// Origins found without walking: the default python on PATH; $CARGO_HOME.
    fn default_origins(&self) -> Vec<Origin> { vec![] }
    /// Called once per visited directory with its entries; may claim origins, prune descent, or skip named children.
    fn detect(&self, dir: &Path, items: &[DirItem]) -> Detect;   // DirItem { path, file_type }; Detect { origins, prune, skip_children }
    /// Cheap check: does a user-provided path (`--origin`) look like an Origin of this ecosystem?
    fn accepts(&self, path: &Path) -> bool;
    /// Convert a user-provided path into an Origin of this ecosystem, validating it.
    fn normalize_origin(&self, path: &Path) -> ResultDynError<Origin>;
    fn origin_to_sites(&self, origin: &Origin, config: &ScanConfig, log: FlagLog) -> Vec<Site>;
    fn site_to_packages(&self, site: &Site) -> Vec<Package>;
    /// Milestone 10: code that runs implicitly at install/startup/activation (for `inspect`). Default: none.
    fn site_to_hooks(&self, site: &Site) -> Vec<Hook> { vec![] }
}
```
- Scanners are registered in `eco::scanner_for(Ecosystem) -> Option<&'static dyn EcosystemScanner>`. `eco::scanners(&[Ecosystem])` returns the implemented ones among those given. A scanner has no `ecosystem()` method; the registry is the mapping. Adding an ecosystem means a new `eco/<name>.rs`, a `static` in `eco/mod.rs`, and an arm in `scanner_for`.
- `DirItem.file_type` comes from the directory read and does not follow symlinks. That removes the extra `is_dir`/`symlink_metadata` stats per entry that `exe_search` did.

- **Walker** (`src/walk.rs`, replacing `exe_search.rs`): `walk::discover(scanners, all_users, roots)`. Roots are walked in parallel, and recursion within a root is also parallel (rayon `par_extend` over child dirs) since milestone 4. Before that, a single large root such as `--root ~/src` was walked by one thread.
  - It keeps `get_origins_home`, `get_search_exclude_paths`, and the no-symlink-recursion rule.
  - At each directory it reads the entries once and dispatches to every enabled scanner.
  - The `pyvenv.cfg` shortcut becomes `PythonScanner::detect` returning `prune: true`.
- **Global skip list**: only `.git` and `__pycache__` (`walk::SKIP_DIR_NAMES`), plus the existing platform excludes. `node_modules`, `target` and `site-packages` are generic names that could hide other ecosystems' origins (for example a venv under a dir named `target`), so they are skipped contextually by scanners instead.
- **Prune semantics**: if any scanner returns `prune: true` for a directory, none of its children are visited by any scanner. All scanners still see the pruning directory itself. Used for venvs (Python) and `$CARGO_HOME` / `$RUSTUP_HOME` (Cargo).
- **`skip_children`** (milestone 6): named children of this directory are not visited, for any scanner, while siblings still are. Cargo uses it to skip `target` next to a `Cargo.toml`; npm should do the same for `node_modules`.
- **Pipeline**: the same shape as today's `ScanFS::from_exe_to_sites`, generalized:
  1. walk + `default_origins` → origins
  2. `origin_to_sites` (rayon)
  3. `site_to_packages` (rayon)
  4. build the three index maps (sorted)
- **Layout**: `src/eco/mod.rs` plus one file per ecosystem: `src/eco/{python,npm,cargo,gha,vscode}.rs` (python and cargo exist).
  - Python's `get_site_package_dirs`, `get_packages`, `get_origins_bin`, and venv detection are in `eco/python.rs`. Env-marker loading stays in `ScanFS::load_env_marker_state` (filtered to Python origins), because it mutates `ScanFS` state. `is_python_exe` and related helpers stay in `util.rs`, where `exe_path_normalize` also uses them.

### Hooks (generalized `inspect`)

`Hook { ecosystem, package: Option<Package>, kind, path, detail }` describes code that runs without being explicitly invoked.

| Ecosystem | Hooks |
|---|---|
| python | `.pth` files with executable lines; `sitecustomize`/`usercustomize` (existing `inspect_report.rs` logic) |
| npm | `preinstall`/`install`/`postinstall`/`prepare` scripts in installed packages' `package.json`; `hasInstallScript` in lockfiles |
| cargo | crates with `build.rs` or proc-macros, visible in `$CARGO_HOME/registry/src`; from `Cargo.lock` alone this is unknowable, so it is reported as unknown |
| gha | local composite actions only in v6.0 (remote `runs.using` would need fetching) |
| vscode | `activationEvents` containing `*` or `onStartupFinished`; `main` entry |

## 5. CLI v6

**Global flags**

- `-E, --ecosystem <LIST>`: comma-separated or repeated; default **all**. An explicitly requested ecosystem with no scanner yet prints `fetter: ecosystem `npm` is not yet supported and will be skipped` to stderr and continues.
- `--origin <PATH>…`: replaces `-e/--exe`. It deliberately has **no short flag**, because `-o` reads as "output" (and `write -o <FILE>` already means that). `-e`/`--exe` remain as **hidden aliases**, so v5 command lines and v5-installed `site-install` hooks keep working. `*` means discover. A given path's ecosystem is inferred via `eco::infer_origins`: every enabled scanner whose `accepts()` is true contributes an Origin through `normalize_origin()`, so one path can yield several (e.g. a dir with `package.json` and `Cargo.lock`). If no scanner accepts the path, or an accepting scanner cannot normalize it, that is an error. Python accepts interpreter-named paths and virtual-environment directories (`--origin .venv`).
- `--root <DIR>…`: if given, **only** these directories are walked (recursively). Home directories, scanner roots (PATH bins) and default origins (the PATH `python3`) are not added. If not given, discovery is unchanged from v5. Roots are part of the cache hash.
- `--all-users`, `--user-site` (Python only), `--cache-duration`, `--cache-directory`, `--quiet`, `--log`, `--stderr`, `--banner`: unchanged.

**Command placement** (all 16 v5 commands accounted for)

| Shared (all ecosystems, in concept) | `fetter python …` (Python only) |
|---|---|
| `scan`, `search`, `count`, `inspect`, `audit`, `derive`, `validate`, `lookup-name`, `lookup-bound`, `unpack-count`, `unpack-files`, `purge-pattern`, `purge-invalid`, `monitor-scan` | `site-install`, `site-uninstall` |

- Only `site-install` / `site-uninstall` are Python-specific in concept, because they install a Python startup hook. Unpack and purge apply to most ecosystems (npm package dirs, VS Code extension dirs, cargo-installed bins). They are shared commands, but until milestone 13 they act only on packages whose ecosystem can read artifacts (`scan_fs::supports_artifacts`, Python only). Packages from other ecosystems are silently left out.

- Ecosystem column, as implemented in milestone 4: `scan`, `search`, `audit`, `lookup-*` tables gain a leading `Ecosystem` column (blank on continuation rows in a TTY, like `Package`). Their JSON already carries `package.ecosystem`. `validate`, `inspect`, `derive` and `unpack-*` are unchanged until their milestones (10, 11), since they are Python-only today.
- `count` is now one row per enabled, implemented ecosystem: `Ecosystem,Origins,Sites,Packages`. A final `all` row is added only when there is more than one row. It used to be a vertical `Executables/Sites/Packages` list.
- `lookup-name` / `lookup-bound` (and every `fetter python …` command) force `-E python`. Passing any other ecosystem is an error until registries exist.
- Python-namespace commands keep their v5 behavior and force `-E python`.
- `lookup-name` / `lookup-bound` need a per-ecosystem `trait Registry` that lists versions. PyPI exists in `pypi_project.rs`; npm registry, crates.io, and GitHub tags come later. Until an ecosystem has a registry, the command errors clearly for it. `lookup-name` takes the ecosystem from `-E` (exactly one) or from a purl argument.
- `monitor-scan` posts a JSON array `[tenant, system_tag, scan_or_null, {"secs": u64, "nanos": u32}]`. `scan_or_null` is `null` when the scan is unchanged since the last post. Otherwise it is the ScanFS v6 tuple:
  ```
  [
    paths:            [string],                          // path dictionary
    origins:          [[ecosystem, path_i]],             // origin table, sorted by path
    sites:            [[ecosystem, kind, path_i]],       // site table, sorted by path; kind: "installed"|"lockfile"|"manifest"
    origin_to_sites:  [[origin_i, [site_i]]],
    package_to_sites: [[{ecosystem, name, key, version, direct_url}, [site_i]]],
    config:           {force_usite, all_users, ecosystems: [ecosystem], roots: [string]},
    origins_hash:     string
  ]
  ```
  `ecosystem` is the lowercase CLI name (`"python"`, `"npm"`, …), not the OSV name. Both tables use indices into `paths`. `site_to_origins` is not sent; derive it from `origin_to_sites`.

Examples:
```
fetter scan                        # everything, one walk
fetter -E npm,cargo audit
fetter -E gha scan --root ~/src
fetter --origin ./my-project count
fetter validate --bound fetter.toml
fetter python site-install --bound requirements.txt
```

### 5a. Cross-ecosystem allow list (`validate` / `derive`)

There is no cross-ecosystem standard, so fetter defines its own format: **`fetter.toml`** (name TBD). It has one table per ecosystem, and each requirement uses that ecosystem's native syntax:

```toml
[python]
requirements = ["requests>=2.31,<3", "numpy==2.1.*"]   # PEP 508, parsed by existing DepSpec

[npm]
"lodash" = "^4.17.21"          # npm range syntax

[cargo]
"serde" = "1"                  # cargo semver req

[gha]
"actions/checkout" = ["v4", "11bd71901bbe5b1630ceea73d27597364c9af683"]   # allowed refs

[vscode]
"ms-python.python" = "*"
```

- Also accepted:
  - a flat list of purl strings (`pkg:npm/lodash@4.17.21`), for interop with SBOM tooling;
  - every existing Python bound source (requirements.txt, pyproject.toml, uv/poetry/pipenv/pip-tools locks via `dep_manifest.rs` / `lock_file.rs`), applied to the Python section.
- `DepManifest` becomes a map `Ecosystem → per-ecosystem manifest`.
- `ValidationReport` (`validation_report.rs`) keeps its missing / unrequired / misdefined categories and adds an ecosystem to each record.
- `--subset` / `--superset` apply per ecosystem section. An ecosystem absent from the file is not validated, unless `--strict` is given.
- `derive` emits `fetter.toml` from a scan. `--anchor lower|upper|both` maps onto each ecosystem's syntax: npm/cargo use `>=x`, `<=x`, or `=x`, and GHA always uses the exact ref.

## 6. OSV

- `OSVPackageQuery::from_package` (`src/osv_query.rs`) uses `package.ecosystem.osv_name()` in place of the hard-coded `"PyPI"`.
- One `query_osv_batches` call takes mixed ecosystems, in chunks of 64. The cache key hashes the query JSON, which includes the ecosystem.
- **Failure is never "clean"** (milestone 5). A request error, an unparseable response (such as a 400 body), or a response whose `results` length differs from the query count is an error. It propagates to `audit` / `lookup-*` as `fetter Error: …` with exit 1, and nothing is cached. Before v6, failures became `None` (no vulnerabilities) for the whole chunk and were cached.
- **Pagination** is followed. A result may carry `next_page_token`, and follow-up requests re-send only those queries, each with `page_token`. IDs accumulate across pages and duplicates are dropped. A failure on any page fails the query, and the number of requests per batch is capped at `OSV_MAX_PAGES` (100). `page_token` is omitted when unset, so cache keys are unchanged.
  - Documented triggers: more than ~1,000 vulns for one query, or more than ~3,000 for a batch. Observed 2026-11-13: a 10-query batch returning 6,002 vulns did *not* paginate. A single Ubuntu `linux` query did (1,000 + token). A `Linux/Kernel` query returned **empty pages that still carry tokens** before finishing with no matches, so a token with zero vulns must be followed, not treated as done. Covered by mock tests and the ignored live test `test_osv_pagination_live` (about 80s).
- **Unaudited packages**: a package is queried only if `osv_query::is_osv_queryable` (currently: a non-empty version). The rest go into `AuditReport.unaudited`, which is serialized as `"unaudited"` only when non-empty. `audit` prints `fetter: N package(s) could not be audited: …` to stderr. Unaudited packages do not change the exit code.

**Observed OSV API behavior** (verified live 2026-11-13; guarded by the ignored test `osv_query::tests::test_osv_ecosystem_names_live`):
- All five `osv_name()` values (`PyPI`, `npm`, `crates.io`, `GitHub Actions`, `VSCode`) are accepted.
- **One invalid ecosystem name fails the entire batch** with HTTP 400 (`error in query at index N: invalid ecosystem`).
- **An empty `version` matches every advisory for the package**: OSV treats it as "any version". That is why `is_osv_queryable` excludes it.
- PyPI, npm and crates.io do server-side version matching (`jinja2@2.4.1`, `lodash@4.17.15`, `time@0.1.43` all return advisories).
- **GitHub Actions has no server-side version matching.** GHA advisories carry only `ECOSYSTEM` ranges (e.g. `introduced 0, fixed 46.0.1`) with no `versions` list, and OSV does not evaluate them for this ecosystem. A versioned query always returns nothing, even for versions inside an affected range (e.g. `actions/download-artifact@4.0.0` against `introduced 4.0.0, fixed 4.1.3`), with or without a `v` prefix. GHA auditing therefore needs client-side matching: query by package name without a version (which returns all advisories), fetch each advisory's `affected` ranges, and compare the ref's version locally. See milestone 8.
- VS Code: the name is accepted; whether advisories carry `versions` lists or need client-side matching is unverified (milestone 9).

## 7. Milestones

Each milestone leaves `cargo test` green and lists the main files it touches.

1. [x] **SPEC.md**: this document.
2. [x] **Core types**: `Ecosystem`, `Package.ecosystem`, `PackageVersion`, `Origin`/`Site`/`SiteKind`, `ScanFS` field renames, and v6 serialization. Python only; CLI behavior unchanged.
   Files: `package.rs`, `scan_fs.rs`, new `ecosystem.rs` and `origin_site.rs`; mechanical updates in reports.
   Done 2026-11-13. `PackageVersion` deferred (see §3). The `osv_name()` part of milestone 5 was done early, since it was a one-line change. Inspect and unpack output is now sorted; previously it followed HashMap iteration order. `Package::new(ecosystem, …)` is the general constructor; `from_name_version_durl` / `from_dist_info` remain the Python constructors.
3. [x] **Scanner trait + generic walker**; port Python into `eco/python.rs`.
   Files: `exe_search.rs` → `walk.rs`, `scan_fs.rs`, new `eco/`.
   Done 2026-11-13. `ScanFS::from_exes` keeps its name until the CLI rework; with `*` it calls `walk::discover(&eco::scanners(), …)`, and explicit paths go through `PythonScanner.normalize_origin` until `--origin` adds ecosystem inference (milestone 4). Walker tests use a mock `MarkerScanner` to exercise dispatch, pruning, skip names, excludes, and symlinks independently of Python.
   Verified against the milestone 2 commit with a full `scan` of a home directory: identical results (1586 rows, same content). Discovery is about 4.5× faster (≈25s vs ≈111–125s, warm cache, release build). The skip list is not the cause (disabling it made no difference). The gain comes from using `DirEntry::file_type()` instead of `is_dir()` + `symlink_metadata()` stats on every entry. Also fixed: `scan`/`search` rows for packages that compare equal (same name+version, different `direct_url`) were ordered by HashMap iteration. They now tie-break by sites.
4. [x] **CLI rework**: `-E`, `--origin`, `--root`, the `python` namespace, and the ecosystem column/field in all reports; document the monitor schema.
   Files: `cli.rs`, `*_report.rs`, `table.rs`, `monitor.rs`.
   Done 2026-11-13.
   - `ScanConfig` is now `Clone` (not `Copy`) and carries `ecosystems` and `roots`; both are in the cache hash and in the serialized scan.
   - The trait gained `accepts()`, and there is a new `eco::infer_origin()`. `ScanFS::from_exes` was renamed `from_origin_paths`.
   - The generated `site-install` hook now passes `--origin`.
   - Revised after review: `--origin` has no short flag (`-o` reads as output), and unpack/purge moved back to the top level as shared commands (Python-only behavior for now). Only `site-install` / `site-uninstall` are under `fetter python`.
   - The walker recurses in parallel: the full home scan went from ≈25s to ≈10.7s and `--root ~/_x/src` from ≈24.5s to ≈8s. A full home `scan` matches milestone 3 row for row once the new column is removed.
   - Not done: README examples still use `-e` and top-level `site-install` / `site-uninstall`; deferred to milestone 14. Old top-level `site-install` / `site-uninstall` get clap's generic "unrecognized subcommand" error with no hint.
5. [x] **OSV multi-ecosystem.**
   Files: `osv_query.rs`, `audit_report.rs`.
   Done 2026-11-13. See §6.
   - `AuditReport::from_packages` and `ScanFS::to_audit_report` now return `ResultDynError`.
   - The audit and lookup commands stop the spinner before propagating an error.
   - Tests cover mixed-ecosystem query JSON and result alignment, request/parse/length errors, failures not being cached, cache hits, `unaudited` partitioning, and the error path.
   - Fixed a latent test bug: `lookup_report::test_from_dep_spec_mock` mocked 1 OSV result for 3 queried versions, and the old code's silent `zip` hid the mismatch.
   - Added OSV pagination support (§6). Before, everything past the first page was silently dropped.
6. [x] **Cargo scanner**: `Cargo.lock`, `.crates2.json`.
   Files: `eco/cargo.rs`.
   Done 2026-11-13.
   - **Origins**: directories containing `Cargo.lock`, plus `$CARGO_HOME` (default `~/.cargo`) when it has an install record. `$CARGO_HOME` is found via `default_origins`, so a non-default location is covered, and is also claimed if walked.
   - **Sites**: `Cargo.lock` (Lockfile); `.crates2.json`, falling back to `.crates.toml` (Installed). All lockfile versions (v1, which has no header, through v4) share `[[package]]`.
   - **Packages**:
     - Lockfile entries **without a `source`** are workspace members or path deps (the project's own crates) and are excluded; so are `path+` installs.
     - crates.io (`registry+…crates.io-index` or `sparse+https://index.crates.io/`) has no `direct_url`.
     - git sources become `DirectURL { url, vcs_info: { vcs: git, commit_id, revision } }`, and other registries `DirectURL { url }`. They are still queried in OSV by name and version, consistent with Python packages that have a `direct_url`; see open questions.
   - **Walk**: `$CARGO_HOME` and `$RUSTUP_HOME` are pruned. On the dev machine they held 599 + 81 `Cargo.lock` files from downloaded crate sources and toolchains, none of them projects. Pruning applies to all scanners, and neither directory holds Python interpreters. `target` is skipped (via the new `Detect.skip_children`) in any directory with a `Cargo.toml`, so venvs inside Rust projects (e.g. pyo3/maturin) are still found.
   - **`--origin`**: accepts `Cargo.lock`, `Cargo.toml`, a crate or workspace dir, or a Cargo home. Members resolve upward to the workspace's `Cargo.lock`. A crate without a lockfile is an error that suggests `cargo generate-lockfile`. `eco::infer_origin` became `infer_origins`, so a path accepted by several scanners (e.g. a Tauri app dir with `package.json` and `Cargo.lock`) yields an origin for each.
   - **Isolation**: `validate`, `derive` and `purge-invalid` act only on Python packages until milestone 11. Without this, `derive` would write crates into requirements.txt and `validate` would flag every crate as Unrequired.
   - **`count`** now always shows a `cargo` row (zeros if none) plus `all`.
   - **Verified on the dev machine**:
     - Python results are unchanged (54/57/764).
     - Cargo: 27 origins (26 projects + `~/.cargo`), 1,737 crates. The 2 dependency-free projects correctly yield no packages.
     - The full scan got faster, ≈10.7s → ≈6.1s, from the pruning.
     - `audit` of this repo's lockfile returned real RustSec/GHSA advisories with details (e.g. `ring` 0.17.8 RUSTSEC-2025-0009 via `ureq` → `rustls`).
7. [x] **npm scanner**: `node_modules` walk, package-lock v1–v3, pnpm, yarn classic/berry, bun.
   Files: `eco/npm.rs`, `lock_file.rs` (YAML migration), `table.rs` (fix), `Cargo.toml`, `ci.yml`.
   Done 2026-11-13.
   - **Origins**:
     - A dir with `package.json` and either `node_modules` or a lockfile.
     - An install dir: a `node_modules` whose parent has no `package.json`. This covers global installs, nvm and volta without hard-coded paths; `/usr/local/lib`, `/usr/lib` and `/opt/homebrew/lib` under `node_modules`, `$NPM_CONFIG_PREFIX`, and `%APPDATA%\npm` come from `default_origins`.
     - Not origins: anything under an editor `extensions` dir (`.vscode`, `.cursor`, `.windsurf`, …), which is left to milestone 9.
   - **Sites**: Installed (`node_modules`) takes precedence over Lockfile.
   - **`node_modules` walk**:
     - Reads each package root's `package.json` (`name`/`version`), scoped dirs, nested `node_modules`, and pnpm's `.pnpm/*/node_modules`.
     - **Never follows symlinks**: workspace links, `npm link`, and pnpm's links into its store (which is read directly) would otherwise double-count.
     - The hidden lockfile `node_modules/.package-lock.json` is **not** used. It is an npm-only cache that can go stale, while the walk is uniform across npm, yarn and pnpm and reflects what is on disk.
   - **Lockfiles**:
     - package-lock v2/v3: the `packages` map, keys under `node_modules/`, skipping `link` entries. v1: the nested `dependencies` tree.
     - pnpm v5 (`/name/1.0.0_peer`), v6 (`/name@1.0.0(peer)`) and v9 (`name@1.0.0`) keys, using explicit `name`/`version` fields when present.
     - yarn classic (custom format) and berry (YAML, `resolution` protocols).
     - Excluded: the root, workspace members/importers, and `file:` / `link:` / `portal:` / `workspace:`.
     - Provenance: the public registry (registry.npmjs.org, registry.yarnpkg.com) has no `direct_url`. Git has a `DirectURL` with a commit; other registries a `DirectURL` with the tarball URL. A URL in place of a version gives an empty version (→ unaudited).
   - **Walk**: projects skip `node_modules`, `.next`, `.nuxt` and `.svelte-kit` (framework build output contains `package.json` + `node_modules` stubs). An install dir reached by the walk is claimed and pruned.
   - **Packages are deduplicated per site**; `ScanFS::from_parts` now also dedups sites and origins after sorting.
   - **YAML**: `serde_yaml` (deprecated, via `unsafe-libyaml`, RUSTSEC-2024-0449) was replaced with **`serde-saphyr` 2.1.1**, parsing into `serde_json::Value`. `lock_file.rs` (pixi) was migrated. `serde-saphyr` requires Rust 1.89, so **MSRV was raised from 1.88 to 1.89** (CI matrix, plus `rust-version` in `Cargo.toml`).
   - **Bug fixed** (predates v6): `table::optimize_widths` underflowed (`width - reduction`) when the terminal was narrower than the non-ellipsisable columns. Release builds then panicked with "Formatting argument out of range"; long npm names triggered it. Now saturating, with a testable `optimize_widths_for`.
   - **Bun** (added in a follow-up): `bun.lock` (text, the default since Bun 1.2) is last in lockfile precedence.
     - The format is JSON with trailing commas, which `serde_json` rejects, so `strip_trailing_commas` removes them first (string- and escape-aware).
     - Each `packages` entry is an array whose first element is `name@spec`. Keys are resolution paths (e.g. `discord.js/undici` for a nested duplicate; 379 of 404 real entries differ from the name) and are not used.
     - Registry entries are `["name@1.0.0", registry, info, integrity]`, where an empty registry is the default.
     - `workspace:`, `file:`, `link:` and `root:` are excluded. `github:`, `git+` and tarball specs keep provenance with an empty version (→ unaudited).
     - The binary `bun.lockb` (pre-1.2, no published format) is **not** supported. Convert with `bun install --save-text-lockfile`, or rely on the `node_modules` walk.
     - Verified: the 4 bun-only projects under `~/.claude/plugins` now scan with package counts exactly matching an independent tally of unique `name@version` entries (114/95/99/95), and `audit` returns advisories (e.g. `@modelcontextprotocol/sdk` 1.27.1).
   - **Verified on the dev machine**:
     - npm: 36 origins, 5,803 packages.
     - An independent `os.walk` cross-check found the same 37 project origins; the 2 extra were `.next` build output, since skipped.
     - Python unchanged; full scan ≈6.1s → ≈4.0s (projects' `node_modules` are no longer walked by the Python scanner).
     - `scan` output deterministic.
     - `-E npm audit` over ~91 OSV batches: 372 packages with advisories.
8. [ ] **GHA scanner**; replace the deprecated `serde_yaml` first. Requires **client-side version matching** (§6): OSV returns nothing for versioned GHA queries. Approach: an ecosystem capability (e.g. `Ecosystem::osv_server_side_versions() -> bool`). For ecosystems without it, query `/v1/query` by name only, then evaluate each advisory's `affected[].ranges` (`ECOSYSTEM`, semver-like with an optional `v` prefix) against the ref. Floating refs (`@v4`) and SHAs cannot be placed in a range without resolving them to a release (GitHub API); report them as `unaudited` until that is implemented.
   Files: `eco/gha.rs`, `osv_query.rs`, `osv_vulns.rs` (parse `affected`), `Cargo.toml`.
9. [ ] **VS Code scanner.**
   Files: `eco/vscode.rs`.
10. [ ] **Hooks**: generalize `inspect` via `site_to_hooks`. Port Python first, then npm, vscode, and cargo.
    Files: `inspect_report.rs`, `site_customize.rs`, `eco/*`.
11. [ ] **Allow list**: `fetter.toml` + purl parsing, per-ecosystem `Requirement` (semver crates), generalized `validate` and `derive`.
    Files: `dep_manifest.rs`, `dep_spec.rs`, `validation_report.rs`, new `allow_list.rs`.
12. [ ] **Registries** for `lookup-name` / `lookup-bound`: npm, crates.io, GitHub tags.
    Files: `pypi_project.rs` → `registry/`, `lookup_report.rs`.
13. [ ] **Artifacts**: generalize `unpack-count`, `unpack-files`, `purge-pattern` and `purge-invalid`. Add an `EcosystemScanner` method that lists a package's installed artifacts (files/dirs). Python uses dist-info `RECORD`; npm and VS Code use the package/extension dir; cargo uses `.crates2.json` bins. Remove `supports_artifacts`. Lockfile/manifest-only packages have no artifacts and are reported as such.
    Files: `unpack_report.rs`, `scan_fs.rs`, `eco/*`.
14. [ ] **Release**: README/docs, crate description and keywords, version 6.0.0, changelog from section 11.

## 8. Testing strategy

- Fixtures, as practiced: inline string constants in each scanner's test module (e.g. `LOCK_V4`, `CRATES2` in `eco/cargo.rs`), written to `tempfile` dirs when a test needs files on disk. This keeps a fixture next to the parser it exercises. Use `tests/fixtures/<eco>/` only for large real-world samples.
- Scanner unit tests: parsers, `detect`, `accepts`/`normalize_origin`, `origin_to_sites`, and `site_to_packages` against those fixtures; `site_to_hooks` from milestone 10.
- Walker tests: a `tempfile` tree with mixed project types. They assert the detected origins and the pruning.
- Determinism: serialize-twice comparisons (like `test_from_exes_equality_comparison`), now covering multi-site packages in every ecosystem.
- OSV: `UreqClientMock` (fixed response per URL prefix) for single-response cases; the test-only `SeqClient` in `osv_query.rs` (queued responses, recorded request bodies) for pagination. Live checks are `#[ignore]` tests.
- Discovery changes are also verified manually against the previous commit and the baselines in "Picking this up".
- Existing Python tests stay green across milestones 2–4; only mechanical constructor changes are allowed.

## 9. Open questions

- GHA: should SHA pins and `@v4` major-tag refs be resolved to concrete versions? That needs the GitHub API and a rate-limit/token story. Until then they go in `unaudited` (§6), so this decides how much of a typical workflow is auditable.
- Table output at scale: a full home `scan` now has thousands of npm rows. Consider grouping by origin, or a `--summary` view.
- VS Code extensions' bundled `node_modules` (28 on the dev machine): should milestone 9 report them as npm packages attributed to the extension, or only the extension itself?
- Should `--root` include `/` under `--all-users`, or stay with home dirs only?
- Packages with a `direct_url` (Python VCS/local installs; Cargo git and alternative-registry crates) are queried in OSV by name, as if from the public registry. For forks or private crates that share a public name, this can produce false positives. Should they go in `unaudited` instead? Today's behavior favors not missing advisories.
- Cargo: `vendor/` directories (from `cargo vendor`) can contain crates that ship their own `Cargo.lock`. These would be detected as projects; consider skipping `vendor` when `.cargo/config.toml` sets `[source.vendored-sources]`.
- Allow-list file name and format. Should purl `vers:` ranges be supported alongside native range syntax?
- npm range semantics (for milestone 11 validation): use the `node-semver` crate, or implement the subset fetter needs? Related: milestone 8 needs semver ordering for GHA ranges; choose a crate that can serve both if possible.
- `Package`'s `Ord` (ecosystem, lowercase name, version) is inconsistent with its derived `Eq`/`Hash` (which include `key` and `direct_url`): distinct packages can compare `Equal`. Reports work around this with tie-breaks. Should `Ord` include `direct_url`, or should identity drop it?
- Should the old top-level `site-install` / `site-uninstall` remain as hidden aliases for migration? Today they fail with clap's generic "unrecognized subcommand" error.

## 10. Decision log

- 2026-11-13: The ecosystem is selected with a global `-E/--ecosystem` flag. Python-only commands live under `fetter python …`.
- 2026-11-13: `--origin` has no short flag (`-o` is associated with output). `-e`/`--exe` are kept as hidden aliases for v5 compatibility.
- 2026-11-13: Unpack and purge are shared commands in concept; only `site-install` / `site-uninstall` are Python-specific. Generalizing them is milestone 13.
- 2026-11-13: npm/cargo use both installed artifacts and lockfiles, and record the `SiteKind`. (Precisely: installed where it exists, else lockfile; see the npm entry.)
- 2026-11-13: The default scan covers all ecosystems in a single walk.
- 2026-11-13: `inspect`, `validate`, and `derive` are generalized to all ecosystems (hooks; cross-ecosystem allow list).
- 2026-11-13: "exe" generalizes to **Origin**; "site" keeps its name, with a `SiteKind`.
- 2026-11-13: npm sources follow the original choice, "installed where it exists, else lockfile". This corrects the drifted "report both" wording above for npm. Hidden lockfiles are not used.
- 2026-11-13: npm does not report packages inside editor extension dirs; extensions (and their bundled dependencies) belong to the VS Code scanner (milestone 9).
- 2026-11-13: Support Bun's text `bun.lock` (the current default); not the binary `bun.lockb`.
- 2026-11-13: YAML via `serde-saphyr` (pure Rust, maintained), replacing deprecated `serde_yaml`; MSRV raised to 1.89 to allow it.
- 2026-11-13: OSV supports VS Code extensions under the ecosystem name `VSCode` (https://osv.dev/list?q=&ecosystem=VSCode), so VS Code packages are fully auditable.

## 11. Breaking changes (for the v6 changelog)

- **MSRV is Rust 1.89** (was 1.88), required by `serde-saphyr`.
- `-e/--exe` → `--origin` (no short flag; old names kept as hidden aliases).
- `audit` / `lookup-*` fail (exit 1) when OSV cannot be queried, instead of reporting no vulnerabilities. Audit JSON may include an `unaudited` list.
- Library API: `AuditReport::from_packages` returns `Result`; `AuditReport` has a new `unaudited` field.
- `count` output is a per-ecosystem table (`Ecosystem,Origins,Sites,Packages`), not `Executables/Sites/Packages` rows.
- `scan`, `search`, `audit` and `lookup-*` tables have a new leading `Ecosystem` column.
- `--root` replaces, rather than adds to, the default discovery locations.
- `site-install` and `site-uninstall` move under `fetter python`.
- The default scan includes all ecosystems, not just Python.
- The JSON output of every report gains an `ecosystem` field; the `monitor-scan` payload changes shape.
- `ScanFS` public fields are renamed (`exe_to_sites` → `origin_to_sites`, `site_to_exes` → `site_to_origins`, `exe_to_ems` → `origin_to_ems`); their value types are now `Origin`/`Site`, not `PathShared`.
- `Package` gains `ecosystem`, so its JSON (e.g. in `audit` output) gains an `"ecosystem"` field. `Package` ordering is by ecosystem first.
- The cache format changes (`scan_fs_v6_` prefix); v5 caches are ignored.
