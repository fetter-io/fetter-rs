# fetter

<a href="https://github.com/fetter-io/fetter-rs/actions/workflows/ci.yml">
    <img style="display: inline!important" src="https://img.shields.io/github/actions/workflow/status/fetter-io/fetter-rs/ci.yml?branch=default&label=CI&logo=Github"></img>
</a>
<a href="https://crates.io/crates/fetter">
    <img src="https://img.shields.io/crates/v/fetter?label=crates.io&logo=rust"></img>
</a>
<a href="https://pypi.org/project/fetter/">
    <img src="https://img.shields.io/pypi/v/fetter?label=PyPI&logo=pypi"></img>
</a>

## System-wide Package Discovery, Validation, and Allow-Listing

The `fetter` command-line tool provides comprehensive open-source supply-chain visibility by finding all packages on a system and checking them against allow lists and the Open Source Vulnerabilities (OSV) database. It covers Python environments, npm projects, Rust (Cargo) projects and installed crates, Go modules, Ruby projects and installed gems, PHP (Composer) projects, Swift packages, pre-commit hooks, GitHub Actions workflows, VS Code extensions, and browser extensions (Chromium and Firefox families), all in a single fast, multi-threaded scan.

As an alternative to `pip-audit` for Python and related tools for different ecosystems, `fetter` can look up vulnerabilities for packages defined in requirements.txt, pyproject.toml, or lock files created by `uv`, `pixi`, `poetry`, `pipenv`, or `pip-tools`, and for packages of every supported ecosystem in a `fetter.toml` allow list. Full vulnerability details, including CVSS scores, are provided.

Additionally, `fetter` can configure a Python virtual environment to validate package alignment before every Python run, enforcing a locked and reproducible development environment.

