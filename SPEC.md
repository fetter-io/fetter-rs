# fetter v6 Specification: Multi-Ecosystem Support

Status: **draft**. This is the working spec for the v6 major release. It is meant to be read at the start of each work session and updated at the end: tick milestones, record decisions, and close open questions.

## How to use this document

- Section 7 (Milestones) is the source of truth for progress. Each milestone must leave `cargo test` green.
- When a decision is made, move it out of section 9 (Open questions) into section 10 (Decision log) with the date.
- Breaking changes are allowed (v6 is a major release), but every one must be listed in section 11 (Breaking changes) for the changelog.

---

## 1. Goals and non-goals

**Goals**

- Discover packages from multiple ecosystems: Python, Rust/crates, JS/TS/npm, GitHub Actions, and VS Code extensions. The design should make adding more ecosystems later straightforward.
- Run **one filesystem walk** that discovers every enabled ecosystem, so no search work is wasted.
- Support `scan`, `search`, `count`, `inspect`, `audit`, `derive`, and `validate` across all ecosystems.
- Send one OSV batch query covering mixed ecosystems.
- Provide one **cross-ecosystem allow list** for `validate`/`derive`.

**Non-goals for v6.0**

- `purge-*` and `site-install` for non-Python ecosystems.
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
| npm | project dir containing `package.json` | `node_modules` (Installed), else `package-lock.json` / `pnpm-lock.yaml` / `yarn.lock` (Lockfile) | `node_modules/.package-lock.json`, or a walk of `node_modules/**/package.json`; lockfile parsers |
| cargo | dir containing `Cargo.lock` (workspace root); also `$CARGO_HOME` | `Cargo.lock` (Lockfile); `$CARGO_HOME/.crates2.json` (Installed bins) | TOML / JSON parse |
| gha | repo root containing `.github/workflows` | each workflow file and local `action.yml` (Manifest) | `uses: owner/repo[/path]@ref`; skip `./local` and `docker://` |
| vscode | editor profile dir (`~/.vscode`, `~/.vscode-insiders`, `~/.cursor`, `~/.vscode-oss`, …) | its `extensions/` dir (Installed) | `extensions.json`, or each extension's `package.json` (`publisher.name@version`) |

For npm and cargo, fetter reports **both installed and lockfile** sources. It prefers Installed when it exists for an origin, falls back to Lockfile, and always records the `SiteKind` so reports show where each package came from.

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
- Version: `enum PackageVersion { Pep440(VersionSpec), Semver(..), GitRef(String) }`. *Deferred until a scanner needs it (milestone 6+).* `VersionSpec` splits on `.` with numeric/text parts, so it already orders and displays semver strings and refs well enough for scanning; it is not sufficient for range matching.
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
      config: ScanConfig,                  // will gain `ecosystems: EcosystemSet`, `roots` (milestone 3/4)
      origins_hash: String,
  }
  ```
  - As implemented (milestone 2): Python-only state is the single `origin_to_ems` field rather than a `PythonExtras` struct. Wrap it in a struct if more Python-only state appears.
  - All constructors end in `ScanFS::from_parts`, which derives `site_to_origins` and sorts every inner Vec. That makes `ScanFS: PartialEq` a plain derive.
  - **Determinism rule**: every inner `Vec` is sorted at construction. HashMap iteration order must never leak into output. A v5 bug of exactly this kind was fixed in `from_exe_to_sites`.
  - Serialization uses a new v6 tuple: `(paths, origins: [(eco, path_i)], sites: [(eco, kind, path_i)], origin_to_sites: [(origin_i, [site_i])], package_to_sites: [(Package, [site_i])], force_usite, all_users, origins_hash)`. Tables are assigned indices in sorted order. `site_to_origins` is not stored; it is rebuilt on load. The cache file prefix is `scan_fs_v6_`. The ecosystem set will be added to the hash when it lands in `ScanConfig`.

## 4. Scanner architecture (single walk)

As implemented in `src/eco/mod.rs` (milestone 3):

```rust
trait EcosystemScanner: Sync {
    /// Extra (dir, recursive) roots beyond home dirs: PATH bins for Python; later ~/.vscode, $CARGO_HOME.
    fn roots(&self) -> Vec<(PathBuf, bool)> { vec![] }
    /// Origins found without walking: the default python on PATH.
    fn default_origins(&self) -> Vec<Origin> { vec![] }
    /// Called once per visited directory with its entries; may claim origins and prune descent.
    fn detect(&self, dir: &Path, items: &[DirItem]) -> Detect;   // DirItem { path, file_type }; Detect { origins, prune }
    /// Convert a user-provided path (`--origin`) into an Origin of this ecosystem.
    fn normalize_origin(&self, path: &Path) -> ResultDynError<Origin>;
    fn origin_to_sites(&self, origin: &Origin, config: &ScanConfig, log: FlagLog) -> Vec<Site>;
    fn site_to_packages(&self, site: &Site) -> Vec<Package>;
    /// Milestone 10: code that runs implicitly at install/startup/activation (for `inspect`). Default: none.
    fn site_to_hooks(&self, site: &Site) -> Vec<Hook> { vec![] }
}
```
- Scanners are registered in `eco::scanner_for(Ecosystem) -> Option<&'static dyn EcosystemScanner>`; `eco::scanners()` returns all implemented ones. A scanner has no `ecosystem()` method; the registry is the mapping.
- `DirItem.file_type` comes from the directory read and does not follow symlinks. That removes the extra `is_dir`/`symlink_metadata` stats per entry that `exe_search` did.

