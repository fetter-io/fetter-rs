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

```rust
trait EcosystemScanner: Sync {
    fn ecosystem(&self) -> Ecosystem;
    /// Called once per visited directory with its entries; may claim origins and prune descent.
    fn detect(&self, dir: &Path, entries: &[DirEntry]) -> Detect;   // Detect { origins, prune: bool }
    /// Origins found without walking: PATH bins, ~/.vscode, $CARGO_HOME, default python.
    fn fixed_origins(&self, config: &ScanConfig) -> Vec<Origin>;
    fn origin_to_sites(&self, origin: &Origin, config: &ScanConfig, log: FlagLog) -> Vec<Site>;
    fn site_to_packages(&self, site: &Site) -> Vec<Package>;
    /// Code that runs implicitly at install/startup/activation (for `inspect`). Default: none.
    fn site_to_hooks(&self, site: &Site) -> Vec<Hook> { vec![] }
}
```

- **Walker**: `exe_search::find_exe_inner` turns into a generic parallel walker.
  - It keeps `get_origins_home`, `get_search_exclude_paths`, and the no-symlink-recursion rule.
  - At each directory it reads the entries once and dispatches to every enabled scanner.
  - The `pyvenv.cfg` shortcut becomes `PythonScanner::detect` returning `prune: true`.
- **Global prune list**: `node_modules` (npm reads it from the project root), `target`, `.git`, `site-packages`, `__pycache__`, plus the existing platform excludes.
- **Pipeline**: the same shape as today's `ScanFS::from_exe_to_sites`, generalized:
  1. walk + `fixed_origins` → origins
  2. `origin_to_sites` (rayon)
  3. `site_to_packages` (rayon)
  4. build the three index maps (sorted)
- **Layout**: `src/eco/mod.rs` plus one file per ecosystem: `src/eco/{python,npm,cargo,gha,vscode}.rs`.
  - Python's `get_site_package_dirs`, `get_packages`, and env-marker loading move into `eco/python.rs`.

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

- `-E, --ecosystem <LIST>`: comma-separated or repeated; default **all**.
- `-o, --origin <PATH>…`: replaces `-e/--exe`. `*` means discover. A given path's ecosystem is inferred from its type (an interpreter, a `package.json` dir, a `Cargo.lock`, …) or follows `-E`.
- `--root <DIR>…`: search roots for project-type origins; defaults to home dirs, matching today's home walk.
- `--all-users`, `--user-site` (Python only), `--cache-duration`, `--cache-directory`, `--quiet`, `--log`, `--stderr`, `--banner`: unchanged.

**Command placement** (all 16 v5 commands accounted for)

| Shared (all ecosystems) | `fetter python …` (Python only) |
|---|---|
| `scan`, `search`, `count`, `inspect`, `audit`, `derive`, `validate`, `lookup-name`, `lookup-bound`, `monitor-scan` | `site-install`, `site-uninstall`, `purge-pattern`, `purge-invalid`, `unpack-count`, `unpack-files` |

- Every report gains an ecosystem column (table) or field (JSON). `count` reports per ecosystem, plus a total.
- Python-namespace commands keep their v5 behavior and force `-E python`. Unpack and purge are candidates to generalize later, since npm package dirs can be unpacked and removed the same way.
- `lookup-name` / `lookup-bound` need a per-ecosystem `trait Registry` that lists versions. PyPI exists in `pypi_project.rs`; npm registry, crates.io, and GitHub tags come later. Until an ecosystem has a registry, the command errors clearly for it. `lookup-name` takes the ecosystem from `-E` (exactly one) or from a purl argument.
- The `monitor-scan` payload changes shape; its new JSON schema is documented here once milestone 4 lands.

