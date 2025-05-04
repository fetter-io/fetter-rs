import os
import site
import sys
from packaging.requirements import Requirement
from packaging.version import Version, InvalidVersion
from importlib import metadata
from packaging.utils import canonicalize_name


def parse_requirements(file_path):
    """Parse the requirements.txt file and return a list of Requirement objects."""
    requirements = []
    with open(file_path, 'r') as file:
        for line in file:
            line = line.strip()
            if line and not line.startswith("#"):
                try:
                    requirements.append(Requirement(line))
                except Exception as e:
                    print(f"Failed to parse requirement '{line}': {e}")
    return requirements

def get_installed_packages():
    """Return a dictionary of installed packages and their versions."""
    packages = {canonicalize_name(dist.name): dist.version for dist in metadata.distributions()}
    return packages



def validate_requirements(requirements, installed_packages):
    """Validate each requirement against installed packages."""
    for req in requirements:
        package_name = req.name
        if package_name in installed_packages:
            installed_version = Version(installed_packages[package_name])
            if not req.specifier.contains(installed_version, prereleases=True):
                print(f"{package_name} {installed_version} is NOT compatible with {req}")
        else:
            print(f"{package_name} is not installed")


if __name__ == '__main__':
    file_path = "requirements.txt"
    requirements = parse_requirements(file_path)
    installed_packages = get_installed_packages()
    validate_requirements(requirements, installed_packages)


# fetter is 35% faster, or takes 75 percent the time

# {.env311-fetter-bench}{default} % time /home/ariza/.env311-fetter-bench/bin/fetter -e python3 validate --bound requirements.txt --superset
# real    0m0.202s
# user    0m0.060s
# sys     0m0.042s
# {.env311-fetter-bench}{default} % time python3 validate_native.py
# real    0m0.304s
# user    0m0.254s
# sys     0m0.040s

# unlike pip-audit, fetter searches all installed packages, not just what is in requirements.
# takes 14% time for osv, or 7.14 times faster

# {.env311-fetter-bench}{default} % time pip-audit -s osv
# Found 21 known vulnerabilities in 11 packages
# Name         Version ID                  Fix Versions
# ------------ ------- ------------------- ------------
# aiohttp      3.9.5   GHSA-jwhx-xcg6-8xhj 3.10.2
# cryptography 41.0.3  PYSEC-2023-254      41.0.6
# cryptography 41.0.3  GHSA-3ww4-gg4f-jr7f 42.0.0
# cryptography 41.0.3  GHSA-6vqw-3v5j-54x4 42.0.4
# cryptography 41.0.3  GHSA-9v9h-cgj8-h64p 42.0.2
# cryptography 41.0.3  GHSA-h4gh-qq45-vh27 43.0.1
# cryptography 41.0.3  GHSA-v8gr-m533-ghj9 41.0.4
# idna         3.5     PYSEC-2024-60       3.7
# jinja2       3.1.2   GHSA-h5c8-rqwp-cp95 3.1.3
# jinja2       3.1.2   GHSA-h75v-3vvj-5mfj 3.1.4
# pillow       10.1.0  GHSA-3f63-hfp8-52jq 10.2.0
# pillow       10.1.0  GHSA-44wm-f244-xhp3 10.3.0
# pyarrow      13.0.0  PYSEC-2023-238      14.0.1
# requests     2.31.0  GHSA-9wx4-h78v-vm56 2.32.0
# setuptools   68.2.0  GHSA-cx63-2mw6-8hw5 70.0.0
# tqdm         4.66.0  GHSA-g7vv-2v7x-gj9p 4.66.3
# werkzeug     3.0.0   PYSEC-2023-221      2.3.8,3.0.1
# werkzeug     3.0.0   GHSA-2g68-c3qc-8985 3.0.3
# werkzeug     3.0.0   GHSA-f9vj-2wh5-fj8j 3.0.6
# werkzeug     3.0.0   GHSA-q34m-jh98-gwm2 3.0.6
# zipp         3.16.0  GHSA-jfmj-5v4g-7637 3.19.1

# real    0m50.022s
# user    0m2.923s
# sys     0m0.255s