* 🔎 System Scanning: Finds Python, npm, Cargo, Go, Ruby, PHP, Swift, pre-commit, GitHub Actions, VS Code, and browser extension packages across a system in one pass.
* 🛡️ Vulnerability Audit: Scans defined or installed packages of every ecosystem for security vulnerabilities in the OSV database.
* ⚖️ Package Validation: Checks packages against a cross-ecosystem `fetter.toml` allow list, or Python packages against requirements.txt, pyproject.toml, or lock files created by `uv`, `pixi`, `poetry`, `pipenv`, or `pip-tools`, sourced locally, via URLs, or via `git` repositories.
* 🪝 Hook Inspection: Reports code that runs without being invoked: Python startup files, npm install scripts, Ruby gems with native extensions or RubyGems plugins, Composer plugins, and VS Code extensions that activate on every editor start.
* 🔒 Locked & Reproducible Environments: Automatically validate Python packages against a lock file before every Python run.
* ⚙️ CI Integration: Validate and audit with `pre-commit` [hooks](#Using-fetter-with-pre-commit).
* 🚀 Fast: Multi-threaded Rust implementation.
* 🪢 Bound Requirements: Derive an allow list from observed system packages.
* 🧹 Search and Purge: Find packages and their installed files across environments, and remove Python packages.
* 🧩 Flexible Output: Display results in terminal or export to delimited files; JSON output carries a package URL (purl) for every package.
* 📦 SBOMs: Export a scan of every ecosystem as a CycloneDX SBOM; audit or validate against a CycloneDX or SPDX SBOM, or a list of purls with optional VERS version ranges.


## Articles

* [What Is the Most Dangerous Python Package on Your System?](https://python.plainenglish.io/what-is-the-most-dangerous-python-package-on-your-system-03d7e3ea2000)
* [System-Wide Python Package Control: Enforce Allow Lists & Find Vulnerabilities](https://levelup.gitconnected.com/system-wide-python-package-control-enforce-allow-lists-find-vulnerabilities-35f3daeb44a8)
* [Guarantee a Locked & Reproducible Environment with Every Python Run](https://medium.com/data-science-collective/guarantee-a-locked-reproducible-environment-with-every-python-run-c0e2bf19fb53)


## Installing the `fetter` Command Line Application

While available as a pure Rust binary ([crates](https://crates.io/crates/fetter)), `fetter` is easily installed via a Python package ([pypi](https://pypi.org/project/fetter)):

```shell
$ pip install fetter
$ fetter --help
```

As `fetter` can operate across multiple virtual environments, installation via [`pipx`](https://pipx.pypa.io) might be desirable:

```shell
$ pipx install fetter
$ fetter --version
```

An "ephemeral" `fetter` installation and run is also possible with [`uvx`](https://docs.astral.sh/uv/guides/tools):

```shell
$ uvx fetter --version
```

Or, with a Rust toolchain:

```shell
$ cargo install fetter
```


## Supported Ecosystems

Every command works on all ecosystems unless limited with `-E` / `--ecosystem` (e.g. `-E python` or `-E npm,cargo`). For each ecosystem, `fetter` finds *origins* (the things that use packages) and their *sites* (where those packages are recorded or installed).

| Ecosystem (`-E`) | Origin | Site | Packages |
|---|---|---|---|
| `python` | A Python executable or virtual environment | Its `site-packages` directories | `*.dist-info` directories |
| `npm` | A project directory with a `package.json`, or a global install directory | Its `node_modules`, if installed; otherwise its lock file | Installed `package.json` files; `package-lock.json` (v1–v3), `npm-shrinkwrap.json`, `pnpm-lock.yaml`, `yarn.lock` (classic and berry), `bun.lock` |
| `cargo` | A project directory with a `Cargo.lock`, or `~/.cargo` | `Cargo.lock`; for `~/.cargo`, the record of `cargo install`ed crates | Registry and git dependencies (workspace and path crates are excluded) |
| `go` | A module directory with a `go.mod` | Its `go.mod` | Requirements, direct and indirect, after `replace` directives (modules replaced by a local directory are excluded) |
| `ruby` | A project directory with a `Gemfile.lock` (or `gems.locked`), or a gem directory (`~/.gem/ruby/3.3.0`, an rbenv or rvm Ruby's gems, `vendor/bundle/ruby/3.3.0`, system gem directories, RubyInstaller on Windows) | The lockfile; for a gem directory, the directory | Locked gems (`GEM`, `GIT`, and plugin sources; local `PATH` gems are excluded); installed `.gemspec` files, including Ruby's default gems |
| `php` | A project directory with a `composer.json` and a `composer.lock` or an installed vendor directory; or a vendor directory on its own | The vendor directory (`vendor`, or `config.vendor-dir`), if installed; otherwise `composer.lock` | `composer/installed.json`; `composer.lock` (`packages` and `packages-dev`; path packages are excluded). Branch versions (`dev-main`) are not audited |
| `swift` | A directory with a `Package.resolved` (a SwiftPM package, or an Xcode project's `xcshareddata/swiftpm`) | The `Package.resolved` | Remote pins, named by repository URL as OSV names them (`github.com/apple/swift-nio`; case-sensitive, as written); local pins are excluded; branch and revision pins are resolved to tags where possible |
| `precommit` | A directory with a `.pre-commit-config.yaml` | The config | Hook repositories (`repo` and `rev`; `local` and `meta` are excluded), named by URL (`github.com/psf/black`). Audited through the packages they install (see below). Hooks' pinned `additional_dependencies` are reported in their own ecosystems (see below) |
| `gha` | A repository root with `.github/workflows` | Each workflow and action file | `uses: owner/repo@ref` steps and jobs |
| `vscode` | An editor directory (`~/.vscode`, `~/.cursor`, `~/.windsurf`, …) | Its `extensions` directory | Installed extensions, as `publisher.name`; their bundled `node_modules` are reported as npm packages |
| `chromium` | A browser profile (`Default`, `Profile 1`, …) of Chrome, Edge, Brave, Vivaldi, Opera, Arc, Chromium, or Yandex, on macOS, Linux (including Snap and Flatpak), or Windows | The profile | Extensions from the profile's `Preferences` and `Secure Preferences`, including unpacked (developer mode) ones; built-in component extensions are excluded. Named by extension id (`cjpalhdlnbpafiamejdnhcphjbkeiagm`) |
| `firefox` | A profile of Firefox, LibreWolf, Waterfox, Zen, Floorp, or Thunderbird | The profile | Extensions from the profile's `extensions.json`; add-ons built into the browser are excluded. Named by add-on id (`uBlock0@raymondhill.net`) |

Browser profiles hold private data. For browser extensions, `fetter` reads only the files that describe extensions (for Chromium, `Preferences` and `Secure Preferences`, of which only the extension settings are used, and each extension's `manifest.json`; for Firefox, `extensions.json`) and keeps nothing but the packages found. OSV has no advisories for browser extensions, so they are not audited; `inspect`, `validate`, and `derive` apply.

By default, `fetter` searches the home directory (and, for Python, executables on the `PATH`). Use `--origin` to name specific origins, or `--root` to search only specific directories:

```shell
$ fetter --origin .venv scan      # one Python environment
$ fetter --origin . scan          # the npm, Cargo, or GitHub Actions project here
$ fetter --root ~/src scan        # search only ~/src
```


## Using `fetter` to Replace `pip-audit`

The `fetter lookup-bound` command can perform numerous operations similar to `pip-audit`, though with broader file format support and (by using the OSV Database) greater vulnerability details (including CVSS scores).

To audit dependencies for a local project (a `fetter.toml` is used first, then a Python lock file, requirements.txt, or pyproject.toml):

```shell
$ fetter lookup-bound
```

To audit dependencies for a specified requirements file:

```shell
$ fetter lookup-bound requirements.txt
```

To audit dependencies for a lock file created by `uv`, `pixi`, `poetry`, `pipenv`, or `pip-tools`:

```shell
$ fetter lookup-bound uv.lock
```

To audit dependencies defined in an online git repository:

```shell
$ fetter lookup-bound git@github.com:psf/clabot.git
```

Requirements whose package is not found in its registry (for example, a private package) are reported on stderr as unresolved and are not audited.

`lookup-bound` also accepts an SBOM (CycloneDX or SPDX 2.x JSON) or a list of package URLs (purls, one per line), auditing every package version listed, in any supported ecosystem:

```shell
$ fetter lookup-bound bom.cdx.json
```

A purl in a list may be followed by a [VERS](https://github.com/package-url/vers-spec) version range of the same type instead of a version, which `lookup-bound` audits at the newest version in each interval (CycloneDX 1.7 components' `versionRange` are read the same way):

```text
pkg:npm/lodash vers:npm/>=4.17.0|<4.17.21
pkg:pypi/requests vers:pypi/2.19.0|>=2.31|<2.32
pkg:github/actions/checkout vers:github/v4|11bd71901bbe5b1630ceea73d27597364c9af683
```

The `fetter lookup-name` command finds vulnerabilities for a single package or version, or for every version matching a requirement, with versions listed from the ecosystem's registry (PyPI, npm, crates.io, the Go module proxy, rubygems.org, Packagist, git tags, or Open VSX). Without `-E`, the name is a Python requirement; for other ecosystems, give `name@requirement` in that ecosystem's syntax, or a purl, which names its ecosystem (a `pkg:github` purl is a GitHub Action, or a pre-commit hook with `-E precommit`):

```shell
$ fetter lookup-name "pip>=25"
$ fetter -E npm lookup-name "lodash@>=4.17.15 <4.17.22"
$ fetter -E cargo lookup-name "time@=0.1.43"
$ fetter -E go lookup-name "golang.org/x/net@>=v0.20.0 <v0.24.0"
$ fetter -E ruby lookup-name "nokogiri@~> 1.16.0"
$ fetter -E php lookup-name "guzzlehttp/guzzle@~7.4.0"
$ fetter lookup-name "pkg:npm/lodash@4.17.15"
$ fetter -E swift lookup-name "github.com/sparkle-project/Sparkle@>=2.6.0 <2.9.0"
$ fetter -E gha lookup-name "tj-actions/changed-files@v35"
```


## Using `fetter` to Scan, Validate, and Audit Packages

For complete command-line documentation, see [CLI Documentation](#Command-Line-Interface-Documentation).

By default, `fetter` finds all packages of all ecosystems in the user's home directory and in Python environments on the system. Depending on your system, this command might take several seconds.

```shell
$ fetter scan
```

The examples below use a small project, `~/demo`, with a Python virtual environment, an npm lock file, a `Cargo.lock`, and a GitHub Actions workflow. Using `--root` limits the search to that directory:

```shell
$ fetter --root ~/demo scan
Ecosystem  Package                       Site
python     certifi-2026.10.23            ~/demo/.venv/lib/python3.12/site-packages
python     charset_normalizer-3.5.2      ~/demo/.venv/lib/python3.12/site-packages
python     idna-3.20                     ~/demo/.venv/lib/python3.12/site-packages
python     jinja2-3.1.3                  ~/demo/.venv/lib/python3.12/site-packages
python     markupsafe-3.0.4              ~/demo/.venv/lib/python3.12/site-packages
python     requests-2.32.3               ~/demo/.venv/lib/python3.12/site-packages
python     urllib3-2.8.0                 ~/demo/.venv/lib/python3.12/site-packages
python     zipp-3.18.1                   ~/demo/.venv/lib/python3.12/site-packages
npm        lodash-4.17.15                ~/demo/package-lock.json
cargo      time-0.1.43                   ~/demo/Cargo.lock
gha        actions/checkout-v4           ~/demo/.github/workflows/ci.yml
gha        tj-actions/changed-files-v35  ~/demo/.github/workflows/ci.yml
```

The `count` command summarizes what was found, by ecosystem:

```shell
$ fetter --root ~/demo count
Ecosystem  Origins  Sites  Packages
python     1        1      8
npm        1        1      1
cargo      1        1      1
gha        1        1      2
vscode     0        0      0
go         0        0      0
ruby       0        0      0
php        0        0      0
precommit  0        0      0
swift      0        0      0
all        4        4      12
```


### Validating Python Packages

The Python environment was built from this "requirements.txt":

```
jinja2==3.1.3
zipp==3.18.1
requests==2.32.3
```

To validate that the installed packages match the packages specified in "requirements.txt", we can use the `fetter validate` command, targeting the virtual environment with `--origin .venv`, and providing "requirements.txt" to the `--bound` argument.

```shell
$ fetter --origin .venv validate --bound requirements.txt
Ecosystem  Package                   Dependency  Explain     Sites
python     certifi-2026.10.23                    Unrequired  ~/demo/.venv/lib/python3.12/site-packages
python     charset_normalizer-3.5.2              Unrequired  ~/demo/.venv/lib/python3.12/site-packages
python     idna-3.20                             Unrequired  ~/demo/.venv/lib/python3.12/site-packages
python     markupsafe-3.0.4                      Unrequired  ~/demo/.venv/lib/python3.12/site-packages
python     urllib3-2.8.0                         Unrequired  ~/demo/.venv/lib/python3.12/site-packages
```

The `--superset` flag can be provided to accept packages that are not defined in the bound requirements.

```shell
$ fetter --origin .venv validate --bound requirements.txt --superset
```

If we update `zipp` to version 3.20.2 and re-run validation, `fetter` will report it as a "Misdefined" record:

```shell
$ fetter --origin .venv validate --bound requirements.txt --superset
Ecosystem  Package      Dependency    Explain     Sites
python     zipp-3.20.2  zipp==3.18.1  Misdefined  ~/demo/.venv/lib/python3.12/site-packages
```

If we remove the `zipp` package entirely, `fetter` identifies this as a "Missing" record:

```shell
$ fetter --origin .venv validate --bound requirements.txt --superset
Ecosystem  Package  Dependency    Explain  Sites
python              zipp==3.18.1  Missing
```

If we want to permit the absence of specified packages, the `--subset` flag can be used:

```shell
$ fetter --origin .venv validate --bound requirements.txt --superset --subset
```


### Validating All Ecosystems with `fetter.toml`

A `fetter.toml` allow list has one table per ecosystem. Python requirements use PEP 508 syntax; other entries map a package name to a requirement in its ecosystem's native syntax (npm ranges for npm and VS Code, Cargo requirements for Cargo, and refs for GitHub Actions). Go has no range syntax of its own, so Go entries use npm ranges over Go versions, which keep their `v` (`">=v1.2.3 <v2"`). Ruby entries use RubyGems requirements (`"~> 7.1"`, `">= 1.0, < 2"`), PHP entries use Composer constraints (`"^5.4 || ^6.2"`, `"~7.4"`, `"dev-main"`), Swift entries use npm-style ranges (`">=2.6.4 <3"`, or a revision SHA for a branch pin), and browser extension entries (`chromium`, `firefox`) use comma-separated comparisons in the browser's version order (`">=1.62, <2"`, an exact version, or `"*"` for any version). Go and Ruby names are case-sensitive, as in their registries. An array means "any of".

The `derive` command writes a `fetter.toml` from the packages found (a requirements file, if all of them are Python packages; use `--format` to choose). The `--anchor` (`-a`) argument sets whether observed versions become lower bounds (`lower`), upper bounds (`upper`), or both (`both`: the range from the lowest to the highest version observed, or an exact version if only one is):

```shell
$ fetter --root ~/demo derive -a lower write -o fetter.toml
```

After editing the npm and GitHub Actions entries to require patched versions, the allow list reads:

```toml
[python]
requirements = [
    "certifi>=2026.10.23",
    "charset_normalizer>=3.5.2",
    "idna>=3.20",
    "jinja2>=3.1.3",
    "markupsafe>=3.0.4",
    "requests>=2.32.3",
    "urllib3>=2.8.0",
    "zipp>=3.18.1",
]

[npm]
"lodash" = ">=4.17.21"

[cargo]
"time" = ">=0.1.43"

[gha]
"actions/checkout" = ["v4"]
"tj-actions/changed-files" = "v46"
```

Validating against it reports the packages that fall outside the allow list:

```shell
$ fetter --root ~/demo validate --bound fetter.toml --superset
Ecosystem  Package                       Dependency                    Explain     Sites
npm        lodash-4.17.15                lodash >=4.17.21              Misdefined  ~/demo/package-lock.json
gha        tj-actions/changed-files-v35  tj-actions/changed-files v46  Misdefined  ~/demo/.github/workflows/ci.yml
```

An ecosystem that the allow list does not mention is not validated; with `--strict`, it is validated as if it allowed no packages.

A `fetter.toml` can also list package URLs, each allowing exactly its version, the versions in its VERS range, or any version if it has neither, as alternatives to the tables' entries. A `pkg:github` purl allows that repository as a GitHub Action or a pre-commit hook, and is not reported missing if neither is found:

```toml
purls = [
    "pkg:npm/lodash@4.17.21",
    "pkg:pypi/django vers:pypi/3.2.25|>=4.2|<5",
    "pkg:github/actions/checkout vers:github/v4|v4.2.2",
]
```

A VERS range can also be one of a table entry's specs (`"rails" = ["vers:gem/>=7.0|<7.1", "~> 6.1.7"]`). VERS compares versions by order alone, so a pre-release inside an interval is in the range, unlike in npm or Cargo range syntax. Versions with no order (GitHub refs, Swift revision SHAs, Composer `dev-` branches) may be listed exactly.

`validate --bound` likewise accepts an SBOM or a purl list. `derive --format purls` writes the purls of all package versions found, and `derive --format purl-vers` writes one purl per package with a VERS range from the anchor (`>=` the lowest version, `<=` the highest, or both), so a purl list can serve as a complete allow list for every ecosystem:

```shell
$ fetter derive -a both --format purl-vers write -o allow.txt
$ fetter validate --bound allow.txt
```

Python environment markers, extras and direct-URL requirements have no purl or VERS form; use a `fetter.toml` or requirements file for those.

`derive --format cyclonedx` writes a [CycloneDX](https://cyclonedx.org) 1.6 JSON SBOM of every package version found, in all ecosystems, for other SBOM tools or as a bound for `validate` and `lookup-bound`. Each component has its purl, and each site where it was found is an `evidence.occurrences` entry (with its kind, installed, lockfile or manifest, in `additionalContext`); the ecosystem and any VCS commit are `fetter:` properties. The anchor does not apply:

```shell
$ fetter derive -a lower --format cyclonedx write -o sbom.json
$ fetter validate --bound sbom.json
```


### Auditing Packages

Using the `fetter audit` command, details are provided for every vulnerability associated with the packages found.

```shell
$ fetter --root ~/demo -E npm,cargo,gha audit
Ecosystem  Package                       Vulnerabilities      Attribute  Value
npm        lodash-4.17.15                GHSA-29mw-wpgm-hmr9  URL        https://osv.dev/vulnerability/GHSA-29mw-wpgm-hmr9
                                                              Summary    Regular Expression Denial of Service (ReDoS) in lodash
                                                              Reference  https://nvd.nist.gov/vuln/detail/CVE-2020-28500
                                                              Severity   CVSS 5.3 (Medium): CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:L
                                         GHSA-35jh-r3h4-6jhm  URL        https://osv.dev/vulnerability/GHSA-35jh-r3h4-6jhm
                                                              Summary    Command Injection in lodash
                                                              Reference  https://nvd.nist.gov/vuln/detail/CVE-2021-23337
                                                              Severity   CVSS 7.2 (High): CVSS:3.1/AV:N/AC:L/PR:H/UI:N/S:U/C:H/I:H/A:H
...
cargo      time-0.1.43                   GHSA-wcg3-cvx6-7396  URL        https://osv.dev/vulnerability/GHSA-wcg3-cvx6-7396
                                                              Summary    Segmentation fault in time
                                                              Reference  https://nvd.nist.gov/vuln/detail/CVE-2020-26235
                                                              Severity   CVSS 6.2 (Medium): CVSS:3.1/AV:L/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H
...
gha        tj-actions/changed-files-v35  GHSA-mcph-m25j-8j63  URL        https://osv.dev/vulnerability/GHSA-mcph-m25j-8j63
                                                              Summary    tj-actions/changed-files has Potential Actions command injection in output filenam…
                                                              Reference  https://nvd.nist.gov/vuln/detail/CVE-2023-51664
                                                              Severity   CVSS 7.3 (High): CVSS:3.1/AV:N/AC:L/PR:L/UI:R/S:U/C:H/I:H/A:N
...
```

GitHub Actions refs are matched to advisories by convention: an exact tag (`@v4.1.1`), a SHA pin with a version comment (`@<sha> # v4.1.1`), or a floating tag (`@v4`, taken as the newest release in that line). A SHA pin without a version comment is resolved with `git ls-remote` to the tag its commit is released as (the most specific, e.g. `v4.1.1` over `v4`), and audited as that version; the resolutions are listed on stderr and in `audit json` (`resolved_pins`). This requires `git`. A pin whose commit is not tagged, or whose repository cannot be listed, is listed on stderr as not audited. A pin with a version comment is checked the same way: if the tag the comment names is not on the pinned commit (a stale comment, or one written to mislead), `fetter` warns, and audits the version the commit is actually tagged as (`pin_mismatches` in `audit json`).


### Auditing pre-commit Hooks

OSV has no pre-commit ecosystem, so `audit` maps each hook repository on GitHub to the packages it installs, and audits those in their own ecosystems. The repository's `.pre-commit-hooks.yaml` at the configured `rev` gives the hooks' languages; the language's manifest gives the package: `psf/black` at `rev: 23.1.0` is PyPI `black==23.1.0`, and wrapper repositories such as `astral-sh/ruff-pre-commit` or `pre-commit/mirrors-mypy` give the tools they pin (`ruff==0.1.13`, `mypy==1.8.0`). Python, node, Go, and Rust hooks are mapped; others (Docker, system, script) are listed as not audited. Frozen revs (`rev: <sha>  # frozen: v1.2.3`) are checked against their tags as for GitHub Actions pins. This requires network access to GitHub and `git`.

A hook's `additional_dependencies` in the `.pre-commit-config.yaml` itself (in any repository, `local` included) are installed into the hook's environment, so pinned ones are reported as packages of their own ecosystems, with the config as their site: `scan`, `audit`, `validate`, and `derive` see them without network access. The hook's `language`, if the config gives it, decides the ecosystem; otherwise the form does: `name==1.2.3` is Python, `name@1.2.3` npm, and `host.tld/path@v1.2.3` Go (on GitHub, GitLab, and Bitbucket the module is the repository root). With `language: ruby`, `name:1.2.3` is a gem. Unpinned entries (`types-requests`, `attrs>=23`) are not reported, since the version installed is not known; nor are Rust dependencies, whose versions are requirements.

```shell
$ fetter -E precommit audit
$ fetter -E precommit lookup-name "github.com/psf/black@23.1.0"
```

### Inspecting Hooks

The `inspect` command reports code that runs without being explicitly invoked: Python `.pth` files with executable lines and `sitecustomize` / `usercustomize` modules, npm packages with install scripts, Ruby gems with native extensions or RubyGems plugins, Composer plugins, VS Code extensions that activate on every editor start, and browser extensions (see below).

```shell
$ fetter --root ~/demo inspect
Ecosystem  Site                                       Hook             Content
python     ~/demo/.venv/lib/python3.12/site-packages  _virtualenv.pth  import _virtualenv
```

A browser extension is reported if it can read every site (`<all_urls>`, `*://*/*`), holds a strong permission (`debugger`, `management`, `nativeMessaging`, `proxy`), was installed outside the store (unpacked, sideloaded, or loaded temporarily), or is unsigned. Its content gives the extension's name, its access to all sites, notable permissions (such as `cookies` and `webRequest`), how it was installed, and whether it is disabled or uses Manifest V2. In a terminal, content is truncated; `write` saves it in full:

```shell
$ fetter -E firefox inspect write -o hooks.txt -d '|'
Ecosystem|Site|Hook|Content
firefox|~/Library/Application Support/Firefox/Profiles/kokrnh29.default|plugin@okta.com-6.49.0|Okta Browser Plugin; all sites: http://*/, https://*/; permissions: cookies, webNavigation, webRequest, webRequestBlocking; source: addons.mozilla.org
```


## Installing Automatic Environment Validation

The `fetter python site-install` command can be used to configure a Python environment to automatically perform pre-run validation against a requirements.txt, pyproject.toml, or lock file created by `uv`, `poetry`, `pipenv`, or `pip-tools`. Once installed, packages will be validated before every Python execution, warning or optionally exiting if the environment is invalid.

The `site-install` command takes the same arguments as `validate`. Automatic validation is implemented by installing a special ".pth" file in the virtual environment associated with the provided Python executable.

For example, the following command will validate packages installed for the currently available `python3` against a "requirement.txt" lock file and issue warnings:

```shell
$ fetter --origin python3 python site-install --bound requirements.txt
```

For stronger control, the `exit` subcommand can be added to force process termination on validation errors.

```shell
$ fetter --origin python3 python site-install --bound requirements.txt exit
```

To uninstall automatic environment validation, run `site-uninstall` with the same Python executable:

```shell
$ fetter --origin python3 python site-uninstall
```


## Using `fetter` with `pre-commit`

Two `fetter` commands can be run via [pre-commit](https://pre-commit.com/) hooks for continuous integration of Python package controls.


### Running `fetter validate` with `pre-commit`.


The `fetter validate` command permits validating that the actually installed Python packages in the current environment are what are defined to be installed, as specified by a requirements.txt file, a pyproject.toml file, or a lock file produced by `uv`, `poetry`, `pipenv`, or `pip-tools`.

The `fetter validate` command takes a required argument, `--bound`, to specify that path or URL to the file to be used to define the bound requirements. The optional `--superset` argument permits packages not defined in the bound requirements to be present. The optional `--subset` argument permits not all packages in the bound requirements to be present.

To run `fetter validate` with `pre-commit`, add the following to your `.pre-commit-config.yaml`.


```yaml
repos:
- repo: https://github.com/fetter-io/fetter-rs
  rev: v5.0.0
  hooks:
    - id: fetter-validate
      args: [--bound, {FILE}, --superset, --subset, display, --code, 3]
```


### Running `fetter audit` with `pre-commit`.

The `fetter audit` command will check for cybersecurity vulnerabilities issued for all installed Python packages in the current environment. Vulnerabilities are searched for in the Open Source Vulnerability (OSV) database.

To run `fetter audit` with `pre-commit`, add the following to your `.pre-commit-config.yaml`. Note that, as searching vulnerabilities can take time, this hook is likely better deployed as a `pre-push` rather than a `pre-commit` hook.

```yaml
repos:
- repo: https://github.com/fetter-io/fetter-rs
  rev: v5.0.0
  hooks:
    - id: fetter-audit
```



## Command Line Interface Documentation

### Global Options

- `--ecosystem, -E <ECOSYSTEMS>`: Limit to one or more ecosystems (`python`, `npm`, `cargo`, `go`, `ruby`, `php`, `swift`, `precommit`, `gha`, `vscode`, `chromium`, `firefox`), as a comma-separated list or a repeated flag. If omitted, all ecosystems are used.
- `--origin <PATHS>`: Provide zero or more origin paths, such as a Python executable or virtual environment, or a project directory. The ecosystem of each path is inferred; a directory can be an origin for several ecosystems. If omitted, all discoverable origins are used. (`--exe` and `-e` are accepted as aliases.)
- `--root <DIRECTORIES>`: Search only these directories (recursively) for origins, instead of the home directory and system locations.
- `--quiet, -q`: Disable terminal animations.
- `--user-site`: Force inclusion of the Python user site-packages, even if it is not activated. Defaults to only including if the interpreter is configured to use it.
- `--all-users`: When searching for all discoverable origins, include all user directories. Otherwise, include only the user's home directory.
- `--cache-duration, -c`: Create or use caches that expire after the provided number of seconds. A duration of zero will disable caching.
- `--cache-directory`: Provide an explicit directory to be used for storing caches.
- `--log, -l`: Enable logging output.
- `--stderr`: Force all output to stderr.

### Command: `fetter scan`

- Description: Scan the environment to report on packages found.
- Subcommands
  - `display`: Show scan results in the terminal.
  - `write`: Save scan results to a file.
    - `--output, -o <FILE>`: Specify the output file.
    - `--delimiter, -d <char>`: Set the delimiter for the file (default: `,`).

### Command: `fetter inspect`

- Description: Inspect all sites for code that runs without being explicitly invoked: Python startup files, npm install scripts, Ruby gems with native extensions (built at install) or RubyGems plugins (loaded by `gem` commands), Composer plugins (run by `composer install`), and VS Code extensions that activate on every editor start.
- Subcommands
  - `display`: Show inspect results in the terminal.
  - `write`: Save inspect results to a file.
    - `--output, -o <FILE>`: Specify the output file.
    - `--delimiter, -d <char>`: Set the delimiter for the file (default: `,`).

### Command: `fetter search`

- Description: Search the environment to report on packages based on a pattern.
- Options
  - `--pattern, -p <STRING>`: Specify a glob-like pattern to match packages.
  - `--case`: Enable case-sensitive pattern matching.
- Subcommands
  - `display`: Show search results in the terminal.
  - `write`: Save search results to a file.
    - `--output, -o <FILE>`: Specify the output file.
    - `--delimiter, -d <char>`: Set the delimiter for the file (default: `,`).

### Command: `fetter count`

- Description: Count discovered origins, sites, and packages, by ecosystem.
- Subcommands
  - `display`: Show count results in the terminal.
  - `write`: Save count results to a file.
    - `--output, -o <FILE>`: Specify the output file.
    - `--delimiter, -d <char>`: Set the delimiter for the file (default: `,`).

### Command: `fetter derive`

- Description: Derive an allow list from discovered packages.
- Options
  - `--anchor, -a <BOUND>`: Set the nature of the bound in the derived requirements: `lower`, `upper`, or `both` (from the lowest to the highest observed version, or exact if only one).
  - `--format <FORMAT>`: Write a `requirements` file (Python packages only), a `fetter.toml` (`toml`, all ecosystems), a list of package URLs (`purls`, every observed package version; the anchor does not apply), a list of purls with VERS ranges (`purl-vers`, one per package; the anchor applies), or a CycloneDX 1.6 JSON SBOM (`cyclonedx`, every observed package version with its sites; the anchor does not apply). If omitted, a requirements file is written when all derived packages are Python, and a `fetter.toml` otherwise.
- Subcommands
  - `display`: Show derived requirements in the terminal.
  - `write`: Save derived requirements to a file.
    - `--output, -o <FILE>`: Specify the output file.

### Command: `fetter validate`

- Description: Validate if packages conform to a specified validation target.
- Options
  - `--bound, -b <FILE>`: Path or URL to the file containing bound requirements: a `fetter.toml`, an SBOM (CycloneDX or SPDX 2.x JSON), a list of purls, or a requirements.txt, pyproject.toml or lock file created by `uv`, `poetry`, `pipenv`, or `pip-tools` (Python only).
  - `--bound-options <OPTIONS>`: Names of additional optional dependency groups.
  - `--ignore <OPTIONS>`: Names of packages to be excluded from all evaluation.
  - `--subset`: Allow the observed packages to be a subset of the bound requirements.
  - `--superset`: Allow the observed packages to be a superset of the bound requirements.
  - `--strict`: Validate every scanned ecosystem, treating an ecosystem the bound does not mention as having no allowed packages.
- Subcommands
  - `display`: Show validation results in the terminal.
    - `--code, -c <INT>`: Optionally specify and exit with an error code (default: `3`).
  - `json`: Print validation results in JSON format.
  - `write`: Save validation results to a file.
    - `--output, -o <FILE>`: Specify the output file.
    - `--delimiter, -d <char>`: Set the delimiter for the file (default: `,`).
  - `exit`: Return an exit code (0 for success, customizable for errors).
    - `--code, -c <INT>`: Specify the error code (default: `3`).

### Command: `fetter audit`

- Description: Search for security vulnerabilities in packages via the OSV DB.
- Options
  - `--pattern, -p <STRING>`: Specify a glob-like pattern to select packages (default: `*`).
  - `--case`: Enable case-sensitive pattern matching.
  - `--cache-refresh`: Ignore any OSV caches and re-fetch vulnerability details
  - `--cvss`: Filter vulnerabilities to those greater or equal to a provided CVSS score. If no argument is provided, the maximum is reported.
- Subcommands
  - `display`: Show audit results in the terminal.
  - `json`: Print audit results in JSON format.
  - `write`: Save audit results to a file.
    - `--output, -o <FILE>`: Specify the output file.
    - `--delimiter, -d <char>`: Set the delimiter for the file (default: `,`).
  - `exit`: Return an exit code (0 for success, customizable for errors).
    - `--code, -c <INT>`: Specify the error code (default: `3`).

### Command: `fetter lookup-name`

- Description: Search for security vulnerabilities for a package name or requirement via the OSV DB, with versions listed from the ecosystem's registry.
- Options
  - `<NAME>`: A Python package name or dependency specification; with `-E <ECOSYSTEM>` (one only), a name or `name@requirement` in that ecosystem's syntax; or a package URL (`pkg:npm/lodash@4.17.15`), which names its ecosystem.
  - `--limit <INT>`: If the package does not specify a version, determine how many recent versions to audit.
  - `--retain-passing`: Show a record for all packages, even if the package has no vulnerabilities.
  - `--cache-refresh`: Ignore any OSV caches and re-fetch vulnerability details.
  - `--cvss`: Filter vulnerabilities to those greater or equal to a provided CVSS score. If no argument is provided, the maximum is reported.
- Subcommands
  - `display`: Show lookup results in the terminal.
  - `json`: Print lookup results in JSON format.
  - `write`: Save lookup results to a file.
    - `--output, -o <FILE>`: Specify the output file.
    - `--delimiter, -d <char>`: Set the delimiter for the file (default: `,`).
  - `exit`: Return an exit code (0 for success, customizable for errors).
    - `--code, -c <INT>`: Specify the error code (default: `3`).

### Command: `fetter lookup-bound`

- Description: Search for security vulnerabilities for all packages in a bound requirements file via the OSV DB.
- Options
  - `<SOURCE>`: Directory, file path, URL, or git repository from which to read bound requirements: a `fetter.toml`, an SBOM (CycloneDX or SPDX 2.x JSON), a list of purls, or a requirements.txt, pyproject.toml or lock file created by `uv`, `poetry`, `pipenv`, or `pip-tools`. If no argument is provided (or a directory is provided), the directory is searched for a `fetter.toml`, then a lock file, requirements.txt, or pyproject.toml. Requirements whose package is not in its registry are reported as unresolved.
  - `--bound-options <OPTIONS>`: Names of additional optional dependency groups.
  - `--retain-passing`: Show a record for all packages, even if the package has no vulnerabilities.
  - `--cache-refresh`: Ignore any OSV caches and re-fetch vulnerability details.
  - `--cvss`: Filter vulnerabilities to those greater or equal to a provided CVSS score. If no argument is provided, the maximum is reported.
- Subcommands
  - `display`: Show lookup results in the terminal.
  - `json`: Print lookup results in JSON format.
  - `write`: Save lookup results to a file.
    - `--output, -o <FILE>`: Specify the output file.
    - `--delimiter, -d <char>`: Set the delimiter for the file (default: `,`).
  - `exit`: Return an exit code (0 for success, customizable for errors).
    - `--code, -c <INT>`: Specify the error code (default: `3`).

### Command: `fetter unpack-count`

- Description: Count all installed package artifacts: the files of Python packages (from `RECORD`), npm package directories, installed Ruby gems (their directories, built extensions, spec, and cached `.gem`), installed Composer package directories, VS Code extension directories, and `cargo install`ed binaries. Packages known only from lock files or workflows have no installed artifacts; their number is noted on stderr.
- Options
  - `--pattern, -p <STRING>`: Specify a glob-like pattern to select packages (default: `*`).
  - `--case`: Enable case-sensitive pattern matching.

- Subcommands
  - `display`: Show artifact counts in the terminal.
  - `write`: Save artifact counts to a file.
    - `--output, -o <FILE>`: Specify the output file.
    - `--delimiter, -d <char>`: Set the delimiter for the file (default: `,`).

### Command: `fetter unpack-files`

- Description: List the file names of all installed package artifacts (see `unpack-count`).
- Options
  - `--pattern, -p <STRING>`: Specify a glob-like pattern to select packages (default: `*`).
  - `--case`: Enable case-sensitive pattern matching.
- Subcommands
  - `display`: Show artifact file names in the terminal.
  - `write`: Save artifact file names to a file.
    - `--output, -o <FILE>`: Specify the output file.
    - `--delimiter, -d <char>`: Set the delimiter for the file (default: `,`).

### Command: `fetter purge-pattern`

- Description: Purge packages that match a specific pattern. Only Python packages are removed.
- Options
  - `--pattern, -p <STRING>`: Specify a glob-like pattern to select packages (default: `*`).
  - `--case`: Enable case-sensitive pattern matching.

### Command: `fetter purge-invalid`

- Description: Purge packages that are invalid based on dependency specification. Only Python packages are removed.
- Options
  - `--bound, -b <FILE>`: Path or URL to the file containing bound requirements: a `fetter.toml`, or a requirements.txt, pyproject.toml or lock file created by `uv`, `poetry`, `pipenv`, or `pip-tools`.
  - `--bound-options <OPTIONS>`: Names of additional optional dependency groups.
  - `--subset`: Allow the observed packages to be a subset of the bound requirements.
  - `--superset`: Allow the observed packages to be a superset of the bound requirements.

### Command: `fetter python site-install`

- Description: Install automatic validation checks on every Python run. This command operates on exactly one Python environment.
- Options
  - `--bound, -b <FILE>`: Path or URL to the file containing bound requirements, which can be a requirements.txt, pyproject.toml or a lock file created by `uv`, `poetry`, `pipenv`, or `pip-tools`.
  - `--bound-options <OPTIONS>`: Names of additional optional dependency groups.
  - `--ignore <OPTIONS>`: Names of packages to be excluded from all evaluation.
  - `--subset`: Allow the observed packages to be a subset of the bound requirements.
  - `--superset`: Allow the observed packages to be a superset of the bound requirements.
- Subcommands
  - `warn`: Configure site-install to print warnings on validation errors.
  - `exit`: Configure site-install to return an exit code on validation errors.
    - `--code, -c <INT>`: Specify the error code (default: `3`).

### Command: `fetter python site-uninstall`

- Description: Uninstall automatic validation checks.

### Command: `fetter monitor-scan`

- Description: Periodically scan the system and post the JSON scan output to a URL. The payload is a versioned JSON object (schema 1: the system, the time, and the scan's origins, sites, packages, and the hooks `inspect` reports for each site; the scan is `null` if unchanged since the last post). Readers ignore what they do not know, so newer payloads can be read by older readers; `fetter::MonitorPayload::from_json` also reads posts from fetter 3 and 4, as Python packages.
- Options
  - `--period, -p <SECONDS>`: Set the period of the scan in seconds.
  - `--url, -u <URL>`: Provide the URL to which to post results.
  - `--tenant, -t <TENANT>`: Provide the tenant key.

### Command: `fetter monitor-diff`

- Description: Monitor this system for changes. Each run (or each period) scans, compares the scan to a baseline, and reports what changed: origins, sites, and packages added or removed, upgraded, downgraded, or changed, and hooks added or removed. Changes are evaluated:
  - every package is audited each report, and vulnerability IDs are compared to the baseline's, so advisories newly published against unchanged packages are found as well as those of changed packages;
  - new vulnerabilities, new hooks (code that runs without being invoked), downgrades, and new allow-list violations (with `--bound`) are alerts.
- The baseline is, by default, the last reported state, kept in the cache directory for each scan configuration and advanced only after a report is written; the first run establishes it, reporting existing vulnerabilities. `--baseline FILE` compares to a fixed snapshot written by `monitor-diff save`, which is never changed.
- With `--period 0` (the default) `monitor-diff` reports once and exits with `3` if the report is an alert, which suits cron (which also mails a job's output). A live view or a notifier (email, chat, webhooks) is a separate program reading the `json` event stream.
- Options
  - `--period, -p <SECONDS>`: Seconds between scans; `0` scans and reports once.
  - `--report-period <SECONDS>`: Seconds between reports; defaults to the scan period.
  - `--baseline <FILE>`: A fixed baseline written by `monitor-diff save`.
  - `--report <change|always>`: Report only when something changed (default), or every report period.
  - `--bound, -b <FILE>`: Bound requirements to validate against (as for `validate`), with `--bound-options`, `--ignore`, `--subset`, `--superset`, and `--strict`.
  - `--cvss[=<SCORE>]`: Filter vulnerabilities as for `audit`.
  - `--control stdin`: Read commands from stdin while waiting between scans, one per line: `scan` (scan now), `report` (scan and report now), and `stop` (end monitoring). For a program that runs `monitor-diff` as a child process; at the end of input, scanning continues on the period. Requires a period above 0.
- Subcommands
  - `text`: Print a text report (the default); nothing is printed when nothing changed.
  - `json`: Write a JSON Lines event stream, one object per line with `schema`, `event`, `time`, and `system`. Events are `start` (configuration and baseline time), `scan` (after every scan: its `trigger`, `start`, `period`, or `request`; counts of origins, sites and packages, in total and per ecosystem, as `count` reports them; and the changes since the previous scan), `report` (the evaluated changes since the baseline; each new or resolved vulnerability names the sites holding its package, as `sites`), and `error`. Readers should ignore events and fields they do not know.
    - `--output, -o <FILE>`: Append to a file (flushed per line) instead of writing to stdout.
  - `save`: Write a baseline of the current system to a file.
    - `--output, -o <FILE>`: The file to write.

Examples:
```
fetter monitor-diff                                     # report changes since the last run
fetter monitor-diff save -o golden.json                 # a fixed baseline
fetter monitor-diff --baseline golden.json --bound fetter.toml
fetter monitor-diff --period 600 --report-period 86400 json -o ~/fetter-events.jsonl
fetter monitor-diff --period 600 --control stdin json   # driven by a supervising program
```

## What is New in Fetter

### 5.2.0

The cross-ecosystem allow list is public library API: `AllowList` (read from `fetter.toml`, `pyproject.toml`, or Python requirements and lock text with `AllowList::from_text`; derived with `AllowList::from_packages`; written with `to_toml`), `ReqEntry`, `Dependency`, and `ValidationReport::from_allow_list` for validating packages and their sites without a `ScanFS`. `Package::new` is public, and `Package::purl()` gives a package's purl.

### 5.1.0

Extended diff report to include sites.

### 5.0.0

Support for multiple ecosystems: npm, Cargo, Go, Ruby, PHP, Swift, pre-commit hooks, GitHub Actions, VS Code extensions, and Chromium and Firefox browser extensions, alongside Python. pre-commit hooks are audited through the packages they install, and their pinned `additional_dependencies` are reported in their own ecosystems. All are found in a single scan; use `-E` / `--ecosystem` to select ecosystems.

All commands work across ecosystems: `scan`, `search`, `count`, `inspect`, `audit`, `derive`, `validate`, `lookup-name`, `lookup-bound`, `unpack-count`, and `unpack-files`. `purge-pattern` and `purge-invalid` remove only Python packages.

New `monitor-diff` command: compare the system to a baseline, once or periodically, and report changes, new vulnerabilities (including newly published advisories), new hooks, downgrades, and allow-list violations, as text or a JSON Lines event stream for other programs.

New `fetter.toml` allow list for `validate`, `derive`, `lookup-bound`, and `purge-invalid`, with one table per ecosystem in native requirement syntax; `validate --strict` validates ecosystems the allow list does not mention.

`lookup-name` lists versions from npm, crates.io, the Go module proxy, rubygems.org, Packagist, git tags, and Open VSX, given `-E <ECOSYSTEM>` and `name@requirement`.

Package URLs (purls): `lookup-name` accepts a purl; `lookup-bound`, `validate`, and `purge-invalid` accept CycloneDX and SPDX 2.x JSON SBOMs and purl lists as bounds; a `fetter.toml` can list `purls`; `derive --format purls` writes them; JSON output gives each package's `purl`. Purls may carry VERS version ranges in purl lists and `fetter.toml`, and `derive --format purl-vers` writes them. `derive --format cyclonedx` exports a CycloneDX 1.6 SBOM of all ecosystems.

`inspect` reports npm install scripts, Ruby gems with native extensions or RubyGems plugins, Composer plugins, VS Code extensions that activate on every editor start, and browser extensions with access to all sites, strong permissions, or an install outside the store, in addition to Python startup files. `inspect write` saves each hook's content in full.

OSV queries are paginated, and failures to query OSV or a registry are errors rather than reported as no vulnerabilities. In `lookup-bound`, requirements whose package is not in its registry are reported as unresolved.

Breaking changes:

* `--exe` / `-e` is replaced by `--origin` (with no short flag); the old names remain as hidden aliases. The new `--root` option, when given, replaces the default search locations.
* `site-install` and `site-uninstall` move to `fetter python site-install` and `fetter python site-uninstall` (the old spellings are still accepted).
* The default scan includes all ecosystems, not only Python.
* Tables of `scan`, `search`, `audit`, `lookup-*`, `validate`, and `unpack-*` have a leading `Ecosystem` column; `count` reports origins, sites, and packages per ecosystem; `inspect` columns are `Ecosystem, Site, Hook, Content`. JSON output gains `ecosystem` fields. The `monitor-scan` payload is a versioned JSON object (schema 1) with named fields and per-site hooks.
* `derive`, without `--format`, writes a `fetter.toml` when the scan includes non-Python packages.
* JSON output gives each package a `purl` field.
* `audit` and `lookup-*` exit with an error when OSV cannot be queried; audit JSON can include `unaudited` and `unresolved` lists.
* Delimited output quotes fields as needed (RFC 4180), and `write` fails with an error when the file cannot be written.
* Scan caches from prior versions are not read.
* The minimum supported Rust version is 1.89.


### 4.0.1

Updated `sha2` and `toml` dependencies.


### 4.0.0

Support for Windows added.


### 3.5.0

Additional robustness in identifying and parsing `uv` lock files.


### 3.4.0

Testing and building on Python 3.14


### 3.3.0

Testing and building on Python 3.14


### 3.2.0

Extension to the public library interface.


### 3.1.0

Extended `lookup-bound` subcommand to, by default, search the current working directory for a bound requirements file; alternatively, a path to a project directory can now be provided.


### 3.0.0

Added `inspect` subcommand to display all code files runnable on interpreter startup.

Added `lookup-name` subcommand to display all vulnerabilities for a single package.

Added `lookup-bound` subcommand to display all vulnerabilities for all packages in bound requirements, which can be a requirements.txt, pyproject.toml or a lock file created by `uv`, `poetry`, `pipenv`, or `pip-tools`


### 2.9.0

Improvements to `CacheConfig` struct.


### 2.8.0

Extension to the public library interface.


### 2.7.0

Extension to the public library interface.


### 2.6.0

Added `all-users` command-line flag to expand default search to all users.

Added `cache-directory` command-line flag to specify cache directory.


### 2.5.0

Extension to the public library interface.


### 2.4.0

Extension to the public library interface.


### 2.3.0

Extension to the public library interface.


### 2.2.0

Extension to the public library interface.


### 2.1.0

Extension to the public library interface.


### 2.0.2

Updated `cvss` dependency.


### 2.0.1

Update for CI.


### 2.0.0

The `audit` command now features a `--cvss` flag to filter results be CVSS score.

The `audit` command now displays CVSS score, severity, and vector.

The `--cache-duration` parameter is now used to set the cache duration of vulnerabilities per package obtained from the OSV DB.

The `audit` command is now robust to requests with no packages.

The `monitor_scan` command now properly identifies duplicated scans.

The `monitor_scan` command now defaults to a single execution with a `--period` of zero.


### 1.22.0

Extension to the public library interface.


### 1.21.0

Extension to the public library interface.


### 1.20.0

Added caching of OSV vulnerability data.

Added `--cache-refresh` parameter to `audit` command.

Implemented minimization of file path usage in `ScanFS` JSON representation.


### 1.19.0

Extension to the public library interface.


### 1.18.0

Extension to the public library interface.


### 1.17.0

Extension to the public library interface.


### 1.16.0

Added `tenant` argument to `monitor-scan`.


### 1.15.0

Extension to the public library interface.


### 1.14.0

Extension to the public library interface.


### 1.13.0

`SystemTag` now properly captures hostname and OS version on MacOS.


### 1.12.0

Extension to the public library interface.


### 1.11.0

Extension to the public library interface.


### 1.10.0

Extension to the public library interface.


### 1.9.0

Environment markers in `Pipfile.lock` files are now imported.

Support for lock files created by Pixi.

Added the `exit` subcommand to the `audit` command.

Implemented `SystemTag`

Implemented the `monitor-scan` command.


### 1.8.0

Added the `--ignore` parameter to `site-install`.

Added the `--stderr` flag to optionally divert all output to stderr.

Updated `site-install` to always use `--stderr`.


### 1.7.0

Added the `--ignore` parameter to `validate` to permit bypassing validation of specified packages. Defaults to ignore `pip`.


### 1.6.0

Better file path validation and error reporting.

Limit `site-install` and `site-uninstall` to only operate on single-Python environment selections.

Support for nearly all dependency environment markers.


### 1.5.0

Extended support for lock files provided to `--bound` to include `uv.lock`, `poetry.lock`, `Pipfile.lock`, as well as lock files produced by `uv pip compile` and `pip-tools`.

Improved loading of `--bound` files sourced from a `git` repository to find and load various lock file formats as well as `pyproject.toml`.

Improved loading of `--bound` files sourced from a URL to support various lock file formats as well as `pyproject.toml`.


### 1.4.0

Improved `site-install` to run in the same parent Python process.

Added `code` option to `display` to only optionally return an error code.


### 1.3.0

Implemented `site-install` to permit installation of package validation on every Python run.

Implemented `site-uninstall` to remove installation of package validation.

Added `log` option to display logging information.

Added support for `poetry`-style dependency specifications with `^` and `~`.

Usage of `-S` in sub-processed Python calls for better isolation and performance.


### 1.2.0

Improvements to path expansion.

Implemented `cache-duration` to permit creating and using a locally stored JSON scan cache for the provided duration.


### 1.1.0

Implemented `bound-options` to permit selecting optional dependencies in pyproject.toml files.

### 1.0.0

Implemented home-path display abbreviation with `~`.

Handle combining multiple `DepSpec` in producing `DepManifest`.

Added `--pattern` and `--case` options to `audit`.

Added support for creating `DepManifest` from pyproject.toml.

Permit `DepManifest` to be retrieved from a URL.


### 0.13.0

All subcommands now have their output sub-subcommands set to `display` by default.

The `validate` and `audit` subcommands now return a non-zero exit code when items are found.

The CLI now exits for unsupported platforms.


### 0.12.0

Extended `validate` and `audit` commands to return a non-zero error code if `display` prints records.


### 0.11.0

Implemented variable-width and colored terminal displays.

Implemented terminal spinner for long-running commands.

Added `purge-invalid` and `purge-pattern` commands.

Split `unpack` command into `unpack-count` and `unpack-files`.

Added support to specify `--bound` with a git repository.


### 0.10.0

Added `--user-site` flag to force inclusion of user site packages; otherwise, user site packages are only included if `ENABLE_USER_SITE` is set.

Reimplemented display and delimited table outputs to use a generic trait implementation.


### 0.9.0

Support `--requirement` in requirements files.


### 0.8.0

Package and DepSpec comparisons now remove user.

Package and DepSpec comparisons now accept matching either on revision or commit_id.

URLs are now shown in DepSpec displays.

Delimited file output no longer pads spaces.


### 0.7.0

Validate display now shows paths properly.

Updated validate json output to terminate line and flush buffer.


### 0.6.0

Package and dependency keys are case insensitive.

Improved URL validation between dependency and package by removing user components.

Improved validation JSON output to provided labelled objects.

Improved valiation output to show sorted missing packages.

Renamed validation explain values.

Implemented support for nested requirements.txt.


### 0.5.0

Implemented search command with basic wildcard matching.

Implemented `Arc`-wrapped `PathBuf` for sharable site paths.

Added explanation column to validation results.

Added support for both `--subset` and `--superset` validations.

Implemented `ValidationDigest` for simplified JSON serialization.

Added `JSON` CLI output option for validation results.






