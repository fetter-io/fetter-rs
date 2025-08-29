

# Find the Highest Severity Python Package on Your System
# The Highest Severity Python Package on Your System
# What's the Most Dangerous Python Package on Your System?


The `fetter` command-line application can find the highest-severity Python packages on your system, across all Pythons and virtual environments:

```
$ fetter audit --cvss
```

On my system, I find two packages with CVSS scores of 9.1: `flask_applbuilder-4.3.6` and `h11-0.14.0`.



While there are a number of tools to evaluate vulnerable Python packages defined in requirements or lock files, `fetter` takes a bottom-up, system-wide approach, searching for all Python executables, all site packages associated with those executables, and all installed packages.

Countless Python packages have security vulnerabilities, but the severity of those vulnerabilities can be diverse.



```bash
{.env311}{default} % cargo run -- audit --cvss


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