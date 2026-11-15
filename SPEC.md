# fetter v5 Specification: Multi-Ecosystem Support

Status: **draft**. This is the working spec for the v5 major release. It is meant to be read at the start of each work session and updated at the end: tick milestones, record decisions, and close open questions.

## How to use this document

- Section 7 (Milestones) is the source of truth for progress. Each milestone must leave `cargo test` green.
- When a decision is made, move it out of section 9 (Open questions) into section 10 (Decision log) with the date.
- Breaking changes are allowed (v5 is a major release), but every one must be listed in section 11 (Breaking changes) for the changelog.
- Update "Picking this up" (next section) at the end of every session.

## Picking this up

*Last updated 2026-11-15, at the end of milestone 26 (versioned `monitor-scan` payload).*

### Where we are

- **Branch** `207/polye`. **All milestones (1–26) are done** (§7); milestones 15–17 added Go, Ruby and PHP; milestone 18 resolves GHA SHA pins in `audit`; milestone 19 added pre-commit and milestone 20 Swift, for ten ecosystems; milestone 21 added package URLs (purl) and SBOM input, milestone 22 VERS version ranges, milestone 23 CycloneDX export, milestone 24 pre-commit `additional_dependencies`, milestone 25 browser extensions (`chromium`, `firefox`), for twelve ecosystems, and milestone 26 a versioned, forward-compatible `monitor-scan` payload that still reads fetter 3–4 posts. The version is 5.0.0 and the README documents v5. What remains is the release itself, which the maintainer does (see Next), plus the follow-ups in §9.
- **Milestones 23–26 may not be committed yet**: check `git status` (new: `src/cyclonedx.rs`, `src/eco/{browser,browser_version,chromium,firefox}.rs`, `src/transmit.rs`, `tests/fixtures/monitor/`); milestones 1–22 are committed.
- **Health**: `cargo fmt --check` and `cargo clippy --all-targets` are clean; `cargo test` reports 537 passed and 6 ignored. The ignored tests hit live services:
  - `test_osv_ecosystem_names_live`
  - `test_osv_pagination_live` (≈80s)
  - `test_query_pypi_project_live`
  - `test_from_dep_manifest_live`
  - `test_query_client_side_live` (GHA client-side matching against real advisories)
  - `test_list_versions_live` (npm, crates.io index, `git ls-remote`, Open VSX)

  Run them with `cargo test <name> -- --ignored`. `tests/windows_session.rs` runs only on Windows and has not been run since milestone 4 changed `count` output and `--origin`. It was updated by hand; verify it in CI.
- **Implemented scanners**: all of them: `src/eco/{python,npm,cargo,gha,vscode,go,ruby,php,precommit,swift}.rs`. The "not yet supported" warning in `cli::resolve_ecosystems` is now unreachable; keep it for future ecosystems.
- **Python-only isolation in shared commands** (remove as each generalizes):
  - `purge-*` → `scan_fs::supports_purge` (Python only, by choice; see §9). `unpack-*` is general.

### Code map

| Concern | Where |
|---|---|
| Ecosystem enum, OSV names, key normalization | `src/ecosystem.rs` |
| `Origin`, `Site`, `SiteKind` | `src/origin_site.rs` |
| Scanner trait, `Detect`, `DirItem`, registry, `infer_origins` | `src/eco/mod.rs` |
| Per-ecosystem scanners | `src/eco/{python,cargo,npm,gha,vscode,go,ruby,php,precommit,swift}.rs`; the shared editor dir list is `eco::EDITOR_DIRS` |
| Client-side OSV matching (`SemverLike`, `RefKind`, range evaluation, by-name queries) | `src/osv_match.rs` |
| YAML parsing (into `serde_json::Value`) | `serde_saphyr::from_str`; see `eco/npm.rs`, `lock_file.rs` |
| Table rendering, column fitting | `src/table.rs` (`optimize_widths_for` is the testable core) |
| Allow list (`fetter.toml` parse/derive/write, `ReqEntry` matching) | `src/allow_list.rs`; validation of non-Python sections in `ValidationReport::records_from_section`; entry point `ScanFS::to_allow_list_validation` |
| Artifacts (`unpack-*`, `purge-*`): `Artifacts`, `EcosystemScanner::site_to_artifacts` | `src/eco/mod.rs` (`Artifacts::from_dirs`); per-ecosystem in `eco/{python,npm,vscode,cargo}.rs`; report and removal in `src/unpack_report.rs` |
| Hooks (`inspect`): `Hook`, `EcosystemScanner::site_to_hooks` | `src/eco/mod.rs`; per-ecosystem in `eco/{python,npm,vscode}.rs`; report in `src/inspect_report.rs` |
| Single filesystem walk (parallel; prune / skip_children / excludes) | `src/walk.rs` |
| `ScanFS`: assembly (`from_parts`, the only constructor path), serialization, cache, report entry points | `src/scan_fs.rs` |
| `ScanConfig` (ecosystems, roots; hashed for the cache) | `src/util.rs` |
| CLI, `resolve_ecosystems`, `python` namespace | `src/cli.rs` |
| OSV batch query, pagination, `is_osv_queryable` | `src/osv_query.rs` |
| Registries (version listing for lookups), `name@requirement` lookups | `src/registry.rs`; `LookupReport::from_name_requirement` / `from_allow_list` in `src/lookup_report.rs` |
| Which ecosystems match server-side vs. client-side | `Ecosystem::osv_server_side_versions` (`src/ecosystem.rs`) |
| Audit report (`unaudited`, errors) | `src/audit_report.rs` |

### Next: releasing 5.0.0 (maintainer)