# {.env311-fetter-bench}{default} % time /home/ariza/.env311-fetter-bench/bin/fetter -e python3 audit
# Package              Vulnerabilities      Attribute  Value
# aiohttp-3.9.5        GHSA-jwhx-xcg6-8xhj  URL        https://osv.dev/vulnerability/GHSA-jwhx-xcg6-8xhj
#                                           Summary    In aiohttp, compressed files as symlinks are not protected from path traversal
#                                           Reference  https://nvd.nist.gov/vuln/detail/CVE-2024-42367
#                                           Severity   CVSS:4.0/AV:N/AC:H/AT:P/PR:N/UI:N/VC:L/VI:L/VA:N/SC:N/SI:N/SA:N
# cryptography-41.0.3  GHSA-3ww4-gg4f-jr7f  URL        https://osv.dev/vulnerability/GHSA-3ww4-gg4f-jr7f
#                                           Summary    Python Cryptography package vulnerable to Bleichenbacher timing oracle attack
#                                           Reference  https://nvd.nist.gov/vuln/detail/CVE-2023-50782
#                                           Severity   CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:H/VI:N/VA:N/SC:N/SI:N/SA:N
#                      GHSA-6vqw-3v5j-54x4  URL        https://osv.dev/vulnerability/GHSA-6vqw-3v5j-54x4
#                                           Summary    cryptography NULL pointer dereference with pkcs12.serialize_key_and_certificates when ca...
#                                           Reference  https://nvd.nist.gov/vuln/detail/CVE-2024-26130
#                                           Severity   CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H
#                      GHSA-9v9h-cgj8-h64p  URL        https://osv.dev/vulnerability/GHSA-9v9h-cgj8-h64p
#                                           Summary    Null pointer dereference in PKCS12 parsing
#                                           Reference  https://nvd.nist.gov/vuln/detail/CVE-2024-0727
#                                           Severity   CVSS:3.1/AV:L/AC:L/PR:N/UI:R/S:U/C:N/I:N/A:H
#                      GHSA-h4gh-qq45-vh27  URL        https://osv.dev/vulnerability/GHSA-h4gh-qq45-vh27
#                                           Summary    pyca/cryptography has a vulnerable OpenSSL included in cryptography wheels
#                                           Reference  https://github.com/pyca/cryptography/security/advisories/GHSA-h4gh-qq45-vh27
#                      GHSA-jfhm-5ghh-2f97  URL        https://osv.dev/vulnerability/GHSA-jfhm-5ghh-2f97
#                                           Summary    cryptography vulnerable to NULL-dereference when loading PKCS7 certificates
#                                           Reference  https://nvd.nist.gov/vuln/detail/CVE-2023-49083
#                                           Severity   CVSS:3.1/AV:N/AC:H/PR:N/UI:N/S:U/C:N/I:N/A:H
#                      GHSA-v8gr-m533-ghj9  URL        https://osv.dev/vulnerability/GHSA-v8gr-m533-ghj9
#                                           Summary    Vulnerable OpenSSL included in cryptography wheels
#                                           Reference  https://github.com/pyca/cryptography/security/advisories/GHSA-v8gr-m533-ghj9
#                      PYSEC-2023-254       URL        https://osv.dev/vulnerability/PYSEC-2023-254
#                                           Reference  https://github.com/pyca/cryptography/security/advisories/GHSA-jfhm-5ghh-2f97
#                                           Severity   CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H
# idna-3.5             GHSA-jjg7-2v4v-x38h  URL        https://osv.dev/vulnerability/GHSA-jjg7-2v4v-x38h
#                                           Summary    Internationalized Domain Names in Applications (IDNA) vulnerable to denial of service fr...
#                                           Reference  https://nvd.nist.gov/vuln/detail/CVE-2024-3651
#                                           Severity   CVSS:4.0/AV:L/AC:L/AT:N/PR:N/UI:N/VC:N/VI:N/VA:H/SC:N/SI:N/SA:N
#                      PYSEC-2024-60        URL        https://osv.dev/vulnerability/PYSEC-2024-60
#                                           Reference  https://huntr.com/bounties/93d78d07-d791-4b39-a845-cbfabc44aadb
#                                           Severity   CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H
# jinja2-3.1.2         GHSA-h5c8-rqwp-cp95  URL        https://osv.dev/vulnerability/GHSA-h5c8-rqwp-cp95
#                                           Summary    Jinja vulnerable to HTML attribute injection when passing user input as keys to xmlattr fil
#                                           Reference  https://nvd.nist.gov/vuln/detail/CVE-2024-22195
#                                           Severity   CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:U/C:L/I:L/A:N
#                      GHSA-h75v-3vvj-5mfj  URL        https://osv.dev/vulnerability/GHSA-h75v-3vvj-5mfj
#                                           Summary    Jinja vulnerable to HTML attribute injection when passing user input as keys to xmlattr fil
#                                           Reference  https://nvd.nist.gov/vuln/detail/CVE-2024-34064
#                                           Severity   CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:U/C:L/I:L/A:N
# pyarrow-13.0.0       GHSA-5wvp-7f3h-6wmm  URL        https://osv.dev/vulnerability/GHSA-5wvp-7f3h-6wmm
#                                           Summary    PyArrow: Arbitrary code execution when loading a malicious data file
#                                           Reference  https://nvd.nist.gov/vuln/detail/CVE-2023-47248
#                                           Severity   CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:H/VI:H/VA:H/SC:N/SI:N/SA:N
#                      PYSEC-2023-238       URL        https://osv.dev/vulnerability/PYSEC-2023-238
#                                           Reference  https://github.com/advisories/GHSA-5wvp-7f3h-6wmm
# requests-2.31.0      GHSA-9wx4-h78v-vm56  URL        https://osv.dev/vulnerability/GHSA-9wx4-h78v-vm56
#                                           Summary    Requests `Session` object does not verify requests after making first request with verif...
#                                           Reference  https://nvd.nist.gov/vuln/detail/CVE-2024-35195
#                                           Severity   CVSS:3.1/AV:L/AC:H/PR:H/UI:R/S:U/C:H/I:H/A:N
# setuptools-68.2.0    GHSA-cx63-2mw6-8hw5  URL        https://osv.dev/vulnerability/GHSA-cx63-2mw6-8hw5
#                                           Summary    setuptools vulnerable to Command Injection via package URL
#                                           Reference  https://nvd.nist.gov/vuln/detail/CVE-2024-6345
#                                           Severity   CVSS:4.0/AV:N/AC:L/AT:P/PR:N/UI:A/VC:H/VI:H/VA:H/SC:N/SI:N/SA:N
# tqdm-4.66.0          GHSA-g7vv-2v7x-gj9p  URL        https://osv.dev/vulnerability/GHSA-g7vv-2v7x-gj9p
#                                           Summary    tqdm CLI arguments injection attack
#                                           Reference  https://nvd.nist.gov/vuln/detail/CVE-2024-34062
#                                           Severity   CVSS:3.1/AV:L/AC:L/PR:L/UI:R/S:U/C:L/I:L/A:N
# werkzeug-3.0.0       GHSA-2g68-c3qc-8985  URL        https://osv.dev/vulnerability/GHSA-2g68-c3qc-8985
#                                           Summary    Werkzeug debugger vulnerable to remote execution when interacting with attacker controll...
#                                           Reference  https://nvd.nist.gov/vuln/detail/CVE-2024-34069
#                                           Severity   CVSS:3.1/AV:N/AC:H/PR:N/UI:R/S:U/C:H/I:H/A:H
#                      GHSA-f9vj-2wh5-fj8j  URL        https://osv.dev/vulnerability/GHSA-f9vj-2wh5-fj8j
#                                           Summary    Werkzeug safe_join not safe on Windows
#                                           Reference  https://nvd.nist.gov/vuln/detail/CVE-2024-49766
#                                           Severity   CVSS:4.0/AV:N/AC:H/AT:N/PR:N/UI:N/VC:L/VI:N/VA:N/SC:N/SI:N/SA:N
#                      GHSA-hrfv-mqp8-q5rw  URL        https://osv.dev/vulnerability/GHSA-hrfv-mqp8-q5rw
#                                           Summary    Werkzeug DoS: High resource usage when parsing multipart/form-data containing a large pa...
#                                           Reference  https://nvd.nist.gov/vuln/detail/CVE-2023-46136
#                                           Severity   CVSS:3.1/AV:A/AC:L/PR:L/UI:N/S:U/C:N/I:N/A:H
#                      GHSA-q34m-jh98-gwm2  URL        https://osv.dev/vulnerability/GHSA-q34m-jh98-gwm2
#                                           Summary    Werkzeug possible resource exhaustion when parsing file data in forms
#                                           Reference  https://nvd.nist.gov/vuln/detail/CVE-2024-49767
#                                           Severity   CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:N/VI:N/VA:H/SC:N/SI:N/SA:N/E:U
#                      PYSEC-2023-221       URL        https://osv.dev/vulnerability/PYSEC-2023-221
#                                           Reference  https://github.com/pallets/werkzeug/security/advisories/GHSA-hrfv-mqp8-q5rw
#                                           Severity   CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H
# zipp-3.16.0          GHSA-jfmj-5v4g-7637  URL        https://osv.dev/vulnerability/GHSA-jfmj-5v4g-7637
#                                           Summary    zipp Denial of Service vulnerability
#                                           Reference  https://nvd.nist.gov/vuln/detail/CVE-2024-5569
#                                           Severity   CVSS:4.0/AV:L/AC:L/AT:N/PR:N/UI:N/VC:N/VI:N/VA:H/SC:N/SI:N/SA:N

# real    0m6.817s
# user    0m0.106s
# sys     0m0.100s
