

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