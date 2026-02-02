
# What Is the Most Dangerous Python Package on Your System?

The Common Vulnerability Scoring System (CVSS) is a widely used metric to rank the severity of software vulnerabilities from 0 to 10. If you work with Python, you have Python dependencies on your system, and it is likely that some of those packages have vulnerabilities. But which vulnerabilities are important?

With the following command, the `fetter` command-line application can find, among all Python packages on your system, the dependencies with the highest CVSS scores:

```bash
$ fetter audit --cvss
```

Fetter can be installed in a Python environment with `pip install fetter` or in an isolated environment with `pipx install fetter`. Using `uvx`, the command can also be run in an ephemeral environment with `uvx fetter audit --cvss`.

On my system, I find two packages with "critical" CVSS scores of 9.1: `flask_applbuilder` 4.3.6 and `h11` 0.14.0. Fetter provides high level information and relevant links:

```bash
$ fetter audit --cvss
Package                 Vulnerabilities      Attribute  Value
flask_appbuilder-4.3.6  GHSA-j2pw-vp55-fqqj  URL        https://osv.dev/vulnerability/GHSA-j2pw-vp55-fqqj
                                             Summary    Flask-AppBuilder vulnerable to incorrect authentication when using auth type OpenID
                                             Reference  https://nvd.nist.gov/vuln/detail/CVE-2024-25128
                                             Severity   CVSS 9.1 (Critical): CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:N
h11-0.14.0              GHSA-vqfr-h8mv-ghfj  URL        https://osv.dev/vulnerability/GHSA-vqfr-h8mv-ghfj
                                             Summary    h11 accepts some malformed Chunked-Encoding bodies
                                             Reference  https://nvd.nist.gov/vuln/detail/CVE-2025-43859
                                             Severity   CVSS 9.1 (Critical): CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:N
```

But are these dangerous? The `flask_appbuilder` 4.3.6 package, if deployed in a web app, permits an attacker to forge an HTTP request to use an arbitrary OpenID service and gain unauthorized privileged access. The `h11` 0.14.0 package, an HTTP/1.1 protocol library, permits request smuggling under some conditions, potentially allowing an attacker to circumvent reverse proxy controls to get to a protected endpoint. So while these packages are not likely dangerous run locally, they certainly should not be deployed.

To find where packages reside, you can use the `fetter search` command:

```bash
$ fetter search -p h11*
```

To fully uninstall all packages matching a pattern, you can use the `fetter purge-pattern` command:

```bash
$ fetter purge-pattern -p h11-0.14*
```

While there are many tools to evaluate vulnerable Python packages defined in `pyproject.toml`, requirements, or lock files, `fetter` takes a bottom-up, system-wide approach, searching for all Python executables, all site packages associated with those executables, and all installed packages. This finds packages in active projects as well as abandoned Python installations or forgotten virtual environments, all potential sources of malware or vulnerable code.
