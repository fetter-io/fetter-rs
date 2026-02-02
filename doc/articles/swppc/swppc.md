
# System-Wide Python Package Control: Enforce Allow Lists & Find Vulnerabilities
<!-- Introducing the `fetter` command line application -->

A Python developer's system is likely littered with numerous virtual environments and hundreds of packages. Many of these virtual environments might be abandoned, holding out-of-date packages with known security vulnerabilities.

Even within a single virtual environment, installed packages can drift from the project's specified requirements. A developer might mistakenly install a package in the wrong virtual environment, or install a new package that inadvertently upgrades another package. When installed packages deviate from vetted requirements, unexpected behavior can result, or worse, malware can be installed.

The [`fetter`](https://github.com/fetter-io/fetter-rs) command-line application searches an entire system (or targeted virtual environments) for installed Python packages. Once found, those packages can be validated against a requirements or lock file, or audited for security vulnerabilities in the Open Source Vulnerability database. Deployed as a `pre-commit` hook, these checks can be performed on `git` commit or push and integrated into continuous integration workflows. Going further, with `fetter` teams can enforce environment or system-wide package allow listing.

Beyond core validation operations, `fetter` permits searching installed packages, deriving new requirements from observed packages across multiple environments, and unpacking and purging package content.

Similar to `ruff` and `uv`, `fetter` is implemented in efficient multi-threaded Rust, achieving optimal performance on multi-core machines. Compared to implementing requirements validation with the Python `packaging` library, `fetter` can be twice as fast; compared to scanning for vulnerabilities in the OSV database with `pip-audit`, `fetter` can be ten times faster.

## Installing `fetter`

While available as a pure Rust binary ([crates](https://crates.io/crates/fetter)), `fetter` is easily installed via a Python package ([pypi](https://pypi.org/project/fetter)):

```shell
$ pip install fetter
$ fetter --help
```

Alternatively, as `fetter` can operate on multiple virtual environments, installation via [`pipx`](https://pipx.pypa.io) might be desirable:

```shell
$ pipx install fetter
$ fetter --version
```

At this time `fetter` only supports Linux and MacOS systems; support for Windows is planned.

## Scanning Systems and Environments

By default, `fetter` will search for all packages in `site-packages` directories discoverable from all Python executables found in the system or user virtual environments. Depending on your system, this command might take several seconds.

```shell
$ fetter scan
```

The `fetter scan` command finds all installed packages. Observed across an entire system, the results can be surprising. For example, I happen to have eight different versions of the `zipp` package scattered among seventeen virtual environments. (For concise display, virtual environment names are abbreviated.)

```shell
Package      Site
zipp-3.7.0   ~/.env-rt/lib/python3.8/site-packages
             ~/.env-ag/lib/python3.8/site-packages
             ~/.env-qf/lib/python3.8/site-packages
             ~/.env-qa/lib/python3.8/site-packages
zipp-3.8.0   ~/.env-aw/lib/python3.8/site-packages
zipp-3.15.0  ~/.env-gp/lib/python3.8/site-packages
             ~/.env-po/lib/python3.10/site-packages
             ~/.env-yp/lib/python3.8/site-packages
zipp-3.16.0  ~/.env-fb/lib/python3.11/site-packages
zipp-3.16.2  ~/.env-sf/lib/python3.11/site-packages
             ~/.env-te/lib/python3.8/site-packages
             ~/.env-hy/lib/python3.8/site-packages
zipp-3.17.0  ~/.env-sq/lib/python3.12/site-packages
zipp-3.18.1  ~/.env-tp/lib/python3.11/site-packages
             ~/.env-uv/lib/python3.11/site-packages
             ~/.env-wp/lib/python3.12/site-packages
zipp-3.20.2  ~/.env-tl/lib/python3.11/site-packages
```


To limit scanning to `site-packages` directories associated with a specific Python executable, the `--exe` (or `-e`) argument can be supplied. To demonstrate this, we can first build a virtual environment from the following "requirements.txt" content:

```
jinja2==3.1.3
zipp==3.18.1
requests==2.32.3
```

Then, after making that environment active, we can scan the `site-packages` directory of this active Python by providing the argument `-e python3`. As `fetter` reports on all installed packages, we see not only explicit requirements but all dependencies of those requirements, as well as `pip` itself:

```shell
$ fetter -e python3 scan
Package                   Site
certifi-2024.8.30         ~/.env-wp/lib/python3.12/site-packages
charset_normalizer-3.4.0  ~/.env-wp/lib/python3.12/site-packages
idna-3.10                 ~/.env-wp/lib/python3.12/site-packages
jinja2-3.1.3              ~/.env-wp/lib/python3.12/site-packages
markupsafe-2.1.5          ~/.env-wp/lib/python3.12/site-packages
pip-21.1.1                ~/.env-wp/lib/python3.12/site-packages
requests-2.32.3           ~/.env-wp/lib/python3.12/site-packages
setuptools-56.0.0         ~/.env-wp/lib/python3.12/site-packages
urllib3-2.2.3             ~/.env-wp/lib/python3.12/site-packages
zipp-3.18.1               ~/.env-wp/lib/python3.12/site-packages
```

## Validating Installed Packages

Having discovered installed packages, `fetter` can validate them against a list of expected packages. That list, or "bound requirements", can be a "requirements.txt" file, a "pyproject.toml" file, or a lock file created by `uv` or another tool.

For example, to validate that the installed packages match the packages specified in "requirements.txt", we can use the `fetter validate` command, again targeting our active Python with `-e python3`, and providing "requirements.txt" to the `--bound` argument.

```shell
$ fetter -e python3 validate --bound requirements.txt
Package                   Dependency  Explain     Sites
certifi-2024.8.30                     Unrequired  ~/.env-wp/lib/python3.12/site-packages
charset_normalizer-3.4.0              Unrequired  ~/.env-wp/lib/python3.12/site-packages
idna-3.10                             Unrequired  ~/.env-wp/lib/python3.12/site-packages
markupsafe-2.1.5                      Unrequired  ~/.env-wp/lib/python3.12/site-packages
pip-21.1.1                            Unrequired  ~/.env-wp/lib/python3.12/site-packages
setuptools-56.0.0                     Unrequired  ~/.env-wp/lib/python3.12/site-packages
urllib3-2.2.3                         Unrequired  ~/.env-wp/lib/python3.12/site-packages
```

As configured, validation fails with numerous "Unrequired" records: `fetter` found  installed packages that are not defined in the "requirements.txt" file. As this is a common scenario when not using a lock file, the `--superset` command can be provided to accept packages that are not defined in the bound requirements.

```shell
$ fetter -e python3 validate --bound requirements.txt --superset
```

If we happen to update a package outside of the specification of the bound requirements, `fetter` will report these as "Misdefined" records. In the example below, we update `zipp` to version 3.20.2 and re-run validation:

```shell
$ fetter -e python3 validate --bound requirements.txt --superset
Package      Dependency    Explain     Sites
zipp-3.20.2  zipp==3.18.1  Misdefined  ~/.env-wp/lib/python3.12/site-packages
```

If we remove the `zipp` package entirely, `fetter` identifies this as a "Missing" record:

```shell
$ fetter -e python3 validate --bound requirements.txt --superset
Package  Dependency    Explain  Sites
         zipp==3.18.1  Missing
```

If we want to permit the absence of specified packages, the `--subset` flag can be used:

```shell
$ fetter -e python3 validate --bound requirements.txt --superset --subset
```

For greater control, bound requirements can be a lock file, which is expected to fully specify all packages and their dependencies. To help derive a bound requirements file from a system (or virtual environment), the `fetter derive` command can be used. Bound requirements can be stored on the local file system, fetched from a URL, or pulled from a `git` repository. Validating installed packages provides an important check that developer environments conform to a  project's expectations.

The `fetter validate` command can be deployed as a `git` hook with `pre-commit`: just specify in your ".pre-commit-config.yaml" the `fetter-io/fetter-rs` repo, the `fetter-validate` hook, and additional configuration `args`:

```yaml
repos:
- repo: https://github.com/fetter-io/fetter-rs
  rev: v1.0.0
  hooks:
    - id: fetter-validate
      args: [--bound, requirements.txt, --superset]

```

## Auditing Package Vulnerabilities

In addition to validation, `fetter` can audit packages for known security vulnerabilities defined in the Open Source Vulnerability (OSV) database. Using the `fetter audit` command, details are provided for every vulnerability associated with installed packages.

```shell
$ fetter -e python3 audit
Package            Vulnerabilities      Attribute  Value
jinja2-3.1.3       GHSA-h75v-3vvj-5mfj  URL        https://osv.dev/vulnerability/GHSA-h75v-3vvj-5mfj
                                        Summary    Jinja vulnerable to HTML attribute injection when passing ...
                                        Reference  https://nvd.nist.gov/vuln/detail/CVE-2024-34064
                                        Severity   CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:U/C:L/I:L/A:N
pip-21.1.1         GHSA-mq26-g339-26xf  URL        https://osv.dev/vulnerability/GHSA-mq26-g339-26xf
                                        Summary    Command Injection in pip when used with Mercurial
                                        Reference  https://nvd.nist.gov/vuln/detail/CVE-2023-5752
                                        Severity   CVSS:4.0/AV:L/AC:L/AT:N/PR:L/UI:N/VC:N/VI:H/VA:N/SC:N/SI:N/SA
                   PYSEC-2023-228       URL        https://osv.dev/vulnerability/PYSEC-2023-228
                                        Reference  https://mail.python.org/archives/list/security-announce@py...
                                        Severity   CVSS:3.1/AV:L/AC:L/PR:L/UI:N/S:U/C:N/I:L/A:N
setuptools-56.0.0  GHSA-cx63-2mw6-8hw5  URL        https://osv.dev/vulnerability/GHSA-cx63-2mw6-8hw5
                                        Summary    setuptools vulnerable to Command Injection via package URL
                                        Reference  https://nvd.nist.gov/vuln/detail/CVE-2024-6345
                                        Severity   CVSS:4.0/AV:N/AC:L/AT:P/PR:N/UI:A/VC:H/VI:H/VA:H/SC:N/SI:N/SA
                   GHSA-r9hx-vwmv-q579  URL        https://osv.dev/vulnerability/GHSA-r9hx-vwmv-q579
                                        Summary    pypa/setuptools vulnerable to Regular Expression Denial of...
                                        Reference  https://nvd.nist.gov/vuln/detail/CVE-2022-40897
                                        Severity   CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H
                   PYSEC-2022-43012     URL        https://osv.dev/vulnerability/PYSEC-2022-43012
                                        Reference  https://github.com/pypa/setuptools/blob/fe8a98e696241487ba...
zipp-3.18.1        GHSA-jfmj-5v4g-7637  URL        https://osv.dev/vulnerability/GHSA-jfmj-5v4g-7637
                                        Summary    zipp Denial of Service vulnerability
                                        Reference  https://nvd.nist.gov/vuln/detail/CVE-2024-5569
                                        Severity   CVSS:4.0/AV:L/AC:L/AT:N/PR:N/UI:N/VC:N/VI:N/VA:H/SC:N/SI:N/SA
```

While tools such as `pip-audit` can provide similar information, `fetter` offers more details and significant performance advantages.

Just as with `fetter validate`, this operation can be configured to run as a `git` hook. While perhaps not appropriate to run on every commit, deployment as a pre-push check might be desirable. As before, all that is necessary is to add the hook in your ".pre-commit-config.yaml" file.

```yaml
repos:
- repo: https://github.com/fetter-io/fetter-rs
  rev: v1.0.0
  hooks:
    - id: fetter-audit
```

## Other Utilities

The `fetter` CLI exposes a number of additional utilities to explore system-wide Python package information. For example, the `fetter count` command can be used to get metrics on discovered executables, site-packages directories, and packages:

```shell
$ fetter count
             Count
Executables  67
Sites        45
Packages     1420
```

To discover, for example, all versions of NumPy installed on my system, I can use `fetter search` with with the glob-style pattern "numpy-*".

```shell
$ fetter search -p numpy-*
Package       Site
numpy-1.18.5  ~/.env-ag/lib/python3.8/site-packages
numpy-1.19.5  ~/.env-qa/lib/python3.8/site-packages
numpy-1.22.0  ~/.env-qf/lib/python3.8/site-packages
numpy-1.22.2  ~/.env310/lib/python3.10/site-packages
numpy-1.22.4  ~/.env-te/lib/python3.8/site-packages
numpy-1.23.5  ~/.env-hy/lib/python3.8/site-packages
              ~/.env-tn/lib/python3.9/site-packages
              ~/.env-gp/lib/python3.8/site-packages
              ~/.env-yp/lib/python3.8/site-packages
              ~/.env-tl/lib/python3.11/site-packages
              ~/.env-np/lib/python3.10/site-packages
numpy-1.24.2  ~/.env-er/lib/python3.11/site-packages
              ~/.env-aw/lib/python3.8/site-packages
              ~/.env-am/lib/python3.8/site-packages
numpy-1.24.3  ~/.env-tl/lib/python3.11/site-packages
              ~/.env-ak/lib/python3.8/site-packages
              ~/.env-uv/lib/python3.11/site-packages
numpy-1.24.4  ~/.env-rt/lib/python3.8/site-packages
numpy-1.25.1  ~/.env-sf/lib/python3.11/site-packages
numpy-1.26.0  ~/.env-fb/lib/python3.11/site-packages
numpy-1.26.2  ~/.env-tt/lib/python3.12/site-packages
              ~/.env-rr/lib/python3.12/site-packages
numpy-1.26.4  ~/.env-sg/lib/python3.11/site-packages
              ~/.env-df/lib/python3.12/site-packages
numpy-2.0.0   ~/.env-tt/lib/python3.12/site-packages
              ~/.env-sq/lib/python3.12/site-packages
numpy-2.1.2   ~/.env-lt/lib/python3.11/site-packages
```

Having 15 different versions of NumPy in 27 virtual environments might be undesirable. Using `fetter unpack-count`, we can view how many files are associated with a particular package.

```shell
$ fetter unpack-count -p numpy-1.18.5
Package       Site                                   Files  Dirs
numpy-1.18.5  ~/.env-ag/lib/python3.8/site-packages  855    2
```

Using `fetter purge-pattern`, we can remove all files associated with a package, equivalent to uninstalling that package. Unlike conventional package managers, it is possible to remove packages across all virtual environments on a system:

```shell
$ fetter purge-pattern -p numpy-1.18.5
```


## Delimited File Output

All previous examples demonstrate the default `fetter` behavior to print output to the terminal. Alternatively, output can be written to delimited text files suitable for further processing. To write the output of the `fetter search` command to a CSV file, we simply include the "write" subcommand and additional arguments:

```shell
$ fetter search -p numpy-* write -o /tmp/out.txt -d ","
$ cat /tmp/out.txt
Package,Site
numpy-1.19.5,~/.env-qa/lib/python3.8/site-packages
numpy-1.22.0,~/.env-qf/lib/python3.8/site-packages
numpy-1.22.2,~/.env310/lib/python3.10/site-packages
numpy-1.22.4,~/.env-te/lib/python3.8/site-packages
numpy-1.23.5,~/.env-np/lib/python3.10/site-packages
numpy-1.23.5,~/.env-yp/lib/python3.8/site-packages
numpy-1.23.5,~/.env-gp/lib/python3.8/site-packages
numpy-1.23.5,~/.env-hy/lib/python3.8/site-packages
numpy-1.23.5,~/.env-tn/lib/python3.9/site-packages
numpy-1.23.5,~/.env-tl/lib/python3.11/site-packages
numpy-1.24.2,~/.env-am/lib/python3.8/site-packages
numpy-1.24.2,~/.env-er/lib/python3.11/site-packages
numpy-1.24.2,~/.env-aw/lib/python3.8/site-packages
numpy-1.24.3,~/.env-tl/lib/python3.11/site-packages
numpy-1.24.3,~/.env-ak/lib/python3.8/site-packages
numpy-1.24.3,~/.env-uv/lib/python3.11/site-packages
numpy-1.24.4,~/.env-rt/lib/python3.8/site-packages
numpy-1.25.1,~/.env-sf/lib/python3.11/site-packages
numpy-1.26.0,~/.env-fb/lib/python3.11/site-packages
numpy-1.26.2,~/.env-rr/lib/python3.12/site-packages
numpy-1.26.2,~/.env-tt/lib/python3.12/site-packages
numpy-1.26.4,~/.env-df/lib/python3.12/site-packages
numpy-1.26.4,~/.env-sg/lib/python3.11/site-packages
numpy-2.0.0,~/.env-tt/lib/python3.12/site-packages
numpy-2.0.0,~/.env-sq/lib/python3.12/site-packages
numpy-2.1.2,~/.env-lt/lib/python3.11/site-packages
```

## Conclusion

Should I be concerned that my system has eight different versions of `zipp`, or fifteen different versions of `numpy`? Maybe: both of those packages have documented vulnerabilities across numerous versions. I might return to these environments and execute code now known to have security vulnerabilities, potentially putting my system at risk of exploit or malware.

Rather than building-up this out-of-date package "debt," enforcing environment or system-wide controls on what packages can be installed might be better. This form of Python package allow listing, at a minimum, ensures that all developers work within the same environment; at a maximum, packages can be controlled across entire systems. The `fetter` command-line tool offers an efficient utility for this purpose.
