Examples:
```
fetter scan                        # everything, one walk
fetter -E npm,cargo audit
fetter -E gha scan --root ~/src
fetter -o ./my-project count
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
- One `query_osv_batches` call takes mixed ecosystems. The cache key already hashes the query JSON, so no change is needed there.
- Packages whose ecosystem has no OSV mapping are excluded from the query and reported as "not auditable".
- GHA refs that are SHAs are sent as-is; see open questions.

## 7. Milestones

Each milestone leaves `cargo test` green and lists the main files it touches.

1. [x] **SPEC.md**: this document.
2. [x] **Core types**: `Ecosystem`, `Package.ecosystem`, `PackageVersion`, `Origin`/`Site`/`SiteKind`, `ScanFS` field renames, and v6 serialization. Python only; CLI behavior unchanged.
   Files: `package.rs`, `scan_fs.rs`, new `ecosystem.rs` and `origin_site.rs`; mechanical updates in reports.
   Done 2026-11-13. `PackageVersion` deferred (see §3). The `osv_name()` part of milestone 5 was done early, since it was a one-line change. Inspect and unpack output is now sorted; previously it followed HashMap iteration order. `Package::new(ecosystem, …)` is the general constructor; `from_name_version_durl` / `from_dist_info` remain the Python constructors.
3. [ ] **Scanner trait + generic walker**; port Python into `eco/python.rs`.
   Files: `exe_search.rs` → walker, `scan_fs.rs`, new `eco/`.
4. [ ] **CLI rework**: `-E`, `-o`, `--root`, the `python` namespace, and the ecosystem column/field in all reports; document the monitor schema.
   Files: `cli.rs`, `*_report.rs`, `table.rs`, `monitor.rs`.
5. [ ] **OSV multi-ecosystem.** (`osv_name()` in queries is done; remaining: exclude un-auditable packages, mixed-ecosystem mock test, ecosystem in audit table.)
   Files: `osv_query.rs`, `audit_report.rs`.
6. [ ] **Cargo scanner**: `Cargo.lock`, `.crates2.json`.
   Files: `eco/cargo.rs`.
7. [ ] **npm scanner**: `.package-lock.json`, `node_modules` walk, package-lock v1–v3, pnpm, yarn classic/berry.
   Files: `eco/npm.rs`.
8. [ ] **GHA scanner**; replace the deprecated `serde_yaml` first.
   Files: `eco/gha.rs`, `Cargo.toml`.
9. [ ] **VS Code scanner.**
   Files: `eco/vscode.rs`.
10. [ ] **Hooks**: generalize `inspect` via `site_to_hooks`. Port Python first, then npm, vscode, and cargo.
    Files: `inspect_report.rs`, `site_customize.rs`, `eco/*`.
11. [ ] **Allow list**: `fetter.toml` + purl parsing, per-ecosystem `Requirement` (semver crates), generalized `validate` and `derive`.
    Files: `dep_manifest.rs`, `dep_spec.rs`, `validation_report.rs`, new `allow_list.rs`.
12. [ ] **Registries** for `lookup-name` / `lookup-bound`: npm, crates.io, GitHub tags.
    Files: `pypi_project.rs` → `registry/`, `lookup_report.rs`.
13. [ ] **Release**: README/docs, crate description and keywords, version 6.0.0, changelog from section 11.

## 8. Testing strategy

- Fixtures go under `tests/fixtures/<eco>/`: sample lockfiles of each format, a `node_modules` stub, workflow yml, `.crates2.json`, and an extensions dir.
- Scanner unit tests: `site_to_packages` and `site_to_hooks` run against the fixtures.
- Walker tests: a `tempfile` tree with mixed project types. They assert the detected origins and the pruning.
- Determinism: serialize-twice comparisons (like `test_from_exes_equality_comparison`), now covering multi-site packages in every ecosystem.
- OSV: `UreqClientMock` with a mixed-ecosystem batch.
- Existing Python tests stay green across milestones 2–4; only mechanical constructor changes are allowed.

## 9. Open questions

- GHA: should SHA pins and `@v4` major-tag refs be resolved to concrete versions? That needs the GitHub API and a rate-limit/token story.
- npm: report transitive dependencies from nested `node_modules` by default? How should table output stay usable with thousands of packages (group by origin? collapse lockfile-only?)
- YAML parser to replace the deprecated `serde_yaml` (e.g. `serde_yaml_ng`, `saphyr`).
- Should `--root` include `/` under `--all-users`, or stay with home dirs only?
- Allow-list file name and format. Should purl `vers:` ranges be supported alongside native range syntax?
- npm range semantics: use the `node-semver` crate, or implement the subset fetter needs?
- Should the Python-only namespace also accept `-E python` on shared commands as an alias, to ease migration?

## 10. Decision log

- 2026-11-13: The ecosystem is selected with a global `-E/--ecosystem` flag. Python-only commands live under `fetter python …`.
- 2026-11-13: npm/cargo use both installed artifacts and lockfiles, and record the `SiteKind`.
- 2026-11-13: The default scan covers all ecosystems in a single walk.
- 2026-11-13: `inspect`, `validate`, and `derive` are generalized to all ecosystems (hooks; cross-ecosystem allow list).
- 2026-11-13: "exe" generalizes to **Origin**; "site" keeps its name, with a `SiteKind`.
- 2026-11-13: OSV supports VS Code extensions under the ecosystem name `VSCode` (https://osv.dev/list?q=&ecosystem=VSCode), so VS Code packages are fully auditable.

## 11. Breaking changes (for the v6 changelog)

- `-e/--exe` → `-o/--origin`.
- `site-install`, `site-uninstall`, `purge-pattern`, `purge-invalid`, `unpack-count`, and `unpack-files` move under `fetter python`.
- The default scan includes all ecosystems, not just Python.
- The JSON output of every report gains an `ecosystem` field; the `monitor-scan` payload changes shape.
- `ScanFS` public fields are renamed (`exe_to_sites` → `origin_to_sites`, `site_to_exes` → `site_to_origins`, `exe_to_ems` → `origin_to_ems`); their value types are now `Origin`/`Site`, not `PathShared`.
- `Package` gains `ecosystem`, so its JSON (e.g. in `audit` output) gains an `"ecosystem"` field. `Package` ordering is by ecosystem first.
- The cache format changes (`scan_fs_v6_` prefix); v5 caches are ignored.