1. ~~`cargo update`~~: done (see Known issues).
2. Verify `tests/windows_session.rs` in CI (it has not run since milestone 4).
3. Merge `207/polye`, tag `v5.0.0` (the README's pre-commit examples use `rev: v5.0.0`), publish the crate, and build the PyPI wheel.
4. `cargo package` includes `SPEC.md` and `doc/` (74 files); add an `exclude` in `Cargo.toml` if that is not wanted.

§12 surveys further ecosystems (NuGet and Hex are the remaining candidates with OSV coverage). After the release, pick from §9 (open questions); the largest deferred items are purge for non-Python ecosystems and VEX (audit results in the CycloneDX export); for browser extensions, a known-malicious id feed would give them an `audit`.

### Baselines for regression checks (dev machine, release build, warm cache)

- Full home scan, `count`: python 54 origins / 57 sites / 764 packages; npm 40 / 45 / 5,915 (5 sites are VS Code extensions' bundled `node_modules`); cargo 27 / 25 / 1,733 (1,747 before this repo's `cargo update`: superseded versions locked only here dropped out); gha 52 / 118 / 110; vscode 1 / 1 / 39; go 1 / 1 / 4 (`openclaw/scripts/docs-i18n`; 2 / 2 / 153 before `~/.pulumi` was excluded); ruby 2 / 2 / 48 (macOS system Ruby 2.6: `/Library/Ruby/Gems/2.6.0` and the framework's gem directory); ≈4.4s. Each ecosystem alone (`-E <eco> count`) equals its full-scan row, except npm, whose 5 sites in VS Code extensions come from VS Code origins (40 / 40 / 5,840 alone).
- Ruby samples (`--root <scratchpad>/ruby_samples -E ruby`; 8 shallow clones: rails, discourse, mastodon, Homebrew/brew, chatwoot, forem, jekyll, rubocop; the last two are libraries without lockfiles): 10 origins / 10 sites / 1,330 packages (1,847 package × site records); `audit` finds 47 gems with 128 unique advisories in ≈11s uncached; `derive -a lower|upper` writes 886 entries and `validate` reports 0 findings. System gems: `audit` flags 8, e.g. nokogiri 1.13.8 (23 advisories), rexml 3.1.9.1 (7), bundler 1.17.2 (4).
- PHP samples (`--root <scratchpad>/php_samples -E php`; shallow clones of matomo, phpMyAdmin, BookStack, monica, koel, nextcloud/3rdparty (a committed vendor directory, `vendor-dir: .`) and composer/semver (a library without a lockfile)): 6 origins / 6 sites / 705 packages (940 package × site records; nextcloud is an installed site, the rest lockfiles); `audit` finds 23 packages with 61 unique advisories in ≈6s uncached and lists 6 branch versions as unaudited; `derive -a lower|upper` writes 443 entries and `validate` reports 0 findings; `inspect` reports 7 composer plugins. No PHP on this machine: full-home `php` is 0 / 0 / 0.
- pre-commit (full home): 2 origins / 2 sites / 16 hook repositories (`openclaw`, `pandas`); `audit` maps 14 to 14 packages (13 PyPI, 1 Go) and flags pylint 3.0.1 (GHSA-22qq-2fwx-7p67); 2 are unaudited (`shellcheck-precommit`, Docker; `pygrep-hooks`, regex-only hooks). The other ecosystems' counts are unchanged.
- Swift (full home): 2 origins / 2 sites / 17 packages (openclaw's `Swabble` and `apps/macos`); `audit` flags Sparkle 2.8.1 (GHSA-g3hp-f6mg-559v, GHSA-hg88-v3cw-3qrh); 1 unaudited branch pin (`steipete/Peekaboo` on `main`, an untagged commit).
- Go samples (`--root <scratchpad>/go_samples -E go`; 7 shallow clones: hugo, caddy, cli, prometheus, etcd, terraform, moby): 43 origins / 41 sites / 1,284 packages (2,973 package × site records; 2 `go.mod` have no requirements); `audit` finds 14 modules with 20 unique advisories in ≈3s uncached; `derive -a lower|upper` writes 996 entries and `validate` against either reports 0 findings.
- `lookup-name` controls (live): `-E npm lodash@4.17.15` 5 advisories; `-E cargo 'time@=0.1.43'` 2; `-E gha 'tj-actions/changed-files@v35'` 2; `-E gha actions/download-artifact` (all tags) flags exactly v4.0.0–v4.1.2; `-E vscode codeinklingon.git-worktree-menu@1.0.9` resolves to `CodeInKlingon.…` and 1 `MAL-` advisory.
- `fetter derive -a lower|upper|both` then `fetter validate --bound <that fetter.toml>` (full home, since milestone 20 and the follow-ups): 4,474 entries, 0 findings for each anchor. Before: about 4,410 lines (python 302, npm 3,220, cargo 775, gha 68, vscode 32 entries); **0 findings** for both `lower` and `upper`; validate ≈3.8s.
- `fetter inspect` (full home): 37 python, 60 npm, 4 vscode hook rows; ≈4.4s. Python rows identical to milestone 9 apart from sort order within a site (previously filesystem order).
- `fetter -E vscode audit` (full home): no extension is flagged; 3 bundled npm deps are (`simple-git` 2.38.0 with 8 advisories and `moment` 2.29.1 with 2, both in `cschlosser.doxdocgen` 1.4.0; `@github/copilot` 1.0.39 in `github.copilot-chat`).
- `fetter -E gha audit` (full home): 0 advisories (verified by hand against the full OSV GHA dataset: the 4 used repos with advisories are all on unaffected refs). Of the 8 SHA pins without version comments, 7 resolve to tags (milestone 18) and 1 stays unaudited (`bus1/cabuild`, a repository with no tags); 2 commented pins have mismatched comments; ≈10s uncached. (Cargo was 1,737 before this repo's own `Cargo.lock` gained 12 crates from the `serde-saphyr` swap; `Cargo.lock` is gitignored here.)
- Full home `scan`: byte-identical across runs.
- `fetter unpack-count` (full home): python 1,560, npm 11,916, vscode 39, cargo 5 records (package × site); 3,493 packages noted as lockfile/manifest-only (npm 1,641, cargo 1,742, gha 110); ≈3.8s. Byte-identical across runs. Python rows equal v4's apart from order and the new column.
- `fetter -E npm audit` (full home): 372 npm packages with advisories, ≈68s uncached (mostly per-advisory detail fetches).

### Habits that have paid off

- **Survey the real filesystem before writing a scanner**: the 680 non-project `Cargo.lock` files were found this way. For npm, the survey found 65 lockfiles shipped inside `node_modules` packages, 28 VS Code extensions with bundled `node_modules`, and that hidden lockfiles are unreliable.
- **Cross-check found vs. expected with an independent script.** For npm, a Python `os.walk` reimplementing the origin rule found exactly the scanner's 37 project origins. The 2 that had no packages turned out to be Next.js `.next` build output.
- **When an audit finds nothing, prove the detector can find something.** For GHA, a scratch repo with known-affected refs of every kind (exact, floating, SHA + comment, subpath) confirmed that 0 findings across the real workflows was a true negative.
- **Run the release binary's table output, not just `write` and tests.** The milestone 7 `optimize_widths` underflow appeared only when rendering a table with long npm names on a narrow terminal.
- **Probe the live API before trusting docs or assumptions.** This is how we learned that GHA has no server-side version matching, that empty versions match everything, that one bad query fails a whole batch, and that pagination can return empty pages carrying tokens.
- **Diff against the previous commit**, built in a scratch `git worktree`, after any change to discovery.
- **Make anything that reaches output deterministic**: sort at construction, tie-break equal `Package`s.

### Known issues outside the milestones

- *Resolved 2026-11-14*: fetter's own dependencies had OSV advisories (`ring` 0.17.8, `rustls` 0.23.27, `rustls-pemfile` 2.2.0, `rustls-webpki` 0.103.3, `webpki-roots` 1.0.0, `bytes` 1.10.1, `rustix` 1.0.7, `crossbeam-epoch` 0.9.18), and `spin` 0.9.8 was yanked. A `cargo update` (run with `CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=fallback`; no crate needed holding back, and every locked crate's `rust-version` is ≤ 1.89) cleared all of them: `fetter --origin . -E cargo audit` reports no advisories. Tests, clippy and all 6 live tests pass. `Cargo.lock` is gitignored, so CI and `cargo install` resolve afresh.
- *Resolved 2026-11-14*: README examples were stale (`-e`, top-level `site-install`); rewritten in milestone 14.
- *Resolved 2026-11-14*: delimited output (`write -d`) did not quote fields, so a value containing the delimiter corrupted its row (e.g. `sharp`'s `node install/check.js || npm run build` under `-d '|'`). See the delimited-output entry under milestone 10.
- The `unsafe-libyaml` advisory (RUSTSEC-2024-0449) is resolved: `serde_yaml` was replaced with `serde-saphyr`.

---

## 1. Goals and non-goals

**Goals**

- Discover packages from multiple ecosystems: Python, Rust/crates, JS/TS/npm, GitHub Actions, and VS Code extensions. The design should make adding more ecosystems later straightforward.
- Run **one filesystem walk** that discovers every enabled ecosystem, so no search work is wasted.
- Support `scan`, `search`, `count`, `inspect`, `audit`, `derive`, and `validate` across all ecosystems.
- Send one OSV batch query covering mixed ecosystems.
- Provide one **cross-ecosystem allow list** for `validate`/`derive`.

**Non-goals for v5.0**

- `site-install` / `site-uninstall` for non-Python ecosystems. (Unpack *is* generalized, in milestone 13; purge stays Python-only for now, see §9.)
- Resolving moving tags (`@v4`) to concrete releases (SHA pins are resolved; see milestone 18).
- Fetching remote action/extension sources to inspect them.

## 2. Terminology: generalizing "exe" and "site"

v4 is built around *exe → site-packages → packages*. That model doesn't carry over to most other ecosystems; you can't go from an exe to its packages in Rust or TS. v5 generalizes it:

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
| gha | repo root: a dir with `.github/workflows` and `.git` (dir or file) | each top-level workflow file and each local `action.yml` under `.github` (Manifest) | `uses: owner/repo[/path]@ref` → name `owner/repo`; skip `./local`, `docker://`, expressions (implemented) |
| vscode | editor data dir (`~/.vscode`, `~/.cursor`, …; `eco::EDITOR_DIRS`) containing an `extensions/` *dir* | its `extensions/` dir (Installed), plus an **npm** site per installed extension's bundled `node_modules` | installed set from `extensions.json` (else dirs not in `.obsolete`); identity `publisher.name@version` from each `package.json` (implemented) |

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
- Version: `enum PackageVersion { Pep440(VersionSpec), Semver(..), GitRef(String) }`. *Still deferred*: the Cargo scanner did not need it. `VersionSpec` splits on `.` with numeric/text parts, so it orders and displays semver strings and refs well enough for scanning, but it is not sufficient for range matching. Milestone 8 met its GHA need with a small dedicated `osv_match::SemverLike` (`MAJOR[.MINOR[.PATCH]][-PRE]`, optional `v`), since every GHA advisory event in OSV is plain numeric. `PackageVersion` and a general range-matching crate are deferred to allow-list validation (milestone 11).
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
  - **Determinism rule**: every inner `Vec` is sorted at construction. HashMap iteration order must never leak into output. A v4 bug of exactly this kind was fixed in `from_exe_to_sites`.
  - Serialization is the schema 1 scan object (milestone 26; `src/transmit.rs`): named tables of paths, origins, sites (with optional hooks), origin sites and packages, by index into the path dictionary, assigned in sorted order. `site_to_origins` is not stored; it is rebuilt on load. The cache file prefix is `scan_fs_v6_`. The full `ScanConfig` (including ecosystems and roots) is serialized and included in the cache hash. See §5 for the `monitor-scan` form.

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
- **Layout**: `src/eco/mod.rs` plus one file per ecosystem: `src/eco/{python,npm,cargo,gha,vscode}.rs` (all but vscode exist).
  - Python's `get_site_package_dirs`, `get_packages`, `get_origins_bin`, and venv detection are in `eco/python.rs`. Env-marker loading stays in `ScanFS::load_env_marker_state` (filtered to Python origins), because it mutates `ScanFS` state. `is_python_exe` and related helpers stay in `util.rs`, where `exe_path_normalize` also uses them.

### Hooks (generalized `inspect`)

`Hook { ecosystem, package: Option<Package>, kind, path, detail }` describes code that runs without being explicitly invoked.

**Principle** (user, 2026-11-13): Python's `.pth` check is valuable because it is cheap (the first line tells you, e.g. an `import`) and the hook is rare, so its mere presence is a signal. Recording that a package has a hook is only of (minimal) use when most packages do not. Prefer hooks that are cheap to detect and rare; treat common ones as low priority or opt-in.

Measured prevalence on the dev machine (2026-11-13):
- npm install-time scripts (`preinstall`/`install`/`postinstall`): **0.4%** (44 of 10,903 installed top-level packages, e.g. `esbuild`, `sharp`, `unrs-resolver`). This is the clearest analogue of `.pth`.
- VS Code extensions activating on every start (`*`, `onStartupFinished`): 10% (4 of 39).
- Cargo crates with `build.rs`: 13% (93 of 734 registry sources); `proc-macro`: 6%. These are common, and detection needs per-crate lookups in `$CARGO_HOME/registry/src`.
- npm `prepare`: 5%. **Excluded**: it does not run when a package is installed from the registry (only for git deps and a package's own development install), so it is noise.

| Ecosystem | Hooks |
|---|---|
| python | `.pth` files with executable lines; `sitecustomize`/`usercustomize` (existing `inspect_report.rs` logic) |
| npm | `preinstall`/`install`/`postinstall` scripts in installed packages' `package.json` (not `prepare`; see above); `hasInstallScript` in lockfiles |
| cargo | low priority (13% / 6% prevalence): crates with `build.rs` or proc-macros, visible only in `$CARGO_HOME/registry/src`; from `Cargo.lock` alone this is unknowable |
| gha | none in v5.0 (local composite actions are plain workflow steps, already scanned; remote `runs.using` would need fetching) |
| vscode | `activationEvents` containing `*` or `onStartupFinished`; `main` entry |

## 5. CLI v5

**Global flags**

- `-E, --ecosystem <LIST>`: comma-separated or repeated; default **all**. An explicitly requested ecosystem with no scanner yet prints `fetter: ecosystem `npm` is not yet supported and will be skipped` to stderr and continues.
- `--origin <PATH>…`: replaces `-e/--exe`. It deliberately has **no short flag**, because `-o` reads as "output" (and `write -o <FILE>` already means that). `-e`/`--exe` remain as **hidden aliases**, so v4 command lines and v4-installed `site-install` hooks keep working. `*` means discover. A given path's ecosystem is inferred via `eco::infer_origins`: every enabled scanner whose `accepts()` is true contributes an Origin through `normalize_origin()`, so one path can yield several (e.g. a dir with `package.json` and `Cargo.lock`). If no scanner accepts the path, or an accepting scanner cannot normalize it, that is an error. Python accepts interpreter-named paths and virtual-environment directories (`--origin .venv`).
- `--root <DIR>…`: if given, **only** these directories are walked (recursively). Home directories, scanner roots (PATH bins) and default origins (the PATH `python3`) are not added. If not given, discovery is unchanged from v4. Roots are part of the cache hash.
- `--all-users`, `--user-site` (Python only), `--cache-duration`, `--cache-directory`, `--quiet`, `--log`, `--stderr`, `--banner`: unchanged.

**Command placement** (all 16 v4 commands accounted for)

| Shared (all ecosystems, in concept) | `fetter python …` (Python only) |
|---|---|
| `scan`, `search`, `count`, `inspect`, `audit`, `derive`, `validate`, `lookup-name`, `lookup-bound`, `unpack-count`, `unpack-files`, `purge-pattern`, `purge-invalid`, `monitor-scan` | `site-install`, `site-uninstall` |

- Only `site-install` / `site-uninstall` are Python-specific in concept, because they install a Python startup hook. Unpack and purge apply to most ecosystems (npm package dirs, VS Code extension dirs, cargo-installed bins). They are shared commands. Since milestone 13, `unpack-*` reads artifacts in every ecosystem; `purge-*` removes only Python packages (`scan_fs::supports_purge`).

- Ecosystem column, as implemented in milestone 4: `scan`, `search`, `audit`, `lookup-*` tables gain a leading `Ecosystem` column (blank on continuation rows in a TTY, like `Package`). Their JSON already carries `package.ecosystem`. `inspect` gained it in milestone 10. `validate` gained it in milestone 11 and `unpack-*` in milestone 13; `derive` output is a bound, not a table.
- `count` is now one row per enabled, implemented ecosystem: `Ecosystem,Origins,Sites,Packages`. A final `all` row is added only when there is more than one row. It used to be a vertical `Executables/Sites/Packages` list.
- `fetter python …` commands force `-E python`. `lookup-name` takes exactly one ecosystem, Python if none is given (milestone 12). `lookup-bound` follows `-E` like other commands (all by default).
- Python-namespace commands keep their v4 behavior and force `-E python`.
- **Registries and lookups** (milestone 12, `src/registry.rs`; all verified live 2026-11-14):
  - **npm**: `https://registry.npmjs.org/<name>` with the abbreviated metadata format (`Accept: application/vnd.npm.install-v1+json`), 2–4× smaller than the full document. Scoped names are URL-encoded (`@babel%2fcore`).
  - **Cargo**: the crates.io **sparse index** (`https://index.crates.io/se/rd/serde`), the endpoint cargo uses; it has no User-Agent requirement or API rate limit. Yanked versions are excluded. (The crates.io web API returns 403 without a User-Agent.)
  - **GitHub Actions**: `git ls-remote --tags --refs https://github.com/<owner>/<repo>`, which takes ≈0.3s and needs no token. The GitHub API allows only 60 unauthenticated requests per hour. Runs with `GIT_TERMINAL_PROMPT=0` and stdin closed, because GitHub asks for credentials for missing or private repos, which would hang. Requires `git`.
  - **VS Code**: Open VSX (`https://open-vsx.org/api/<publisher>/<name>`, `allVersions`). Chosen over the Marketplace gallery API, which is undocumented, intended for VS Code, and has restrictive terms; the user agreed. Marketplace-only extensions give a clear "not found on Open VSX" error.
  - Each registry also supplies the **canonical name**, which matters because OSV's VS Code names are case-sensitive (and crates.io names can be capitalized, e.g. `Inflector`).
  - Versions are cached per name (`registry_<eco>_<hash>.json`). Failures are errors and are not cached.
  - **`lookup-name`**: Python keeps PEP 508 and PyPI unchanged. Other ecosystems take `name@requirement` (user decision), split at the last `@` that is not the first character, with the requirement in fetter.toml syntax (`allow_list::ReqEntry`). An exact version (npm/vscode bare, cargo `=…`, GHA a single ref) skips version listing; Cargo and VS Code still query the registry for the canonical name. Otherwise the satisfying versions are taken, the most recent `--limit` if given, and audited. For GHA, a subpath reduces to `owner/repo`.
  - **`lookup-bound`** accepts a `fetter.toml`, and a directory (default: the current one) prefers its `fetter.toml` over Python bound files. It audits the most recent satisfying version of each requirement in the enabled ecosystems. Env-marker state is loaded only when there are Python requirements.
  - **Not found versus failed** (follow-up): a registry reporting that a package does not exist (HTTP 404/410 from PyPI, npm, the crates.io index or Open VSX; Open VSX's `error` body; or `git ls-remote` finding no repository or asking for credentials) is a typed `registry::NotFound`. In `lookup-bound` such a requirement is **unresolved**: it is listed in `AuditReport.unresolved` (`{ecosystem, requirement}`, serialized only when non-empty), printed to stderr (`fetter: N requirement(s) not found in their registry, so not audited: …`), and the lookup continues. Any other failure is an error. This replaces v4's silent drop of failed PyPI lookups (`.ok()`) and milestone 12's hard error for other ecosystems, which would have broken `lookup-bound` for any bound with an internal package. In `lookup-name`, a missing package is still an error. An exact requirement (e.g. Python `pkg==1.0`, npm `1.2.3`, a GHA ref) skips the registry, so its existence is never checked.
  - **Registries can delete versions**: Open VSX removed the compromised `checkmarx.ast-results` < 2.58.0, so a *range* lookup cannot see them. An *exact* lookup (`@2.50.0`) still audits and flags MAL-2026-2231.
- `monitor-scan` posts a **schema 1** payload (`src/transmit.rs`, `MonitorPayload`), a JSON object with named fields:
  ```
  {
    "schema": 1,                       // payload and scan schema; independent of fetter's version
    "fetter_version": "5.0.0",         // informational
    "tenant": string,
    "system": {username, hostname, os_name, os_version, architecture, logical_cores},
    "time": {"secs": u64, "nanos": u32},   // since the Unix epoch
    "scan": null | {                   // null when unchanged since the last post
      "paths":        [string],                                   // path dictionary
      "origins":      [{"ecosystem", "path": path_i}],            // sorted
      "sites":        [{"ecosystem", "kind", "path": path_i,      // sorted; kind: "installed"|"lockfile"|"manifest"
                        "hooks"?: [{"name", "detail"}]}],         // present on every site when hooks were collected
      "origin_sites": [{"origin": origin_i, "sites": [site_i]}],
      "packages":     [{"package": {"ecosystem", "name", "key", "version", "direct_url", "purl"?}, "sites": [site_i]}],
      "config":       {"force_usite", "all_users", "ecosystems": [ecosystem], "roots": [string]},
      "origins_hash": string
    }
  }
  ```
  - `ecosystem` is the lowercase CLI name (`"python"`, `"npm"`, …), not the OSV name. `site_to_origins` is not sent; it is derived from `origin_sites`. Environment markers are not sent (they are local to an installation).
  - **Hooks**: `monitor-scan` collects them (`ScanFS::load_hooks`, the same `eco::site_hooks` that `inspect` uses), so `ScanFS.site_to_hooks` is `Some` and every site has a `hooks` array (empty if none). A scan without collected hooks (the cache, `ScanFS` serialized directly) omits the field, and reads back as `None`. Hooks are part of `ScanFS` equality, so a changed hook makes the scan "new".
  - **Forward compatibility** (`transmit::scan_from_wire`): unknown fields are ignored and absent optional fields default; an origin, site or package of an unknown ecosystem, or a site of an unknown kind, is skipped and counted (`MonitorPayload.skipped`: counts and names), as is a known package found only at skipped sites; unknown names in `config.ecosystems` are dropped. A payload with a higher `schema` is read the same way. Out-of-range indices are errors.
  - **Legacy fallback** (`MonitorPayload::from_json`): a JSON array of four is the format of fetter 3.0.0 to 4.0.1, `[tenant, system, scan_or_null, time]` with the scan `[paths, exe_to_sites, package_to_sites, site_to_exes, force_usite, all_users, exes_hash]` and packages without an ecosystem. It is read as `schema` 0 and reinterpreted as Python: executables are Python origins, sites installed Python sites, packages Python packages, `config.ecosystems` `[python]`. A package without an `ecosystem` field in schema 1 is also Python.
  - **API for fetter.io**: `fetter::MonitorPayload` (`schema`, `fetter_version`, `tenant`, `system`, `time`, `scan: Option<ScanFS>`, `skipped`), `MonitorPayload::from_json`, `MonitorPayload::new` and `to_json`; `fetter::Skipped`, `fetter::Hook`, `fetter::SCHEMA`; `ScanFS.site_to_hooks`. `ScanFS` serializes to (and deserializes from) the schema 1 scan object. The v5 positional tuple was never released and is not read.
  - Fixtures: `tests/fixtures/monitor/v4.json` (a real fetter 4.0.1 post, captured from a local listener, with user, host and paths made generic) and `schema1.json` (written by this fetter; reading it pins schema 1's field names).

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

As implemented in milestone 11 (`src/allow_list.rs`). There is no cross-ecosystem standard, so fetter defines **`fetter.toml`**: one table per ecosystem, each in that ecosystem's native syntax.

```toml
[python]
requirements = ["requests>=2.31,<3", "numpy==2.1.*"]   # PEP 508 lines (existing DepSpec, env markers included)

[npm]
"lodash" = "^4.17.21"                                  # npm range semantics (nodejs-semver): bare 1.2.3 is exact
"@babel/parser" = [">=7.18.7", "8.0.0-rc.1"]           # an array means any of

[cargo]
"serde" = "1"                                          # cargo semantics (semver crate): bare 1 is ^1

[gha]
"actions/checkout" = ["v4", "v4.2.2"]                  # allowed refs as written, or a pinned SHA; "*" for any

[vscode]
"ms-python.python" = "*"                               # npm range semantics
```

- **Recognition**: a `.toml` bound (other than `pyproject.toml`) whose top-level keys are all ecosystem tables. Every existing Python bound source (requirements.txt, pyproject.toml, lockfiles, URLs, git repos) is still accepted, as a Python-only allow list (`AllowList::from_bound`). Purl lists and SBOMs are also accepted (milestones 21–22).
- **Keys** are normalized per ecosystem (`Ecosystem::name_to_key`); names are written with their original case.
- **Any of**: every non-Python entry may be a string or an array of strings. Ranges exclude pre-releases in both npm and cargo semantics (`>=7.18.7` does not match `8.0.0-rc.1`), and cargo has no `||`, so arrays are how `derive` keeps pre-releases valid. npm users can also write a native `||` in one string.
- **GHA** entries match the ref as written (`v4` matches only `@v4`, not `@v4.1.0`), or the pinned SHA (`direct_url.vcs_info.commit_id`) of a SHA-pinned ref, which otherwise carries its comment version.
- **Validation** (`ScanFS::to_allow_list_validation`):
  - Python uses the existing path unchanged.
  - Every other enabled ecosystem with a section uses `ValidationReport::records_from_section`, with the same semantics without env markers: a package with an unsatisfied requirement is Misdefined; a package without one is Unrequired unless `--superset`; an unmatched requirement is Missing unless `--subset`. `--ignore` applies to every ecosystem.
  - An ecosystem with **no section is skipped**; `--strict` treats it as an empty section (user decision).
  - Records and reports gain an ecosystem (table column; `"ecosystem"` in `validate json`). Requirements display as `name spec[ | spec…]` (e.g. `serde >=99`).
- **`derive`** (`AllowList::from_packages`):
  - Python uses `DepManifest::from_packages` unchanged.
  - npm, cargo and vscode: `--anchor lower` gives `>=min`, `upper` gives `<=max`, ordered by semver. Observed versions the range excludes (pre-releases) are added as exact alternatives (`8.0.0-rc.1`, or `=…` for cargo). Unparseable versions are skipped. `both` is still "Not implemented".
  - GHA lists every observed ref (the anchor does not apply).
  - **Output** (user decision): requirements format (as before) when every derived package is Python, otherwise `fetter.toml`; `--format requirements|toml` overrides. `requirements` writes only Python packages.
- `purge-invalid` accepts a `fetter.toml` too; removal is still limited to Python packages (`supports_purge`). `site-install` passes its bound to `validate`, so a `fetter.toml` works there as well.

## 6. OSV

- `OSVPackageQuery::from_package` (`src/osv_query.rs`) uses `package.ecosystem.osv_name()` in place of the hard-coded `"PyPI"`.
- One `query_osv_batches` call takes mixed ecosystems, in chunks of 64. The cache key hashes the query JSON, which includes the ecosystem.
- **Failure is never "clean"** (milestone 5). A request error, an unparseable response (such as a 400 body), or a response whose `results` length differs from the query count is an error. It propagates to `audit` / `lookup-*` as `fetter Error: …` with exit 1, and nothing is cached. Before v5, failures became `None` (no vulnerabilities) for the whole chunk and were cached.
- **Pagination** is followed. A result may carry `next_page_token`, and follow-up requests re-send only those queries, each with `page_token`. IDs accumulate across pages and duplicates are dropped. A failure on any page fails the query, and the number of requests per batch is capped at `OSV_MAX_PAGES` (100). `page_token` is omitted when unset, so cache keys are unchanged.
  - Documented triggers: more than ~1,000 vulns for one query, or more than ~3,000 for a batch. Observed 2026-11-13: a 10-query batch returning 6,002 vulns did *not* paginate. A single Ubuntu `linux` query did (1,000 + token). A `Linux/Kernel` query returned **empty pages that still carry tokens** before finishing with no matches, so a token with zero vulns must be followed, not treated as done. Covered by mock tests and the ignored live test `test_osv_pagination_live` (about 80s).
- **Unaudited packages**: a package is queried only if `osv_query::is_osv_queryable` (currently: a non-empty version). The rest go into `AuditReport.unaudited`, which is serialized as `"unaudited"` only when non-empty. `audit` prints `fetter: N package(s) could not be audited: …` to stderr. Unaudited packages do not change the exit code.

**Observed OSV API behavior** (verified live 2026-11-13; guarded by the ignored test `osv_query::tests::test_osv_ecosystem_names_live`):
- All five `osv_name()` values (`PyPI`, `npm`, `crates.io`, `GitHub Actions`, `VSCode`) are accepted.
- **One invalid ecosystem name fails the entire batch** with HTTP 400 (`error in query at index N: invalid ecosystem`).
- **An empty `version` matches every advisory for the package**: OSV treats it as "any version". That is why `is_osv_queryable` excludes it.
- PyPI, npm and crates.io do server-side version matching (`jinja2@2.4.1`, `lodash@4.17.15`, `time@0.1.43` all return advisories).
- **GitHub Actions has no server-side version matching.** GHA advisories carry only `ECOSYSTEM` ranges (e.g. `introduced 0, fixed 46.0.1`) with no `versions` list, and OSV does not evaluate them for this ecosystem. A versioned query always returns nothing, even for versions inside an affected range (e.g. `actions/download-artifact@4.0.0` against `introduced 4.0.0, fixed 4.1.3`), with or without a `v` prefix. GHA auditing therefore needs client-side matching: query by package name without a version (which returns all advisories), fetch each advisory's `affected` ranges, and compare the ref's version locally. See milestone 8.
- **GHA client-side matching** (milestone 8, `src/osv_match.rs`):
  - `Ecosystem::osv_server_side_versions()` is false for `Gha`. `AuditReport::from_packages` sends those packages through `query_osv_client_side` (one `/v1/query` per distinct name, without a version, with pagination and a per-name cache, `osv_package_*.json`; failures are errors and are not cached), and merges the results back in package order. Advisory details still come from the normal `/v1/vulns` path.
  - Ranges: `ECOSYSTEM` and `SEMVER` events (`introduced`, `fixed`/`limit`, `last_affected`) become intervals. `GIT` (commit) ranges are skipped. An interval left open is bounded by GHSA's `database_specific.last_known_affected_version_range` (`< X` / `<= X`) if present: GHSA-vqf5's open v2 range (`introduced 2.26.11`) otherwise covers all of v3. Explicit `versions` lists match exact refs.
  - **Ref interpretation** (`RefKind`, by convention; the user chose this over "exact only"):
    - **Exact** (`@v4.1.0`, or a SHA pin with a full version comment `# v4.1.1`): within an interval or listed.
    - **Floating** (`@v4` = `[4.0.0, 5.0.0)`, `@v4.1` = `[4.1.0, 4.2.0)`): affected iff an interval overlaps the line and is not fixed within it, since a moving tag points at the newest release of its line. A `last_affected` within the line counts as unfixed.
    - **Branch** (`@main`, `@release/v1`, any other non-SHA, non-numeric ref): affected only by an unbounded (unfixed) interval.
    - **Unresolvable** (a SHA without a full version comment): goes to `unaudited`.
  - The package version is the ref as written (`actions/checkout-v4`), so reports show what was inferred. SHA pins keep the SHA in `direct_url.vcs_info.commit_id` and the comment in `revision`.
  - Survey of 1,113 real `uses:` on the dev machine: floating major 53%, SHA pin 30.5% (91% of them with a full version comment), branch 7.5%, local 6.8%, exact 1.3%. With these interpretations about 97% of non-local refs are auditable, against 1.3% without them.
  - OSV names actions by repository: `github/codeql-action/init` returns nothing, while `github/codeql-action` has the advisory. Reusable workflows (`owner/repo/.github/workflows/x.yml@ref`) are likewise `owner/repo`.
  - Full OSV GHA dataset (2026-11-13): 64 advisories; ranges: 78 `ECOSYSTEM`, 2 `SEMVER`, 10 `GIT`; 2 open ranges (both with `last_known_affected_version_range`); 19 entries with `versions` lists; no `v`-prefixed events.
- **VS Code** (verified live 2026-11-13):
  - **Server-side matching works**: an open `introduced: 0` range matches an unlisted version (9.9.9), so `VsCode` uses the batch path (`osv_server_side_versions` true).
  - **Package names are case-sensitive**: `CodeInKlingon.git-worktree-menu` matches but `codeinklingon.git-worktree-menu` does not. `extensions.json` IDs are lowercased, so names come from `package.json` (`publisher` + `name`). Keys stay lowercase for identity.
  - Full dataset: 21 advisories across 20 extensions, nearly all `MAL-` (OpenSSF malicious-packages), mostly `introduced: 0` open ranges plus `versions` lists, 2 with fixes, and 4 entries with `versions` only (no ranges). Notable compromises of legitimate extensions: `nrwl.angular-console` 18.95.0, `checkmarx.ast-results` < 2.58.0, `checkmarx.cx-dev-assist` < 1.12.0, `aquasecurityofficial.trivy-vulnerability-scanner` 1.8.12–1.8.13.

## 7. Milestones

Each milestone leaves `cargo test` green and lists the main files it touches.

1. [x] **SPEC.md**: this document.
2. [x] **Core types**: `Ecosystem`, `Package.ecosystem`, `PackageVersion`, `Origin`/`Site`/`SiteKind`, `ScanFS` field renames, and v5 serialization. Python only; CLI behavior unchanged.
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
   - **Bug fixed** (predates v5): `table::optimize_widths` underflowed (`width - reduction`) when the terminal was narrower than the non-ellipsisable columns. Release builds then panicked with "Formatting argument out of range"; long npm names triggered it. Now saturating, with a testable `optimize_widths_for`.
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
8. [x] **GHA scanner** with **client-side version matching** (§6).
   Files: `eco/gha.rs`, new `osv_match.rs`, `osv_query.rs`, `audit_report.rs`, `ecosystem.rs`. (`serde_yaml` was already replaced in milestone 7.)
   Done 2026-11-13.
   - **Origins**: repo roots only, i.e. `.github/workflows` plus `.git`, where a `.git` file (worktree, submodule) counts, since GitHub only runs workflows at a repo root. On the dev machine this excluded 16 non-running copies: 6 editor extensions, 7 build copies under Cargo `target/`, 2 vendored subprojects, and 1 ambiguous dir. An explicit `--origin` does not require `.git`, and accepts the repo dir, `.github`, `workflows`, or a workflow file.
   - **Sites**: top-level `*.yml`/`*.yaml` in `.github/workflows` (GitHub does not read subdirectories), plus `action.yml`/`action.yaml` anywhere under `.github` (local composite actions).
   - **Packages**: `uses:` values come from the parsed YAML (`jobs.*.steps[*]`, `jobs.*` reusable workflows, `runs.steps[*]`), so text in `run:` scripts is ignored. Comments (version hints) come from raw lines, because YAML drops them. If a file does not parse as YAML, a line-based fallback is used. Packages are deduplicated per site.
   - **Verified on the dev machine**:
     - 52 origins, 118 sites, 110 packages. An independent script found the same 52 repo roots (including a nested org `.github` repo, `disclude-io/.github`) and the same 109 `name-version`s; 110 because one `name-version` appears with two different SHAs.
     - Other ecosystems unchanged; `scan` deterministic.
     - The live audit found 0 advisories, confirmed as a true negative by hand against the full dataset.
     - A scratch control repo produced exactly the expected findings for every ref kind.
9. [x] **VS Code scanner.**
   Files: new `eco/vscode.rs`; `eco/mod.rs` (`EDITOR_DIRS` moved here from `npm.rs`).
   Done 2026-11-13.
   - **Origins**:
     - An editor data dir named in `EDITOR_DIRS` that contains an `extensions` **directory**. A project's `.vscode` settings folder (which may hold an `extensions.json` *file* of recommendations) is never one.
     - Claimed and **pruned** by `detect`; also found by `default_origins` (home editor dirs; `VSCODE_EXTENSIONS` as `<editor>/extensions`).
     - `--origin` accepts the editor dir or its `extensions` dir.
   - **Installed extensions**: `extensions.json` is authoritative. On the dev machine it listed exactly 39 of 94 dirs, and all 55 others were in `.obsolete`, with no overlap. Without it, all dirs not marked obsolete are used. Entries whose dir is missing are skipped.
   - **Packages**: `publisher.name@version` from each installed extension's `package.json`, case preserved.
   - **Bundled dependencies** (user decision): an installed extension's `node_modules` becomes an extra site of the **npm** ecosystem under the VS Code origin, and the npm scanner reads it (`site_to_packages` dispatches on the site's ecosystem). Paths name the extension, so findings are traceable. Obsolete versions' `node_modules` are not included.
     - Consequence: `-E vscode` reports these npm-ecosystem packages too; `-E npm` alone does not, because npm skips editor extension dirs.
     - Package-like dirs without a `package.json` (bundled build output, e.g. `vue.volar`'s plugin packs, `rust-analyzer`'s `d3`) have no identity and yield nothing.
   - **Verified on the dev machine**:
     - 1 origin, 39 extensions; identities match an independent read of `extensions.json` + `package.json` exactly.
     - 7 extensions bundle `node_modules` and 5 yield packages (+75 unique npm packages); the other 2 hold only bundled build output.
     - Other ecosystems unchanged; ≈4.0s; deterministic.
     - The live audit flagged no extension but 3 bundled deps (see baselines).
     - A control `.cursor` dir (fork, no `extensions.json`) flagged exactly the compromised versions and not their fixed versions, including the case-sensitive name.
10. [x] **Hooks**: generalize `inspect` via `site_to_hooks`.
    Files: `inspect_report.rs` (rewritten), `table.rs`, `eco/{mod,python,npm,vscode}.rs`.
    Done 2026-11-14.
    - **Framework**: `Hook { name, detail }` and `EcosystemScanner::site_to_hooks` (default: none). `InspectReport::from_site_to_origins` asks every site's scanner, in parallel. Sites without hooks are omitted, records are sorted by site, and hooks within a site are sorted and deduplicated.
    - **Report**: columns are now `Ecosystem | Site | Hook | Content` (was `Site | File | Content`). Content is truncated to 60 characters as before. The red highlight of `sitecustomize.py` / `usercustomize.py` now keys on the `Hook` header.
    - **Python** (moved to `eco/python.rs`): every `.pth` plus `sitecustomize.py` / `usercustomize.py`, showing the first 10 lines joined by spaces, unchanged. Rows are identical to before except that they are now **sorted** within a site; they used to follow `read_dir` order.
    - **npm**:
      - Installed packages' `preinstall` / `install` / `postinstall` scripts; the detail is the command. `prepare` is excluded (see §4).
      - Package-lock sites report `hasInstallScript` entries, with no script text available. Other lockfiles do not record install scripts.
      - The `node_modules` walk was refactored into a visitor (`visit_node_modules`) shared by packages and hooks.
    - **VS Code**: installed extensions whose `activationEvents` include `*` or `onStartupFinished`; the detail lists the events and `main`.
    - **Not done**, per the §4 principle: cargo `build.rs` / proc-macros (common, and they need registry lookups) and GHA. No `inspect json` output was added.
    - **Verified on the dev machine**:
      - Python rows match the milestone 9 baseline as a set.
      - npm: 60 hook rows across 45 sites, matching an independent re-implementation row for row. The first comparison "failed" only because the `||` in `sharp`'s script split under the `|` delimiter; see known issues.
      - VS Code: the same 4 startup extensions as the prevalence survey.
      - Output deterministic.
    - **Follow-up: delimited output quoting and write errors** (`table.rs`, `cli.rs`, `dep_manifest.rs`):
      - `write -d` now quotes fields per RFC 4180, but **only** fields containing the delimiter, `"`, `\n` or `\r`; embedded quotes are doubled. Everything else is byte-identical to before (a full `scan` file was unchanged, with 0 quoted lines), and output now parses with standard CSV readers.
      - `Tableable::delimited_quoting()` (default true) is overridden to false by `DepManifestReport`: `derive write` produces a requirements file, whose header (`# via fetter`) and marker lines contain spaces (the delimiter it uses), so it is written as before.
      - A delimiter of `"`, `\n` or `\r` is rejected (`InvalidInput`) before the file is created or truncated.
      - **Write errors are no longer swallowed**: all 11 `let _ = x.to_file(…)` calls in `cli.rs` became `?`. A bad delimiter or an unwritable path now gives `fetter Error: Cannot write <path>: …` and exit 1 (previously silent, exit 0). Display output (`to_writer`) still ignores errors, so a closed pipe (`| head`) stays quiet.
11. [x] **Allow list**: `fetter.toml`, per-ecosystem requirements (semver crates), generalized `validate`, `derive` and `purge-invalid` (§5a).
    Files: new `allow_list.rs`; `validation_report.rs`, `scan_fs.rs`, `cli.rs`, `dep_manifest.rs` (unchanged API), `Cargo.toml` (`semver` 1.0.28, MSRV 1.68; `nodejs-semver` 7.0.0, MSRV 1.85).
    Done 2026-11-14.
    - **Survey first**: in `package.json` files on the dev machine, npm ranges were caret 78.5%, exact 11.5%, protocols/aliases 5.9%, `*`/`latest` 3.0%, comparators 0.6% and tilde 0.5%, with no unions, hyphens or x-ranges. The decisive point for using crates was semantics: bare `1.2.3` is exact in npm but `^1.2.3` in cargo.
    - `ValidationRecord` gained `ecosystem`, and `dep_spec` became `dependency: Option<Dependency>` (`Python(DepSpec)` or `Other(ReqEntry)`). `ValidationRecord::new` still builds Python records. `ScanFS::to_validation_report(dm, …)` remains as a test-only wrapper; `to_dep_manifest` was replaced by `to_allow_list`.
    - **Verified on the dev machine**:
      - Python `derive` writes a byte-identical requirements file (104 lines) to milestone 10. `validate json` records are identical apart from the new `ecosystem` field, and the `validate write` CSV is identical apart from the new first column.
      - Full-system round trip (`derive`, then `validate` against it): 0 findings for `lower` and `upper`. The first attempt found 12 npm pre-releases (`@babel/parser 8.0.0-rc.1` vs `>=7.18.7`), which led to "any of" arrays.
      - A hand-edited bound produced exactly the expected Misdefined (cargo `serde >=99` × 4 versions), Unrequired (10 `actions/checkout` refs) and Missing (`nobody.not-installed`) records.
      - `--strict` and `--format` behave as designed.
12. [x] **Registries** for `lookup-name` / `lookup-bound`: npm, crates.io, GitHub tags, Open VSX (§5).
    Files: new `registry.rs`; `lookup_report.rs`, `cli.rs`, `ureq_client.rs` (`get_with_accept`, a default trait method so mocks are unchanged), `allow_list.rs` (`ReqEntry::new` is now `pub(crate)`). `pypi_project.rs` was kept as is rather than folded into a `Registry` trait: Python's lookup path is unchanged.
    Done 2026-11-14.
    - `LookupReport::from_dep_manifest` now delegates to the shared `dep_manifest_to_packages`, with unchanged behavior.
    - **Verified live**: controls in every ecosystem (see baselines); `lookup-bound` on a 4-ecosystem `fetter.toml` and on its directory with `-E npm`.
    - Errors: more than one `-E` for `lookup-name`; an unknown npm package (404); a missing GitHub repo, which fails with "terminal prompts disabled" instead of hanging.
    - **Follow-up: not found versus failed** (§5): `registry::NotFound` plus `http_error` classification, also used by `pypi_project.rs`. `LookupReport` resolves each requirement to Found, Unresolved or Failed (strings, so outcomes can cross rayon threads), via the shared `from_resolutions` and `dep_manifest_resolutions`. `cli::report_unchecked` prints both unaudited and unresolved notes for `audit` and both lookups. `UreqClientMock` now answers a GET with no mock response with HTTP 404, as a server would.
      - Verified live: a requirements.txt with `jinja2<2.5` plus an internal package audits jinja2 (18 advisories) and reports the other as unresolved (exit 0 with `json`). A fetter.toml with a missing package in Python, npm, Cargo and VS Code reports all four and completes. `lookup-name` on a missing package is still exit 1.
13. [x] **Artifacts**: generalize `unpack-count`, `unpack-files`, `purge-pattern` and `purge-invalid`. Add an `EcosystemScanner` method that lists a package's installed artifacts (files/dirs). Python uses dist-info `RECORD`; npm and VS Code use the package/extension dir; cargo uses `.crates2.json` bins. Remove `supports_artifacts`. Lockfile/manifest-only packages have no artifacts and are reported as such.
    Files: `unpack_report.rs`, `scan_fs.rs`, `eco/*`.
    Done 2026-11-14, **unpack only**; purge stays Python-only (user decision; see §9).
    - `EcosystemScanner::site_to_artifacts(site, wanted)` is called once per site with the set of packages wanted there, so npm walks each `node_modules` once rather than once per package. `Artifacts` moved to `eco/mod.rs`; Python's RECORD reader is `eco::python::artifacts_from_record` (unchanged logic).
    - npm: each package directory (nested and `.pnpm` copies included) plus all files below it, excluding its own `node_modules` (other packages). A package installed at several places in one site is one record with several dirs. Lockfile sites have none.
    - VS Code: each extension directory and its files, excluding its bundled `node_modules` (an npm site of its own).
    - Cargo: the binaries listed in `.crates2.json` / `.crates.toml` under the install root's `bin`, each with whether it exists; no dirs. `Cargo.lock` sites have none.
    - Packages found only at lockfile or manifest sites produce no rows; `UnpackReport::without_artifacts` counts them per ecosystem, and the CLI prints a note on stderr.
    - Records are sorted by (package, site); previously by package, then the package's site order. Within a package name this can reorder rows (e.g. `/opt/...` now before `~/...`).
    - `supports_artifacts` became `supports_purge`.
    - **Verified**: Python `unpack-count` and `unpack-files` (1,560 and 407,322 rows) are the same sets as HEAD's, minus the new column. An independent `os.walk` found the same 11,916 npm (package, site) records with identical file and dir counts, apart from 2 packages vendored in a subdirectory node_modules (`@github/copilot/sdk/sharp/node_modules`), which the scan does not report as packages, so their files count under `@github/copilot`. VS Code matches exactly (39 extensions, 18,875 files), and Cargo matches `.crates2.json` (6 bins, 2 missing on disk, shown as `exists=false`).
14. [x] **Release**: README/docs, crate description and keywords, version 5.0.0, changelog from section 11.
    Done 2026-11-14 (tagging and publishing are left to the maintainer).
    - **Version 5.0.0, not 6.0.0**: the planning session miscounted the current release (4.0.1) as v5. This spec's v5/v6 references were renumbered to v4/v5, and the cache prefix is `scan_fs_v5_`.
    - README: new "Supported Ecosystems" section (origins, sites and package sources per ecosystem); examples regenerated from a scratch demo project with a venv (jinja2 3.1.3, zipp 3.18.1, requests 2.32.3), an npm lockfile (lodash 4.17.15), a `Cargo.lock` (time 0.1.43) and a workflow (`actions/checkout@v4`, `tj-actions/changed-files@v35`), run with `HOME` set to the scratch dir so paths read `~/demo`; `fetter.toml` section; `fetter python site-install`; CLI docs for every command including `monitor-scan`; a 5.0.0 "What is New" entry condensed from §11. The two Python validate examples for a changed or removed `zipp` were edited by hand from the v4 README (adding the column), not re-run.
    - `Cargo.toml`: version, description, keywords (`sbom`, `vulnerability`, `allow-listing`, `cybersecurity`, `osv`); `homepage` dropped (duplicated `repository`).
    - `.pre-commit-hooks.yaml` uses `--origin python3` (the `--exe` alias still works).
    - Help text: `--stderr` said "stdout"; `inspect` and `--all-users` were Python-specific.
    - **Fix found while writing examples**: `--root .` reported paths like `~/demo/./Cargo.lock`. `util::path_normalize` now drops `.` components (it still does not resolve `..`), which affects every user-supplied path.

15. [x] **Go** (added after milestone 14; see §12 for why Go first). An `Ecosystem::Go` (`-E go`, OSV `Go`) scanner over `go.mod`.
    - **Origin**: a directory with a `go.mod` whose `module` path is a valid module path (this excludes templates such as Pulumi's `module ${PROJECT}`). **Site**: that `go.mod` (`SiteKind::Manifest`: since Go 1.17 it lists the full build list, direct and `// indirect`). The main module and modules replaced by a local path are excluded (like Cargo path crates); `replace` with a module version substitutes that module and version.
    - **Pruned**: the module cache (`$GOMODCACHE`, default `$GOPATH/pkg/mod`, default `~/go/pkg/mod`); `vendor` and `testdata` beside a `go.mod` (the go tool ignores `testdata`).
    - **Names** are module paths, case-sensitive (the key is the path as written). **Versions** keep the `v` (`v1.2.3`, pseudo-versions, `+incompatible`); OSV accepts them as-is (probed: `v0.17.0` and `0.17.0` return the same 25 advisories for `golang.org/x/net`), so matching is server-side.
    - **Allow list** `[go]`: npm-style ranges (`">=v1.2.3 <v2"`), since Go has no range syntax of its own; derive writes `>=v…` / `<=v…`.
    - **Registry**: `https://proxy.golang.org/<escaped path>/@v/list` (tagged versions only; uppercase letters escaped as `!` + lowercase, else 404; 404/410 is not found).
    - No hooks (Go runs no code at install) and no artifacts (the module cache is shared and read-only) in this milestone.
    - Deferred: `go.work` (each member's `go.mod` is scanned anyway); `go install` binaries in `~/go/bin` (module versions are embedded in the binary's build info; needs a reader); the `stdlib` package (OSV has Go standard-library advisories, keyed to the `go` / `toolchain` version, which says little about the compiler actually used); `go.mod` files with `go` < 1.17, which list only direct requirements.
    - **Test data**: this machine has little Go (see §12), so shallow clones of popular repositories (hugo, caddy, cli, prometheus, etcd, terraform, moby) in the scratchpad serve as samples, checked against an independent parser. (Go is not installed here, so `go list -m all` was not available as a second check.)
    Done 2026-11-14.
    - `src/eco/go.rs`: a `go.mod` tokenizer (comments, quoted paths, `directive ( … )` blocks) and parser for `module`, `require` and `replace`; `tool`, `godebug`, `exclude`, `retract`, `toolchain` and `ignore` are read past. A version-specific `replace` takes precedence over a path-wide one. A `go.mod` without a valid `module` line is not an origin (one hugo fixture has none). `semver_of`, `strip_version_v` and `proxy_escape` are shared with `allow_list.rs`, `registry.rs` and `lookup_report.rs`.
    - Allow list: `Requirement::GoRange`, npm ranges after removing the `v` from versions in both the spec and the package version; a package version without its `v` does not match. `derive` orders Go versions via `semver_of` (it previously used `semver::Version::parse`, which would have silently dropped every Go entry).
    - **New `EcosystemScanner::cache_dirs`**, collected from *all* scanners (not just enabled ones) into the walk's exclude set: Cargo's `registry` and `git` dirs and `$RUSTUP_HOME`, and Go's module cache. Found because `-E go` alone found a `go.mod` in `~/.cargo/registry/src/…/rustls-platform-verifier…/verification_mock`, while a full scan did not (Cargo's detect prunes `~/.cargo` only when Cargo is enabled). Results no longer depend on `-E`. Full-home counts for the other ecosystems are unchanged.
    - **Verified**: an independent Python parser over the samples found the same 43 origins and identical 2,973 (package, site) records. An independent OSV `querybatch` of all 1,284 packages (via curl) found the same 14 modules and 20 advisories as `audit`. Controls: `lookup-name -E go 'golang.org/x/net@v0.17.0'` returns 25 advisories; `'github.com/BurntSushi/toml@>=v1.3.0'` lists v1.3.0–v1.6.0 (escaped proxy path); a nonexistent module is a not-found error. A changed `fetter.toml` entry (`golang.org/x/net >=v0.99.0`) is reported as Misdefined.
    - Test fixtures that pin the full ecosystem list (serialization, cache hashes, `count` rows) were updated for the sixth ecosystem.

16. [x] **Ruby** (`-E ruby`, OSV `RubyGems`), after Go, at the user's request. `src/eco/ruby.rs`.
    Done 2026-11-14.
    - **Origins**: project directories with `Gemfile.lock` or `gems.locked` (site: the lockfile, `Lockfile`), and gem directories, any directory with `specifications` and `gems` subdirectories (site: the directory, `Installed`; pruned, as installed gems ship lockfiles of their own). Walk-found gem directories include `~/.gem/ruby/X`, rbenv/rvm/asdf Rubies and `vendor/bundle/ruby/X`. System gem directories outside home (`/Library/Ruby/Gems/*`, macOS's `Ruby.framework/Versions/*/usr/lib/ruby/gems/*`, Homebrew, `/usr/local/lib/ruby/gems`, `/usr/lib/ruby/gems`, `/var/lib/gems`, `/usr/share/gems`) are default origins, canonicalized because macOS reaches one directory through three paths (`Versions/2.6`, `Versions/Current`, `/usr/lib/ruby`). Ruby is never executed.
    - **Lockfiles**: `GEM` sections (rubygems.org: no provenance; other gem servers: their URL), `GIT` sections (repository, revision, ref) and 4-space-indented spec lines; `PATH` gems (local, such as the project's own gem) are excluded like Cargo path crates; dependency lines, `PLATFORMS`, `DEPENDENCIES`, `CHECKSUMS`, `RUBY VERSION` and `BUNDLED WITH` are skipped (so `bundler` itself is not a package of a project). Platform variants (`ffi (1.17.1-arm64-darwin)`) are one package with the platform removed: OSV's matching differs with the suffix (nokogiri `1.13.8` has 23 advisories, `1.13.8-arm64-darwin` 22).
    - **Installed**: each `.gemspec` in `specifications` and `specifications/default` (Ruby's bundled gems), identified by the first stub line (`# stub: name version platform require_paths`), else `s.name` / `s.version`. A second stub line lists extensions.
    - **Names are case-sensitive** (`Nokogiri` returns no OSV results and a 404 from rubygems.org); the key is the name as written.
    - **Versions and requirements**: `GemVersion` and `GemRequirement` implement `Gem::Version` ordering (canonical segments; a string segment sorts before a number) and `Gem::Requirement` (`=`, `!=`, `<`, `>`, `<=`, `>=`, `~>`; comma means and; a bare version is `=`). Tested against 410 vectors generated with the local Ruby 2.6, with `Gem::Version#<=>` replaced by RubyGems master's: RubyGems 3.0.3 compares raw segments and differs on 2 of them (`1.0.a` vs `1.a.2`). The `[ruby]` allow-list table uses this syntax; `*` is not valid (use `>= 0`). RubyGems ranges admit pre-releases, so `derive` adds no exact alternatives.
    - **Hooks** (installed sites only): gems with native extensions (built at install; not default gems, which are built with Ruby), gems with `lib/rubygems_plugin.rb` (loaded by `gem` commands), and `plugins/*.rb` in the gem directory. On this machine: libxml-ruby, nokogiri and sqlite3.
    - **Artifacts**: a gem's `gems/<full name>` and `extensions/<platform>/<abi>/<full name>` directories with their files, its spec, and `cache/<full name>.gem`.
    - **Registry**: `https://rubygems.org/api/v1/versions/<name>.json` (one entry per version and platform; deduplicated).
    - **Shared changes**:
      - `VersionSpec` keeps the version text as written for display and serialization; equality, hashing and ordering still use its parts. It had printed `3.2024.0305` (a RubyGems date version) as `3.2024.305`. A full-home scan is byte-identical before and after.
      - The walk applies every scanner's `prune` / `skip_children`, enabled or not, and collects Origins only from enabled ones (`walk.rs`, `WalkScanner`). Ruby's gem directories are found by walking, not at fixed paths, so `cache_dirs` alone could not keep other ecosystems out of them.
      - `head_lines` and `HOOK_HEAD_LINES` moved to `eco/mod.rs`. `allow_list` orders derived versions by a per-ecosystem `VersionKey` (semver or `GemVersion`).
    - **Verified**: Bundler 1.17's own `LockfileParser` (with `BUNDLED WITH` removed, as it refuses newer lockfiles) agrees on all 1,844 gems it reports; the 3 gems only fetter reports are exactly the `GIT` gems, which that Bundler omits without a checkout. Installed gems equal RubyGems' `Gem::Specification.each` (48). An independent OSV `querybatch` of all 1,330 sample gems found the same 47 gems and 128 advisories as `audit`. Lookups: `nokogiri@~> 1.16.0` lists 1.16.0–1.16.x with advisories; `Nokogiri` is not found.
    - Follow-ups, done 2026-11-14:
      - **RubyInstaller (Windows)**: `ruby_installer_gem_dirs` lists `<system drive>\Ruby*\lib\ruby\gems\<abi>` (`%SystemDrive%`, default `C:`); only complete gem directories become origins. These paths are not canonicalized on Windows, where canonical paths are verbatim (`\\?\C:\...`). User gems (`%USERPROFILE%\.gem`) were already found by the home walk. The code is behind `cfg!(windows)` (a runtime constant, so it compiles everywhere) and is unit-tested on a temp tree; it has not run on Windows (a cross `cargo check` here fails building `ring`'s C sources), so the Windows CI job is its first real run.
      - **`PLUGIN SOURCE`** lockfile sections (gems from sources provided by Bundler plugins, e.g. S3 or private registries) are read like `GEM` sections, with the section's `remote` as provenance. The format (`remote:`, `type:`, options, `specs:`) was confirmed against Bundler 1.17's `Plugin::API::Source#to_lock`. None of the samples use one; the test fixture follows that format.
    - Deferred: `.gem` files outside gem directories. Not planned: a `Gemfile` or `.gemspec` without a lockfile, which lists requirement ranges, not versions (reporting packages would need dependency resolution, as for Maven); these are mostly libraries, whose resolved gems appear in the projects that use them.

17. [x] **PHP** (`-E php`, OSV `Packagist`), at the user's request. `src/eco/php.rs`.
    Done 2026-11-14.
    - **Origins**: a directory with `composer.json` and either a `composer.lock` or an installed vendor directory; a vendor directory with no `composer.json` beside it or in its parent (e.g. shipped in a WordPress plugin) is an origin of its own. **Sites**: "installed where it exists, else lockfile", as for npm: the vendor directory (`config.vendor-dir`, default `vendor`; `.` makes the project its own vendor directory, as in nextcloud/3rdparty) if `composer/installed.json` exists, else `composer.lock`. `vendor` is skipped by the walk (installed packages carry their own `composer.json`); a project that is its own vendor directory is pruned. A `composer.json` alone is not an origin (it lists constraints, not versions).
    - **Packages**: `packages` and `packages-dev` of `composer.lock`; `installed.json` in Composer 2 (`{"packages": [...]}`) and Composer 1 (`[...]`) forms. Path packages (`dist`/`source` type `path`) are excluded. Names are lowercased: Packagist is case-insensitive and canonically lowercase, and OSV's names are case-sensitive and lowercase (`Symfony/Http-Kernel` returns nothing). Versions are as written (`v5.4.0`; OSV accepts either form).
    - **Branch versions** (`dev-*`, `*-dev`; 6 in the samples, e.g. `roave/security-advisories dev-latest`) carry their source repository and commit, and `is_osv_queryable` returns false for them, so they are listed as unaudited: OSV compares them as strings (`guzzlehttp/guzzle` `dev-master` matched 12 unrelated advisories).
    - **Versions and constraints** are ported from composer/semver (no `regex` dependency: its AND splitter needs lookbehind, which the `regex` crate lacks): `normalize` (`VersionParser::normalize`, including date versions, stability expansion, aliases, `@stability` flags and numeric branches), `ComposerConstraint` (`parseConstraints`: `||`/`|`, space/comma AND, `^`, `~`, wildcards, hyphen ranges, comparators, and the `-dev` lower-bound rules), and `version_compare` (PHP's, with its special-form order dev < alpha < beta < RC < number < patch). Tested against composer/semver's own data: 73 `successfulNormalizedVersions`, 118 `satisfiesProvider` and 29 `compareProvider` cases, all passing. The `[php]` allow-list table uses this syntax; `derive` writes `>=`/`<=` and adds each observed branch version as an exact alternative.
    - **Hooks**: packages of type `composer-plugin` (run by `composer install`/`update`), with their plugin class; from installed and lockfile sites.
    - **Artifacts**: an installed package's directory (`install-path`, relative to `vendor/composer`; Composer 1: `vendor/<name>`).
    - **Registry**: `https://repo.packagist.org/p2/<name>.json` (tagged releases; branch versions are in the separate `~dev` file, not read).
    - **Verified**: an independent Python parser found the same 6 origins and 940 records; nextcloud's `installed.json` equals its `composer.lock` (94 packages). An independent OSV `querybatch` of the 699 released packages found the same 23 packages and 61 advisories as `audit`. Lookups: `guzzlehttp/guzzle@~7.4.0` lists 7.4.0–7.4.5 (14 → 9 advisories); a missing package is not found.
    - **Bug found while verifying**: an exact `lookup-name` (`Symfony/Http-Kernel@v5.4.0`) skips the registry, so the name was queried as typed and OSV, being case-sensitive, returned nothing. PHP names are now lowercased on that path.
    - Not done: Composer's `repositories` (private repositories are not distinguishable in the lockfile from Packagist packages), `composer.json` without a lock, and a check on a real global install: `~/.composer` or `~/.config/composer` (a `composer.json` plus `vendor/composer/installed.json`) should be found as an ordinary installed project by the walk, but none exists here to confirm it.

18. [x] **GHA SHA pins in `audit`** (deferred from milestone 12; at the user's request).
    Done 2026-11-14.
    - `osv_match::resolve_sha_pins`, called at the start of `AuditReport::from_packages`: for GHA packages whose version is a bare SHA (no version comment, or a comment that is not a full version), list the repository's tags with `git ls-remote --tags` (with peeled `^{}` lines, so annotated tags map to their commit; `registry::tag_commits`, cached like registry listings as `registry_gha_commits_<hash>`), once per repository in parallel. The pin takes the most specific tag on its commit (`version_from_tags`: the highest full version, else the most specific floating tag) as its version, as a pin with that version comment would, keeping the SHA in `direct_url` and setting `vcs_info.revision` to the tag.
    - A commit with no version tag, or a repository that cannot be listed (missing, private, no `git`), leaves the pin unchanged, so it stays in `unaudited`. A lookup failure is logged, never treated as clean.
    - `AuditReport.resolved_pins` (`name`, `commit`, `version`; omitted from JSON when empty) records each resolution, and `cli::report_unchecked` prints them on stderr.
    - **Test seam**: running git moved into `UreqClient::git_ls_remote_tags` (a default method that runs `git`). `UreqClientMock` answers from `mock_get` under `git+<url>` and otherwise fails as git does for a missing repository, so unit tests never run git. The GHA registry listing (`lookup-name`) uses the same method.
    - **Comment mismatches** (follow-up, same day): `check_sha_pins` (replacing `resolve_sha_pins`) also lists the repositories of pins *with* version comments (27 pins in 24 repositories here) and checks that the tag the comment names is on the pinned commit, ignoring a leading `v`. If not, it records a `PinMismatch` (`name`, `commit`, `comment`, `actual`, `tag_exists`: whether the comment's tag exists on another commit or not at all) in `AuditReport.pin_mismatches`, `cli::report_unchecked` prints a warning, and the pin is audited as what its commit is actually tagged as, or (with no version tag) left unaudited, since the SHA is what runs. Found 2 real mismatches here, both stale comments in a numpy clone: `actions/checkout@b4ffde65f463 # v3.1.0` is v4.1.1, and `github/codeql-action@cdcdbb579706 # v2.1.27` is v2.13.4 (each confirmed with `git ls-remote`). Control: `actions/download-artifact@<v4.1.0 SHA> # v4.1.3`, which was previously audited as the fixed v4.1.3 and passed, is now flagged with GHSA-cxww-7g56-2vh6. A comment naming no tag (`# v9.9.9`) is reported with `tag_exists: false`. The full GHA audit takes ≈10s uncached (24 more repositories listed), and is fast with the cache. The warning names the pin but not its workflow file (audit records carry no sites); `fetter search -p <name>` finds it.
    - **Verified**: all 7 resolutions on this machine match an independent `git ls-remote` (e.g. `actions/create-github-app-token@d72941d…` is tagged `v1` and `v1.12.0` and resolves to `v1.12.0`; `gerrymanoim/pr-prefix-labeler` and `mymindstorm/setup-emsdk` carry only floating tags on their commits, `v3` and `v14`). `bus1/cabuild` has no tags and stays unaudited. Control: a scratch workflow pinning bare SHAs of `actions/download-artifact` v4.1.0 and v4.1.3 and `tj-actions/changed-files` v35.0.0 flags v4.1.0 (GHSA-cxww-7g56-2vh6) and v35.0.0 (both advisories), and not the fixed v4.1.3.

19. [x] **pre-commit** (`-E precommit`), at the user's request, with the user's idea of auditing hooks through the packages they install. `src/eco/precommit.rs`.
    Done 2026-11-14.
    - **Scan**: an origin is a directory with `.pre-commit-config.yaml` (site: the file, `Manifest`). Each remote `repo` with a `rev` is a package named by its URL without scheme, credentials or `.git` (`github.com/psf/black`, `gitlab.com/x/y`; `git@host:path` is `host/path`); `local` and `meta` are excluded. A SHA rev with a full version comment (`# frozen: v1.2.3` from `pre-commit autoupdate --freeze`, or `# v1.2.3`) takes the comment's version with the SHA in `direct_url`, as for GHA.
    - **No OSV ecosystem**: `Ecosystem::has_osv` is false for pre-commit (`osv_name` is empty), so hook packages are never queried directly (`is_osv_queryable`), and the live OSV-name test skips it.
    - **Mapping** (`map_hooks`, at audit time, after `check_sha_pins`): for a hook repository on GitHub, fetch `.pre-commit-hooks.yaml` at the rev (the pinned commit, if any) from raw.githubusercontent.com (cached as `precommit_file_<hash>`). For each of its hooks' languages with a registry (a config does not record which hook variant's language is used, so all are mapped; actionlint's golang, docker and system variants run the same tool):
      - python: the package from `pyproject.toml` (`[project]`, else Poetry's `[tool.poetry]`, where a bare version is exact and `optional` dependencies are skipped), else `setup.cfg`, else `setup.py` (literal arguments). The version is the manifest's static version, else the rev without `v`. A placeholder version `0.0.0` marks a wrapper (`ruff-pre-commit`, `mirrors-mypy`), whose package is skipped. Pinned dependencies (`ruff==0.1.13`) are mapped.
      - node: `package.json` (not a `0.0.0` placeholder) and its exact dependencies.
      - golang: the `go.mod` module at the rev; rust: `Cargo.toml`.
      - The hooks' own pinned `additional_dependencies` (`prettier@3.1.0` in mirrors-prettier, python `x==1`).
      A mapped hook is replaced by its packages (audited in their ecosystems; recorded in `AuditReport.precommit_hooks` and printed on stderr); an unmapped hook (other language, not on GitHub, no hooks file, or a fetch failure, logged) stays and is unaudited.
    - **SHA pins**: `check_sha_pins` covers pre-commit too (repository URL `https://<name>`; `registry::tag_commits` now takes a URL), so frozen revs are resolved and comment mismatches caught.
    - **Allow list** `[precommit]`: refs as written, like `[gha]`. **Registry**: tags via `git ls-remote` (`lookup-name -E precommit github.com/psf/black@23.1.0`, which audits through the mapping).
    - **Verified**: an independent Python script re-derived the mapping of all 16 hook repositories here from the raw files and agreed on all 16 (13 python, 1 go, 2 unmappable). An independent OSV `querybatch` of the 14 mapped packages matched `audit` (pylint 3.0.1 only). Control config: `psf/black@23.1.0` maps to black and is flagged with 5 advisories; `psf/black@<22.3.0 SHA>  # frozen: 24.3.0` is caught as a mismatch and audited as 22.3.0; a bare SHA of pre-commit-hooks resolves to v4.5.0; `mirrors-prettier@v3.1.0` maps to npm `prettier` via its `additional_dependencies`. isort was first unmapped (Poetry metadata), which added `[tool.poetry]` support.
    - Deferred: the config's own `additional_dependencies` (done in milestone 24); hook repositories not on GitHub (no raw-file host is assumed); `language: conda`, `dotnet`, `haskell`, `lua`, `r`, `swift`, `julia`, `perl`, `coursier`, `dart`.

20. [x] **Swift** (`-E swift`, OSV `SwiftURL`), at the user's request. `src/eco/swift.rs`.
    Done 2026-11-14.
    - **Origin**: a directory with a `Package.resolved` (a SwiftPM package, or an Xcode project's or workspace's `xcshareddata/swiftpm`); site: the file (`Lockfile`). `.build` beside a `Package.swift` is skipped (its `checkouts` hold dependencies with their own resolved files). Formats 1 (`object.pins`, `repositoryURL`) and 2–3 (`pins`, `location`, `kind`) are read.
    - **Names**: OSV's `SwiftURL` names a package by its repository URL without scheme or `.git` (`github.com/apple/swift-nio`; probed: `https://…`, `….git` and `github.com/Apple/…` all return nothing), case-sensitively; some canonical names are mixed case (`github.com/sparkle-project/Sparkle`). Names keep their case as written in `location` (SwiftPM writes it as declared in `Package.swift`); keys are lowercase for allow lists. A differently-cased name is a false negative (`lookup-name github.com/sparkle-project/sparkle@2.8.1` finds nothing), which only the GitHub API could fix.
    - **Pins**: a remote pin with a version takes it (OSV matches server-side; `v` prefixes are accepted), keeping its revision in `direct_url`. A branch or revision pin takes its revision SHA and goes through `check_sha_pins` (only versionless Swift pins are checked; a versioned pin was resolved by SwiftPM, and checking every one would list every dependency's tags), resolving to a tag where the commit is tagged; otherwise it is unaudited (`swift::is_queryable`). Local pins are excluded. Registry pins (`kind: registry`, identity `scope.name`) keep the identity, which OSV cannot match, so they are unaudited.
    - **Allow list** `[swift]`: npm-style ranges over semver. **Registry**: the repository's tags via `git ls-remote` (a leading `v` removed).
    - **Verified**: an independent parser found the same 21 package × site records in the 2 local files; an independent OSV `querybatch` of the 15 versioned pins matched `audit` (Sparkle 2.8.1, 2 advisories); the `Peekaboo` branch commit has no tag (and is not a current branch head), so it stays unaudited. `lookup-name 'github.com/sparkle-project/Sparkle@>=2.6.0 <2.9.0'` lists 2.6.0–2.8.1 (3 advisories through 2.6.3, then 2).
    - Deferred: checking that a versioned pin's revision is still its tag (a moved tag is a supply-chain signal), at the cost of listing every dependency's tags; Swift package registries.

21. [x] **Package URLs (purl) and SBOM input** (deferred from milestone 11; at the user's request; the CycloneDX export followed in milestone 23). `src/purl.rs`.
    Done 2026-11-14.
    - **Purl** (`Purl::parse`, `Purl::new`, `Display`): the specification's parse and build algorithms (percent-encoding with `.-_~:` unencoded; type, namespace, name, version, qualifiers, subpath), with the type rules of the 9 purl types fetter maps (required or prohibited namespaces; lowercase for `composer`, `vscode-extension`, `github`; PEP 503 for `pypi`). An unencoded npm scope `@` (`pkg:npm/@babel/core`) is accepted. Tested against the purl-spec test suite, vendored with its MIT license in `tests/fixtures/purl-spec/` (commit 26671718c3da): the 18 specification cases and the per-type cases of `pypi`, `npm`, `cargo`, `golang`, `gem`, `composer`, `swift`, `vscode-extension` and `github`; 139 of 142 run and pass. Skipped: a recommended case that normalizes an uppercase qualifier key, contradicting a required case that rejects it; and two pypi cases whose message is in their `subpath` field (upstream errata; the validate case for the same purl runs).
    - **Mapping** (`package_purl`, `purl_targets`): python ↔ `pypi`, npm ↔ `npm`, cargo ↔ `cargo`, go ↔ `golang`, ruby ↔ `gem`, php ↔ `composer`, swift ↔ `swift`, vscode ↔ `vscode-extension`, gha ↔ `github`, precommit ↔ `github` (GitHub-hosted only). A `pkg:github` purl maps to both GitHub Actions and pre-commit (the user's choice). Other types are reported as unsupported.
    - **Output**: `Package` serializes a derived `purl` (omitted when there is none: a pre-commit hook not on GitHub, a Swift registry identity), so every JSON output (and the scan cache, which ignores it on load) carries it. `derive --format purls` writes every observed package version's purl.
    - **Input**:
      - `lookup-name pkg:...` (the purl names the ecosystem; `-E` must agree; `pkg:github` is a GitHub Action unless `-E precommit`).
      - `fetter.toml` `purls = [...]`: each allows exactly its version (`exact_requirement`: `=v` for Cargo, `name==v` for Python, the bare version otherwise; any version without one), merged with the tables' entries as alternatives. Entries created only by `pkg:github` purls are `optional` (`ReqEntry::optional`): never reported Missing, since a repository may be used as an Action, a hook, or neither.
      - Bounds (`AllowList::from_bound`, so `validate`, `lookup-bound` and `purge-invalid`): CycloneDX JSON (`components[].purl`, nested, including the metadata component's subcomponents but not the component itself) and SPDX 2.x JSON (`externalRefs` of type `purl`); and text files whose non-comment lines are all purls. SBOM packages without a purl are counted on stderr.
    - **lookup-bound and exact versions**: a requirement is audited at its newest allowed version (what would be installed), but an entry whose specs are all exact versions (as from an SBOM, where each is present) is now audited at every one. Found by the cross-check below: Juice Shop's SBOM lists `ansi-regex` at four versions, and only the newest was audited.
    - **Verified**: full-home `derive --format purls` (8,618 purls) then `validate --bound` gives 0 findings. Real SBOMs from CycloneDX's `bom-examples` (Juice Shop, 840 npm purls; Laravel, 62 composer; Proton Bridge, 201 golang with pseudo-versions) through `lookup-bound` match an independent OSV `querybatch` *by purl* (OSV's own purl parsing) exactly: 89, 13 and 16 vulnerable packages (202, 45 and 136 advisories). SPDX's example 7 Go SBOM has no purls, which the stderr note now reports. `lookup-name` with purls reproduces the baselines (lodash 4.17.15: 5; time 0.1.43: 2; Jinja2 2.4.1: 18; tj-actions v35: 2; Sparkle 2.8.1: 2). A further check of every derived purl with the reference `packageurl-python` was not run (installing it was not permitted in this session).
    - Deferred: CycloneDX XML, SPDX tag-value and SPDX 3; purl qualifiers (e.g. `vcs_url`, `platform`) are parsed but not used. (VERS ranges: milestone 22; CycloneDX output: milestone 23.)
22. [x] **VERS version ranges** (at the user's request: a purl list with ranges is a complete alternative to `fetter.toml` except for Python markers, extras and direct URLs). `src/vers.rs`.
    - **Parsing** (`Vers::parse`, `Display`): strict, as the specification requires; non-canonical input is an error, not normalized: lowercase `vers` scheme and type; no whitespace; no explicit `=`; percent-encoding only of `><=!*|%` and space, uppercase hex; no leading, trailing or doubled `|`; `*` alone; versions sorted by the type's order and unique; ignoring `!=`, a bare version followed only by a bare version or a lower bound; ignoring both, lower and upper bounds alternating.
    - **Types and order** (`vers::key`): `pypi` (`VersionSpec`), `npm`/`cargo`/`vscode-extension`/`swift`/`semver` (SemVer), `golang` (SemVer of `v`-prefixed), `gem` (`GemVersion`), `composer` (`ComposerVersion`), `github` (unordered), `lexicographic`, and `all`/`none` (`*` only). Other types (e.g. `maven`, `datetime`) are errors. Extensions: an unordered version (all `github` refs, Swift SHAs, composer branches) may be a bare version or `!=` anywhere in the sequence and is matched exactly (SHAs case-insensitively); a range of only `!=` contains all other versions.
    - **Containment** (`Vers::contains`): the specification's algorithm (equality with a bare, `<=` or `>=` version is in; with `!=`, out; then the intervals), plus single-bound ranges, which the algorithm's pairwise loop misses (`vers:pypi/>0.0.2` must contain `0.0.3`).
    - **Allow lists**: `ReqEntry` specs starting with `vers:` are VERS alternatives to the native specs (its type must be the ecosystem's purl type, `purl::purl_type`, or `all`/`none`), so they work in `fetter.toml` tables too; GitHub Actions and pre-commit SHA pins also match by SHA. Purl entries (`purl::parse_entry`) are a purl, whitespace, and an optional VERS of the purl's type; a purl with a version cannot also have a range. For Python a range becomes PEP 508 lines (`Vers::to_pep508`), one per exact version or interval, each with the `!=` exclusions, since a `DepManifest` accepts a package satisfying any of several lines for one name. `lookup_report::exact_version` treats a one-version range as exact. CycloneDX 1.7 `versionRange` (external components; it is not in 1.6) on a versionless purl is read as its range.
    - **Derive**: `derive --format purl-vers` (`purl::purl_ranges`) writes one entry per (ecosystem, key): `>=` lowest, `<=` highest, or both (a bare version if only one), plus every unordered version exactly. Since VERS has no npm/cargo pre-release exclusion, no exact alternatives are needed as in the `fetter.toml` derive.
    - **Tests**: the VERS test suite, vendored with its MIT license in `tests/fixtures/vers-spec/` (commit 2c387b682dfb): canonical parsing (16 of 20 run; the 4 `datetime` cases are skipped as unsupported), pypi and npm containment (8 of 11 run; 3 ranges are not canonically ordered, e.g. `>=3.0.0|2.0.3`, which the parser must reject), pypi validation (14 canonical inputs round-trip; 5 rejected: 2 unsorted, normalization fetter does not do, and 3 upstream errata expecting duplicated versions or redundant bounds such as `>0.0.0|>=0.0.1` to be valid), and the gem `~>2.0.8` conversion checked against `GemRequirement` on sample versions.
    - **Verified**: full-home `derive --format purl-vers` (4,473 entries) with each anchor, then `validate --bound`, gives 0 findings; tightening three ranges (cargo, npm, pypi) gives exactly those three Misdefined. `lookup-bound` of a range list audits each interval's newest version (jinja2 `<2.11` → 2.10.3; lodash `<4.17.21` → 4.17.20; nokogiri `<1.13.4` → 1.13.3; requests `2.19.0|>=2.31|<2.32` → 2.19.0 and 2.31.0).
23. [x] **CycloneDX export** (deferred from milestone 21; at the user's request). `src/cyclonedx.rs`; `derive --format cyclonedx`.
    - **Shape**: CycloneDX 1.6 JSON (1.7 adds nothing used here). One `library` component per distinct purl (its `bom-ref`; packages without a purl get a `fetter:<eco>/<name>@<version>` reference and no `purl`, counted on stderr), sorted by reference, with `group`/`name` from the purl's namespace and name. Sites are `evidence.occurrences` (`location` the absolute path, `additionalContext` `fetter site kind: <kind>`), the standard field for where a component was found, rather than paired properties, which would be ambiguous with several sites. Properties: `fetter:ecosystem`, and `fetter:commit` for a VCS `direct_url`, whose URL is a `vcs` external reference. `metadata.tools.components` names fetter and its version; `metadata.timestamp` is RFC 3339 UTC (a small civil-date conversion; no date crate).
    - **Serial number**: an RFC 4122 layout UUID (version nibble 5) from SHA-256 of the components and the export time in nanoseconds. The schema says a BOM SHOULD have a unique serial number even when its contents are unchanged, so it is not derived from the components alone; components themselves are deterministic.
    - **Not exported**: dependency graph (`dependencies`; scanners do not record edges), hashes, licenses, and audit results (VEX: a candidate follow-up, `vulnerabilities` with `affects` refs).
    - **Verified**: full-home export (8,618 components, 9 ecosystems) checked against the official `bom-1.6.schema.json` (fetched from CycloneDX/specification at tag 1.6) by a stdlib validator covering `$ref`, `type`, `properties`, `additionalProperties`, `required`, `enum`, `pattern`, `items`, `uniqueItems`, `oneOf`/`anyOf`: 0 errors, and 5 of 5 planted errors caught; bom-refs unique. `validate --bound` on the export gives 0 findings; changing two purls' versions gives exactly those two Misdefined.
24. [x] **pre-commit `additional_dependencies`** (deferred from milestone 19; at the user's request). `src/eco/precommit.rs`, `src/scan_fs.rs`.
    - **Sites**: `PreCommitScanner::origin_to_sites` returns the config as the pre-commit site and, for each enabled ecosystem its hooks' pinned dependencies name (Python, npm, Go, Ruby), the same file as a `Manifest` site of that ecosystem, following the VS Code precedent (bundled `node_modules` as npm sites under a VS Code origin). Such a site cannot be read by its ecosystem's scanner, so `scan_fs::site_to_packages` routes it (`precommit::is_dependency_site`: another ecosystem, `Manifest`, file name `.pre-commit-config.yaml`) to `precommit::dependency_site_packages`, which keeps the packages of the site's ecosystem. Packages therefore keep their own ecosystem, and `count`, `-E`, `audit`, `validate`, `derive` and the CycloneDX export handle them unchanged; no network access is needed.
    - **Which entries** (`pinned_dependency`): every repository's hooks, `local` included (its dependencies are installed into the hook's environment even though the repository itself runs no third-party code). The config's `language` decides the ecosystem when given (always for `local`); for a remote hook the language is in the hook repository, unknown offline, so the form decides: PEP 508 `==` (exact, no wildcard) is Python; `name@version` with an exact semver is npm, or Go when the name is a module path (first segment containing `.`, not `@`-scoped) and the version is `v`-semver. Go targets are `go install` package paths; on GitHub, GitLab and Bitbucket the module is the first three segments plus a `/vN` segment. Ruby (`language: ruby` only) is `name:version`, as `gem install` takes it. Unpinned entries are skipped (the version installed is unknown), as are Rust ones (`name:version` and `cli:name:version` are Cargo requirements, not exact versions) and other languages.
    - **Purge**: `purge-*` now removes only from `Installed` sites (`ScanFS::installed_sites`), so a Python package found through a config (a `Manifest` site) is never purged there. Before, this was already harmless (the dist-info path under a file does not exist), but it is now explicit.
    - **Verified**: unit tests over a config with Python, npm (scoped and unscoped, a `^` range skipped), Go (`/cmd/` paths trimmed to the module, a `/v2` module, a non-hosted `mvdan.cc` path), Ruby (pinned and unpinned) and Rust (skipped), and the site per enabled ecosystem. Here, the one config with `additional_dependencies` (pandas) reports exactly `pyright@1.1.347` as npm (through a YAML anchor), with `tomli` and `pyyaml` unpinned and skipped; `count`, `scan`, `derive --format purls` and `audit` include it.
25. [x] **Browser extensions** (at the user's request). `src/eco/{browser,browser_version,chromium,firefox}.rs`; ecosystems `chromium` and `firefox` (`Ecosystem::ALL` is now 12; the scan cache's ecosystem list, and so its hash, changed).
    - **Privacy rule**: profiles hold browsing data, so the scanners open only extension metadata: Chromium `Preferences` and `Secure Preferences` (only `extensions.settings` is kept; the user approved reading these files), each extension's `manifest.json` and `_locales/<default>/messages.json`; Firefox `extensions.json`. Profiles are claimed and pruned by `detect`, so the walk never enters them (on macOS `~/Library` is excluded anyway; profiles there come from `default_origins`).
    - **Chromium**: user-data directories per platform (`MAC_BROWSERS`, `LINUX_CONFIG_BROWSERS`, Snap/Flatpak `LINUX_HOME_BROWSERS`, `WIN_LOCAL_BROWSERS`, `WIN_ROAMING_BROWSERS`); a profile is a directory with `Preferences` and either `Secure Preferences` or `Extensions` (Electron apps here, such as Code, Docker Desktop, Postman, have only `Preferences`). Opera's user-data directory is itself a profile. The settings of both files are merged per id (macOS and Windows keep them in `Secure Preferences`, Linux in `Preferences`). Component extensions (`location` 5 and 10) are excluded: on this machine, all 7 Chrome and 6 Vivaldi entries are components (Chrome Web Store Payments is 10, though it sits in `Extensions`). The manifest comes from `path` (relative to `Extensions` unless absolute, as for unpacked extensions), else the settings' copy; versions left behind by an update are ignored. Without settings, every `Extensions/<id>/<version>` is read. Packages are named by id (purl type `chrome-extension`, registered in purl-spec, with versions of 1–4 integers); the display name, with `__MSG_key__` resolved, appears in `inspect`.
    - **Firefox**: profiles are the children of each browser's profile directory (Firefox, LibreWolf, Waterfox, Zen, Floorp, Thunderbird; Linux Snap/Flatpak too) with `extensions.json` and `prefs.js`. Add-ons of type `extension` are packages, by id, except built-in locations (`app-builtin`, `app-builtin-addons`, `app-system-defaults`, `app-system-addons`). No purl type is registered for Firefox add-ons, so they have no purl (counted on stderr in purl and SBOM output).
    - **Versions and allow lists** (`browser_version`): Chromium order is numeric (`VersionSpec` after an `is_chromium_version` check); Firefox uses Mozilla's toolkit comparison (`moz_cmp`: parts `<num><str><num><extra>`, a missing string after any string, `1.0+` = `1.1pre`, `*` highest), tested with Mozilla's documented ordering. `fetter.toml` entries are `BrowserReq`: `*`, or comma-separated clauses (`>=`, `<=`, `>`, `<`, `=`, `==`, `!=`, bare) that all hold; `derive` writes `>=lo`, `<=hi`, or `>=lo, <=hi`. VERS: `chrome-extension` is a supported type, ordered numerically.
    - **inspect** (`browser::Grant::detail`): reported if any granted host pattern covers every site (`<all_urls>`, a `*` host, `file:///*`), any of `STRONG_APIS` (`debugger`, `management`, `nativeMessaging`, `proxy`) is granted, or the install source is a signal (Chromium: unpacked, command line, external sideload, internal not from the store; Firefox: temporary, sideloaded `app-system-*`/`winreg-*`/`app-global`, or unsigned, `signedState` <= 0). Chromium uses `active_permissions` (granted), falling back to the manifest's permissions, host permissions and content-script matches; Firefox uses `userPermissions`. The detail lists the name, broad hosts, `NOTABLE_APIS` present, the source, Manifest V2, and disabled state (`disable_reasons`/`state`; Firefox `active`/`userDisabled`). Policy installs are not flagged by source alone.
    - **inspect output**: `write` (delimited) now keeps each hook's full content; only the terminal table truncates to 60 characters (all ecosystems; before, both truncated).
    - **audit / lookup**: `has_osv` is false for both (OSV has no browser-extension ecosystem), so they are listed as unaudited; `lookup-name` errors clearly. `unpack-*` lists a Chromium extension's version directory (or unpacked source directory) and a Firefox add-on's `.xpi` (or directory); purge stays Python-only.
    - **Verified**: unit tests over synthetic profiles (store, unpacked, component, left-behind versions, localized names, no-settings fallback, Electron look-alikes, temporary unsigned Firefox add-on, built-ins, a dictionary). Here: 2 Chromium profiles (Chrome, Vivaldi) with only components, so 0 packages; 3 Firefox profiles, 2 extensions (`plugin@okta.com` 6.49.0, flagged for all sites with `cookies`, `webNavigation`, `webRequest`, `webRequestBlocking`; an old SQLiteManager, not flagged). A synthetic Chrome profile through the CLI (`--origin` on its user-data directory): scan, inspect, purls, CycloneDX purl, `fetter.toml` and VERS validation, with a tightened bound reported Misdefined. Full-home `derive` (toml and purl-vers) then `validate` gives 0 findings; the full scan stays ~4 s.
    - Deferred: Safari (extensions are inside app bundles, and its data directory is protected by macOS privacy controls); a known-malicious extension id feed for `audit` (sources not yet evaluated); AMO version listing for `lookup-name` (it has an API, but without advisories there is nothing to look up); Chromium extensions' bundled npm dependencies are not reported (they are usually bundled into single files).
26. [x] **Versioned `monitor-scan` payload** (at the user's request, for fetter.io: `../fetter-orb`, whose `orb-model` reads posts with `fetter = "3.0.0"`'s types, `(String, SystemTag, Option<ScanFS>, Duration)`, and stores `package_to_sites`). `src/transmit.rs`; `ScanFS` (de)serialization and `site_to_hooks`; `monitor.rs`; schema in §5.
    - Schema 1 is a named-field object; the scan object replaces the unreleased v5 tuple in both the payload and the cache (`scan_fs_v6_`). Forward compatible (unknown fields ignored; unknown ecosystems and site kinds skipped and counted), with a fallback reading fetter 3.0.0–4.0.1 posts as Python (that format is identical across those releases, checked at each tag).
    - Hooks per site, optional: collected by `monitor-scan` (about a second here), omitted elsewhere. `eco::site_hooks` is now shared with `inspect` (and skips pre-commit dependency sites). `Hook` is public and serializable; `SiteKind::{as_str, from_name}` and `Ecosystem::from_cli_name` added; `SystemTag` is `Clone`.
    - Environment markers are not sent (the user's decision: local to an installation).
    - `monitor-scan` no longer panics its worker: a failed scan, a poisoned lock or a serialization failure (e.g. a non-UTF-8 path) is logged and that period is skipped (after a serialization failure the next post sends the full scan); a failure to read system details is an error at start.
    - **Verified**: 6 tests (the real v4.0.1 fixture, including its unchanged `null` form, read as two Python origins sharing one site with a git `direct_url` and re-sent as schema 1 identically; round trip with and without hooks and for unchanged scans; the `schema1.json` fixture read and re-written identically; forward compatibility with `schema` 7, extra fields, a `nuget` origin, site and package, an npm site of kind `registry`, and an unknown configured ecosystem, all skipped and counted; malformed payloads). Real data: a full-home `monitor-scan` posted to a local listener (1.6 MB; 185 origins, 270 sites, 8,635 packages, 105 hooks at 53 sites; about 4 s) read back by `from_json` with nothing skipped, and re-written to an identical scan object; reports from the v6 cache equal fresh ones.
    - **fetter-orb, not yet changed**: to use this it needs `fetter` 5, `MonitorPayload::from_json` in `monitor_scan_load_from_json` (replacing the tuple and its `expect`), `Site` instead of `PathShared` for sites (`site_packages_insert_or_get` takes `sp.path`), an ecosystem (and site kind) column for packages and sites, `Package.ecosystem` in `package_from_row`, and optionally storing hooks and `skipped`.

## 8. Testing strategy

- Fixtures, as practiced: inline string constants in each scanner's test module (e.g. `LOCK_V4`, `CRATES2` in `eco/cargo.rs`), written to `tempfile` dirs when a test needs files on disk. This keeps a fixture next to the parser it exercises. Use `tests/fixtures/<eco>/` only for large real-world samples.
- Scanner unit tests: parsers, `detect`, `accepts`/`normalize_origin`, `origin_to_sites`, and `site_to_packages` against those fixtures; `site_to_hooks` from milestone 10.
- Walker tests: a `tempfile` tree with mixed project types. They assert the detected origins and the pruning.
- Determinism: serialize-twice comparisons (like `test_from_exes_equality_comparison`), now covering multi-site packages in every ecosystem.
- OSV: `UreqClientMock` (fixed response per URL prefix) for single-response cases; the test-only `SeqClient` in `osv_query.rs` (queued responses, recorded request bodies) for pagination. Live checks are `#[ignore]` tests.
- Discovery changes are also verified manually against the previous commit and the baselines in "Picking this up".
- Existing Python tests stay green across milestones 2–4; only mechanical constructor changes are allowed.

## 9. Open questions

- **VEX (consider later, at the user's request)**: add `audit` results to the CycloneDX export (milestone 23) as a `vulnerabilities` section: one entry per advisory (OSV id, aliases such as CVEs, severity and CVSS from `osv_vulns`, references), `affects` pointing at the components' `bom-refs`, and an `analysis.state` of `exploitable` or `in_triage` (fetter cannot judge reachability, so not `not_affected`). Unaudited packages (unresolvable SHAs, unmapped hooks) could be noted as properties. Questions: whether this is `derive --format cyclonedx --audit`, `audit --format cyclonedx`, or a separate VEX document that references the SBOM's serial number (CycloneDX supports both inline and standalone VEX).

- GHA `GIT`-type ranges (commit-bounded, 10 in OSV) are skipped. Evaluating them for SHA pins needs the repository's commit graph.
- Table output at scale: a full home `scan` now has thousands of npm rows. Consider grouping by origin, or a `--summary` view.
- Should `--root` include `/` under `--all-users`, or stay with home dirs only?
- Packages with a `direct_url` (Python VCS/local installs; Cargo git and alternative-registry crates) are queried in OSV by name, as if from the public registry. For forks or private crates that share a public name, this can produce false positives. Should they go in `unaudited` instead? Today's behavior favors not missing advisories.
- GHA allow-list semantics: should `"v4"` in a bound also allow `@v4.x.y` refs, or SHA pins whose comment is in v4? Today matching is exact as written.
- `purge-*` for npm, VS Code and Cargo (deferred from milestone 13). The user considers removing packages behind a package manager's back a feature, not a defect. Questions to settle then: whether stale lockfile, `extensions.json` and `.crates2.json` entries are acceptable or should be edited too, and whether an npm package found at several depths is purged everywhere.
- Package display is `name-version`, which reads ambiguously for Go modules whose path ends in a major-version suffix (`github.com/aws/aws-sdk-go-v2-v1.30.0`). JSON output has separate fields. Consider `name@version` for display (npm already uses `@` for scopes, so this needs care).
- npm packages vendored inside a package subdirectory (e.g. `pkg/sdk/x/node_modules/y`) are not reported by the scan; only `node_modules` directly under a site or package root is walked. Two such packages exist here.

## 10. Decision log

- 2026-11-15: The `monitor-scan` payload is a named-field object with a `schema` number independent of fetter's version (1), shared with the scan cache; readers skip what they do not know; fetter 3–4 posts are read as Python. Hooks are sent per site; environment markers are not. fetter-orb is updated separately.

- 2026-11-15: Browser extensions are two ecosystems, `chromium` and `firefox` (different id spaces, stores and version formats; Chromium browsers share one id space, so an extension in Chrome and Brave is one package at two sites). Packages are named by id, not display name. Chromium preferences files are read (the user's choice) for unpacked extensions and state, keeping only `extensions.settings`. No audit (no OSV ecosystem).

- 2026-11-15: pre-commit `additional_dependencies` are reported at scan time in their own ecosystems (not only mapped during `audit`), with the config as a site per ecosystem; ecosystems are inferred from the form when the config gives no `language`; unpinned entries are skipped.

- 2026-11-15: CycloneDX export is a `derive` format (`--format cyclonedx`) beside `purls` and `purl-vers`, not a new command; CycloneDX 1.6 only; sites go in `evidence.occurrences`; the serial number is unique per export.

- 2026-11-14: VERS ranges are supported as a second, interchange syntax beside native requirement syntax (not converted from it, which loses npm pre-release rules and PEP 440 `===`); parsing is strict per the specification; `derive --format purl-vers` is a new format, leaving `--format purls` (exact versions) unchanged.
- 2026-11-14: Purl input of type `github` applies to both GitHub Actions and pre-commit (the user's choice), as optional entries. An entry of only exact versions is audited at each by `lookup-bound`. CycloneDX export is deferred.
- 2026-11-14 (small fixes after milestone 20):
  - `resolver = "3"` in `Cargo.toml`: dependency resolution prefers versions whose `rust-version` is at most 1.89 (a fresh `cargo generate-lockfile` stays within it: 153 packages, none newer).
  - v4's top-level `site-install` / `site-uninstall` are accepted (`cli::migrate_args` inserts `python` before the first such argument unless `python` precedes it), like the hidden `--exe`/`-e` aliases.
  - Cargo: a `vendor` directory beside `Cargo.toml` holding `cargo vendor` output (crates with `.cargo-checksum.json`) is skipped by the walk; its crates' lockfiles are not projects.
  - `~/.pulumi` is excluded from the home walk, like `~/.cache` and `~/.npm`: it holds Pulumi's clone of its template repository (one real `go.mod` with 149 requirements, `tests/go.mod`), provider plugins, workspace state and `credentials.json` (never read), and no user projects.
  - `Cargo.toml` excludes `SPEC.md` and `doc/` from the published crate (74 → 62 files).
  - `derive --anchor both`: from the lowest to the highest observed version, inclusive, or an exact version if only one is observed. Python: `name>=lo,<=hi` or `name==v` (`DepSpec::from_package_range`). Other ecosystems (`allow_list::both_spec`): `>=lo, <=hi` for Cargo and Ruby (comma = and), `>=lo <=hi` for npm-style ranges and composer; exact is `=v` for Cargo (a bare version is caret) and the bare version otherwise. Pre-releases a range excludes are added as exact alternatives, as for `lower`/`upper`. Full-home `derive -a lower|upper|both` then `validate` gives 0 findings for each (4,474 entries).
  - Swift allow-list entries accept a revision SHA as an exact alternative (`Requirement::SwiftReq`), and `derive` adds observed revision pins that way (like PHP branch versions). Before this, the `Peekaboo` branch pin was derived as nothing and reported as unrequired.
  - `Package`'s `Ord` breaks ties by exact name, key and `direct_url` (`DirectURL`/`VcsInfo` derive `Ord`), so only equal packages compare `Equal`, consistent with `Eq`. A full-home `scan` has the same rows; 6 pairs of same-name, same-version Python packages (a registry install and a local or editable one) are reordered, registry first.
- 2026-11-14: Swift is the tenth ecosystem (milestone 20); names keep their case for OSV's case-sensitive `SwiftURL`; only versionless pins are checked against tags.
- 2026-11-14: pre-commit is the ninth ecosystem (milestone 19). Hooks are audited through the packages they install (the user's idea), mapped from the hook repository's own files at the rev.
- 2026-11-14: A SHA pin's version comment is checked against its commit; on a mismatch the SHA wins (audit the commit's actual tag, or leave unaudited), with a warning.
- 2026-11-14: PHP is the eighth ecosystem (milestone 17), `-E php`. Names are lowercased; branch versions are unaudited; the `[php]` table uses composer constraint syntax, ported from composer/semver rather than mapped onto npm ranges (composer's `~` differs from npm's).
- 2026-11-14: Ruby is the seventh ecosystem (milestone 16). Gem names are case-sensitive; platforms are removed from versions; the `[ruby]` allow-list table uses RubyGems requirement syntax, with modern (canonical-segment) `Gem::Version` ordering.
- 2026-11-14: Every scanner prunes during the walk, enabled or not; only enabled scanners contribute Origins.
- 2026-11-14: Go is the sixth ecosystem (milestone 15), ahead of Composer, RubyGems and Pub (see §12). It reads `go.mod`, not `go.sum`; the `[go]` allow-list table uses npm ranges over `v`-prefixed versions.
- 2026-11-14: Package caches of every ecosystem (`EcosystemScanner::cache_dirs`) are excluded from the walk whichever ecosystems are enabled.
- 2026-11-14: The release is 5.0.0 (the next major after 4.0.1), not 6.0.0 as first planned.
- 2026-11-14: Milestone 13 generalizes `unpack-*` only; `purge-*` stays Python-only in v5.0. Purging behind a package manager's back is considered a feature, to revisit later.
- 2026-11-13: The ecosystem is selected with a global `-E/--ecosystem` flag. Python-only commands live under `fetter python …`.
- 2026-11-13: `--origin` has no short flag (`-o` is associated with output). `-e`/`--exe` are kept as hidden aliases for v4 compatibility.
- 2026-11-13: Unpack and purge are shared commands in concept; only `site-install` / `site-uninstall` are Python-specific. Generalizing them is milestone 13.
- 2026-11-13: npm/cargo use both installed artifacts and lockfiles, and record the `SiteKind`. (Precisely: installed where it exists, else lockfile; see the npm entry.)
- 2026-11-13: The default scan covers all ecosystems in a single walk.
- 2026-11-13: `inspect`, `validate`, and `derive` are generalized to all ecosystems (hooks; cross-ecosystem allow list).
- 2026-11-13: "exe" generalizes to **Origin**; "site" keeps its name, with a `SiteKind`.
- 2026-11-13: npm sources follow the original choice, "installed where it exists, else lockfile". This corrects the drifted "report both" wording above for npm. Hidden lockfiles are not used.
- 2026-11-13: npm does not report packages inside editor extension dirs; extensions (and their bundled dependencies) belong to the VS Code scanner (milestone 9).
- 2026-11-14: In `lookup-bound`, a requirement whose package is not in its registry is reported as unresolved (it was silently dropped for Python); other failures are errors. `lookup-name` on a missing package remains an error.
- 2026-11-14: `lookup-name` outside Python takes `name@requirement` in native syntax; without -E it is Python.
- 2026-11-14: VS Code versions come from Open VSX (not the Marketplace gallery API).
- 2026-11-14: Resolving GHA SHA pins in `audit` via `git ls-remote` was deferred, then done in milestone 18. A pin resolves to the most specific tag on its commit; an untagged commit stays unaudited.
- 2026-11-14: Allow list is `fetter.toml` with per-ecosystem tables in native syntax; purl lists deferred.
- 2026-11-14: `derive` writes requirements when all derived packages are Python, else `fetter.toml`; `--format` overrides.
- 2026-11-14: Ecosystems absent from a bound are not validated; `--strict` opts in.
- 2026-11-14: Ranges use the `semver` crate (cargo) and `nodejs-semver` (npm, vscode).
- 2026-11-13: VS Code extensions' bundled `node_modules` are reported as npm packages, via an npm-ecosystem site under the VS Code origin.
- 2026-11-13: GHA refs are interpreted by convention for auditing: exact tags, SHA pins with full version comments, floating tags as "newest in line", branches as "at or past newest". Only unresolvable SHAs are unaudited. The alternatives ("exact and comments only", "exact only") would audit 29% or 1.3% of refs.
- 2026-11-13: GHA origins are repo roots only (`.github/workflows` beside `.git`).
- 2026-11-13: Support Bun's text `bun.lock` (the current default); not the binary `bun.lockb`.
- 2026-11-13: YAML via `serde-saphyr` (pure Rust, maintained), replacing deprecated `serde_yaml`; MSRV raised to 1.89 to allow it.
- 2026-11-13: OSV supports VS Code extensions under the ecosystem name `VSCode` (https://osv.dev/list?q=&ecosystem=VSCode), so VS Code packages are fully auditable.

## 11. Breaking changes (for the v5 changelog)

- **MSRV is Rust 1.89** (was 1.88), required by `serde-saphyr`.
- `-e/--exe` → `--origin` (no short flag; old names kept as hidden aliases).
- `audit` / `lookup-*` fail (exit 1) when OSV cannot be queried, instead of reporting no vulnerabilities. Audit JSON may include an `unaudited` list.
- Library API: `AuditReport::from_packages` returns `Result`; `AuditReport` has a new `unaudited` field.
- `count` output is a per-ecosystem table (`Ecosystem,Origins,Sites,Packages`), not `Executables/Sites/Packages` rows.
- `scan`, `search`, `audit` and `lookup-*` tables have a new leading `Ecosystem` column.
- `unpack-count` / `unpack-files` report all ecosystems (installed npm packages, VS Code extensions, `cargo install` binaries), have a new leading `Ecosystem` column, and order rows by package then site path. Packages known only from lockfiles or manifests are noted on stderr.
- `--root` replaces, rather than adds to, the default discovery locations.
- `lookup-bound` reports requirements whose package is not found (e.g. private packages) on stderr and in an `unresolved` JSON list (previously silently omitted for Python), and fails on other registry errors (previously silently omitted).
- `lookup-bound` validates all ecosystems in its bound by default (it was Python-only), and a directory bound uses its `fetter.toml` first.
- `lookup-name` accepts `-E <one ecosystem>`; more than one is an error.
- `validate` output gains an `Ecosystem` column and an `"ecosystem"` field in `validate json`.
- `derive` (without `--format`) writes a `fetter.toml`, not a requirements file, when the scan includes non-Python packages, e.g. a full-system `fetter derive` without `--origin`.
- Library API: `ValidationRecord.dep_spec` is now `dependency: Option<Dependency>`, and `ValidationRecord` has `ecosystem`.
- Delimited output (`write -d`) quotes fields containing the delimiter, a double quote, or a line break (RFC 4180); delimiters `"`, `\n` and `\r` are rejected. `derive write` is unchanged.
- `write` subcommands exit 1 with an error when the file cannot be written (previously silently succeeded).
- `audit` of GitHub Actions SHA pins without a version comment runs `git ls-remote` (requires `git` and network access) and audits them as their commit's tag; `audit json` gains `resolved_pins`. A SHA pin whose version comment does not match its commit is audited as the commit's actual tag (or is unaudited), with a warning; `audit json` gains `pin_mismatches`.
- The JSON of every package gains a `purl` field.
- `lookup-bound` audits every version of an entry whose specs are all exact versions (previously only the newest).
- `inspect` columns are `Ecosystem, Site, Hook, Content` (was `Site, File, Content`); it reports npm install scripts and VS Code startup extensions as well as Python startup files; Python files within a site are sorted.
- `site-install` and `site-uninstall` move under `fetter python` (the v4 top-level spellings are still accepted, rewritten before parsing).
- The default scan includes all ecosystems, not just Python: Python, npm, Cargo, Go, Ruby, PHP, Swift, pre-commit, GitHub Actions and VS Code. `count` has `go`, `ruby`, `php`, `swift` and `precommit` rows. `audit` of pre-commit hooks fetches files from GitHub and `audit json` gains `precommit_hooks`.
- Versions are displayed and serialized as written: a Python version with a leading zero (e.g. a requirement `==2024.01`) is shown as `2024.01`, not `2024.1`; comparisons are unchanged.
- The JSON output of every report gains an `ecosystem` field; the `monitor-scan` payload changes shape.
- `ScanFS` public fields are renamed (`exe_to_sites` → `origin_to_sites`, `site_to_exes` → `site_to_origins`, `exe_to_ems` → `origin_to_ems`); their value types are now `Origin`/`Site`, not `PathShared`.
- `Package` gains `ecosystem`, so its JSON (e.g. in `audit` output) gains an `"ecosystem"` field. `Package` ordering is by ecosystem first.
- The cache format changes (`scan_fs_v5_` prefix); v4 caches are ignored.


## 12. Further ecosystems (survey 2026-11-14)

Candidates after the first five, judged on how well they fit the scanner framework (a file that lists resolved packages), OSV coverage, and the registry needed for `lookup-name`. "Found here" is a survey of the dev machine's home directory (pruning `node_modules`, `.git`, `target`, `Library`, `site-packages`).

| Ecosystem | OSV name | Source | Effort | Found here | Notes |
|---|---|---|---|---|---|
| Go | `Go` | `go.mod` (full build list since Go 1.17); `go install` binaries' build info | low | 21 `go.mod`, 4 `go.sum` | **Done** (milestone 15). Most of the 21 are Pulumi templates (`~/.pulumi/templates/*`, `module ${PROJECT}`) or inside `~/.cargo/registry`; one cloned repo has 4 real modules. `go.sum` over-reports (it keeps hashes of modules not in the build list). Registry: proxy.golang.org (plain-text version list). |
| PHP | `Packagist` | `composer.lock` (JSON); `vendor/composer/installed.json` | very low | 0 | **Done** (milestone 17). Installed + lockfile, like npm. Version semantics were the real work (a port of composer/semver). |
| Ruby | `RubyGems` | `Gemfile.lock` (indented text) | low | 0 | **Done** (milestone 16). Installed gems via `specifications/*.gemspec`; the system Ruby here has 48. |
| Dart / Flutter | `Pub` | `pubspec.lock` (YAML) | very low | 0 | |
| Swift | `SwiftURL` | `Package.resolved` (JSON, v1–v3) | low | 2 | **Done** (milestone 20). OSV names Swift packages by repository URL, case-sensitively. |
| .NET | `NuGet` | `packages.lock.json` (opt-in), `obj/project.assets.json` | moderate | 0 | The lockfile is often absent; `project.assets.json` exists only after a restore. |
| Elixir / Erlang | `Hex` | `mix.lock` (Elixir terms) | moderate | 0 | Needs a small term parser. |
| Java | `Maven` | `pom.xml` (no lockfile); optional `gradle.lockfile` | high | 6 `pom.xml` | Resolution needs the dependency graph and parent POMs; skip unless `gradle.lockfile` is present. |
| pre-commit | none | `.pre-commit-config.yaml` (`repo` + `rev`) | low | 3 | **Done** (milestone 19), audited through the packages hooks install. Originally: no OSV ecosystem, so no audit, but `scan`, `validate` and `lookup-name` (via `git ls-remote`, as for GHA) would work. Shaped like GHA, so most of its ref logic carries over. Relevant to fetter's own users, who run fetter through pre-commit. |

Composer and Pub would take about an afternoon each, but with no local data they could only be verified against fixtures and lockfiles from public repositories, short of the "independent count on a real machine" bar used for the first five.