- **Walker** (`src/walk.rs`, replacing `exe_search.rs`): `walk::discover(scanners, all_users, roots)`. Roots are walked in parallel, and recursion within a root is also parallel (rayon `par_extend` over child dirs) since milestone 4. Before that, a single large root such as `--root ~/src` was walked by one thread.
  - It keeps `get_origins_home`, `get_search_exclude_paths`, and the no-symlink-recursion rule.
  - At each directory it reads the entries once and dispatches to every enabled scanner.
  - The `pyvenv.cfg` shortcut becomes `PythonScanner::detect` returning `prune: true`.
- **Global skip list**: only `.git` and `__pycache__` (`walk::SKIP_DIR_NAMES`), plus the existing platform excludes. `node_modules`, `target` and `site-packages` are generic names that could hide other ecosystems' origins (for example a venv under a dir named `target`). They are left to scanner `detect` pruning, or to a future per-scanner `skip_children` in `Detect` when npm/cargo land.
- **Prune semantics**: if any scanner returns `prune: true` for a directory, none of its children are visited by any scanner. All scanners still see the pruning directory itself.
- **Pipeline**: the same shape as today's `ScanFS::from_exe_to_sites`, generalized:
  1. walk + `default_origins` → origins
  2. `origin_to_sites` (rayon)
  3. `site_to_packages` (rayon)
  4. build the three index maps (sorted)
- **Layout**: `src/eco/mod.rs` plus one file per ecosystem: `src/eco/{python,npm,cargo,gha,vscode}.rs`.
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
- `--origin <PATH>…`: replaces `-e/--exe`. It deliberately has **no short flag**, because `-o` reads as "output" (and `write -o <FILE>` already means that). `-e`/`--exe` remain as **hidden aliases**, so v5 command lines and v5-installed `site-install` hooks keep working. `*` means discover. A given path's ecosystem is inferred by trying the enabled scanners' `accepts()` in canonical order, then `normalize_origin()`; if no scanner claims the path, that is an error. Python accepts interpreter-named paths and virtual-environment directories (`--origin .venv`).
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
6. [ ] **Cargo scanner**: `Cargo.lock`, `.crates2.json`.
   Files: `eco/cargo.rs`.
7. [ ] **npm scanner**: `.package-lock.json`, `node_modules` walk, package-lock v1–v3, pnpm, yarn classic/berry.
   Files: `eco/npm.rs`.
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

- Fixtures go under `tests/fixtures/<eco>/`: sample lockfiles of each format, a `node_modules` stub, workflow yml, `.crates2.json`, and an extensions dir.
- Scanner unit tests: `site_to_packages` and `site_to_hooks` run against the fixtures.
- Walker tests: a `tempfile` tree with mixed project types. They assert the detected origins and the pruning.
- Determinism: serialize-twice comparisons (like `test_from_exes_equality_comparison`), now covering multi-site packages in every ecosystem.
- OSV: `UreqClientMock` with a mixed-ecosystem batch.
- Existing Python tests stay green across milestones 2–4; only mechanical constructor changes are allowed.

## 9. Open questions

- GHA: should SHA pins and `@v4` major-tag refs be resolved to concrete versions? That needs the GitHub API and a rate-limit/token story. Until then they go in `unaudited` (§6), so this decides how much of a typical workflow is auditable.
- npm: report transitive dependencies from nested `node_modules` by default? How should table output stay usable with thousands of packages (group by origin? collapse lockfile-only?)
- YAML parser to replace the deprecated `serde_yaml` (e.g. `serde_yaml_ng`, `saphyr`).
- Should `--root` include `/` under `--all-users`, or stay with home dirs only?
- Allow-list file name and format. Should purl `vers:` ranges be supported alongside native range syntax?
- npm range semantics: use the `node-semver` crate, or implement the subset fetter needs?
- `Package`'s `Ord` (ecosystem, lowercase name, version) is inconsistent with its derived `Eq`/`Hash` (which include `key` and `direct_url`): distinct packages can compare `Equal`. Reports work around this with tie-breaks. Should `Ord` include `direct_url`, or should identity drop it?
- Should the Python-only namespace also accept `-E python` on shared commands as an alias, to ease migration?

## 10. Decision log

- 2026-11-13: The ecosystem is selected with a global `-E/--ecosystem` flag. Python-only commands live under `fetter python …`.
- 2026-11-13: `--origin` has no short flag (`-o` is associated with output). `-e`/`--exe` are kept as hidden aliases for v5 compatibility.
- 2026-11-13: Unpack and purge are shared commands in concept; only `site-install` / `site-uninstall` are Python-specific. Generalizing them is milestone 13.
- 2026-11-13: npm/cargo use both installed artifacts and lockfiles, and record the `SiteKind`.
- 2026-11-13: The default scan covers all ecosystems in a single walk.
- 2026-11-13: `inspect`, `validate`, and `derive` are generalized to all ecosystems (hooks; cross-ecosystem allow list).
- 2026-11-13: "exe" generalizes to **Origin**; "site" keeps its name, with a `SiteKind`.
- 2026-11-13: OSV supports VS Code extensions under the ecosystem name `VSCode` (https://osv.dev/list?q=&ecosystem=VSCode), so VS Code packages are fully auditable.

## 11. Breaking changes (for the v6 changelog)

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
