

# fetter

## Abstract

Python supply-chain threats, via malicious packages on PyPI or installed from other sources, remain a persistent security risk. While tools exist to limit what packages can be downloaded (JFrog Artifactory, AWS CodeArtifact) and tools exist to scan project requirements for known vulnerabilities (`pip-audit`), neither approach answers a fundamental question: what vulnerable packages are actually on your system?

The recent `spellcheckpy` Python package malware (as reported by Aikido) installs a full-featured remote access trojan: if a developer bypassed package repository controls to install it, and it was not identified in a requirements file, existing tools would miss it entirely.

Fetter solves this by scanning installed packages across all Python virtual environments or an entire system. Unlike `pip-audit` (which only examines requirements and lock files), Fetter finds every installed package (regardless of how it got there) and checks them against the OSV vulnerability database, returning rich vulnerability details and CVSS scores. Fetter can report the highest-risk packages across your entire infrastructure in seconds.


## Demo

What to Include in Your Demo
Recorded Demo length: 10–20 minutes
Your recorded demo should cover:

    Introduction – Briefly introduce yourself
    Your work – Share a little about what you do or what you like to work on
    Tool demonstration – Walk through your tool and its key features


## Introduction

Greetings. My name is Chris Ariza and I will be demonstrating the `fetter` command-line utility as part of this year's Black Hat Arsenal.

A little bit about me first.

I started coding in Python 26 years ago while I was in graduate school. Back then I was exploring the usage of algorithmic processes to generate musical structures, and Python was a natural fit.

After a number of years in academia I took a position as a software engineer at Research Affiliates, a finance firm in Newport Beach, California. I went on to build and lead a team of engineers, and was later appointed CTO. It was in that role that I began to focus on cybersecurity: understanding the threats, getting familiar with commercial tools, and exploring the implementation of new tools. Protection against supply chain risks stood out as a weakness.

## Your Work

While I have been a manager for many years, I have remained very active in software engineering. Now, with argentic coding tools, I have been able to expand my work considerably.

For over eight years I have been developing StaticFrame, an alternative Python DataFrame library built on an immutable data model. This library was critical to developing maintainable systems for portfolio construction, data ingestion, and analytics reporting. Further, as performance is critical for such libraries, I implemented numerous optimized routines as Python C extensions in a library called ArrayKit.

My search for better performance led me to Rust. After some initial experiments I set out to implement a tool to do bottom-up Python package discovery and vulnerability analysis, which became the `fetter` tool I will be demonstrating shortly.

More recently I have exploring broader cybersecurity and developer tools. I created a tool called `disclude` which is specialized for discovering code obfuscation with AST heuristics and LLM review. Using typescript, I built a tool called VulnSig that encodes CVSS scores as distinct visual glyphs. And in Python I have built an AI agent called Spinwright that, via a GitHub action, performs performnace analysis and optimization.

## Tool demonstration

Our focus for today is `fetter`, a command-line application written in Rust. Fetter is designed for bottom-up Python package discovery. Rather than relying on the Python packages as defined in requirements or lock files, fetter discovers every Python executable and, from that, every site-packages directory and importable Python package. Once we have discovered all packages, we can do vulnerability discovery, allow-list enforcement, and number of other things.

### System Searching

Lets start with a basic system-wide search. The `fetter count` command, by default, will search your entire system to find all unique Pythons and `site-packages`. Using multi-threaded Rust, this is quite fast, but will take a number of seconds depending on your environment.

```bash
$ pip3 install fetter
$ fetter count
             Count
Executables  46
Sites        46
Packages     595
```

What is great about this approach is that you probably do not really know what is on your system! You might have old Python versions, abandoned virtual environments, or system-installed packages that you have did not know were there.

`fetter` commands permit using the `-e` parameter to optionally specify the Python executables used to discover importable packages. For example, we can provide the current active `python` executable:

```bash
$ fetter -e python3 count
             Count
Executables  1
Sites        1
Packages     2
```

More than one `-e` argument can be provided:

```bash
$ fetter -e python3 -e ~/.env314-sf/bin/python3 count
             Count
Executables  2
Sites        2
Packages     103
```

Through this mechanism each call to `fetter` establishes a context of one or more Python environments.


### Listing and Searching all Packages

The `fetter` CLI has many subcommands beyond `count`. Another is `scan`: given the context of one or all Python environments, list all packages and their environment directory.

```bash
$ fetter scan
```

With the same context, we can search for packages matching a pattern. For example, to find all versions of NumPy on my system I can do the following:

```bash
$ fetter search -p numpy*
Package       Site
numpy-1.26.4  ~/.env312-dft/lib/python3.12/site-packages
              ~/.env311-sage/lib/python3.11/site-packages
numpy-2.0.0   ~/.env311-ff/lib/python3.11/site-packages
              ~/.env311-am/lib/python3.11/site-packages
numpy-2.1.3   ~/.env313-am/lib/python3.13/site-packages
numpy-2.2.2   ~/_x/src/test-uv/.venv/lib/python3.13/site-packages
numpy-2.2.3   ~/.env313-test/lib/python3.13/site-packages
              ~/.env313-eg/lib/python3.13/site-packages
numpy-2.2.5   ~/.env313-ak/lib/python3.13/site-packages
numpy-2.3.1   ~/.env313-sf/lib/python3.13/site-packages
              ~/.env313-aredox/lib/python3.13/site-packages
numpy-2.3.4   ~/.env314t-ft/lib/python3.14t/site-packages
              ~/.env314-ft/lib/python3.14/site-packages
numpy-2.3.5   ~/.env314-condfut/lib/python3.14/site-packages
              ~/.env314t-condfut/lib/python3.14t/site-packages
numpy-2.4.3   ~/.env313-tidelathe/lib/python3.13/site-packages
              ~/.env314-tidelathe/lib/python3.14/site-packages
numpy-2.4.4   ~/_x/src/spinwright-sf/.venv/lib/python3.14/site-packages
              ~/.env314-sf/lib/python3.14/site-packages
```

### Deriving a Requirements File

With a full listing of packages across many environments, we can derive an aggregate requirements file. By specifying a "lower" anchor we find the lowest version for each package and set the requirement to be greater than or equal to that version.

```bash
$ fetter -e python3 derive --anchor lower
# via fetter
fetter>=3.4.0
pip>=25.2
```

We will see later how we can use these files as an allow list to validate all packages.


### Discovering Local Vulnerabilities

Given the context of a set of packages, we can check if any package has a vulnerability. This is done efficiently using multi-threaded queries to the Open Source Vulnerability database. The following command, for example, lists all vulnerabilities in all packages on your system:

```bash
$ fetter audit
```

You will probably see a lot results. If we just want to see the vulnerability with the highest CVSS score, we can use the following command:

```bash
$ fetter audit --cvss
Package              Vulnerabilities  Attribute  Value
cryptography-45.0.4  PYSEC-2026-36    URL        https://osv.dev/vulnerability/PYSEC-2026-36
                                      Reference  http://www.openwall.com/lists/oss-security/2026/04/08/12
                                      Severity   CVSS 9.8 (Critical): CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H
cryptography-46.0.4  PYSEC-2026-36    URL        https://osv.dev/vulnerability/PYSEC-2026-36
                                      Reference  http://www.openwall.com/lists/oss-security/2026/04/08/12
                                      Severity   CVSS 9.8 (Critical): CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H
torch-2.3.1          PYSEC-2024-259   URL        https://osv.dev/vulnerability/PYSEC-2024-259
                                      Reference  https://rumbling-slice-eb0.notion.site/Distributed-RPC-Framework-RemoteMo…
                                      Severity   CVSS 9.8 (Critical): CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H
                     PYSEC-2025-41    URL        https://osv.dev/vulnerability/PYSEC-2025-41
                                      Reference  https://github.com/pytorch/pytorch/security/advisories/GHSA-53q9-r3pm-6pq6
                                      Severity   CVSS 9.8 (Critical): CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H
```

As with other commands, we can target this search to one or more specific environments.

### Enumerating & Purging Packages

Having identified packages with vulnerabilities, we can examine the package and purge its contents. We can enumerate all the files associated with a package with the `unpack-count` command:

```bash
$ fetter unpack-count --pattern torch-2.3.1
Package      Site                                         Files  Dirs
torch-2.3.1  ~/.env311-sage/lib/python3.11/site-packages  12652  2
```

And, with a similar interface, we can remove these packages with the `purge-pattern` command:

```bash
$ fetter purge-pattern --pattern torch-2.3.1
# via fetter
fetter>=3.4.0
pip>=25.2
```

### Discovering Package Vulnerabilities

While `fetter` provides a unique resource for discovering local packages, we can also check any package by name for vulnerabilities. For example, to see all vulnerabilities associated with the "cryptography" package we can use the following command:

```bash
$ fetter lookup-name cryptography
```

As before, we can set a minimum CVSS score to filter results.

```bash
$ fetter lookup-name cryptography  --cvss="9.8"
```

If we want to scan a the package defined in a requirements or lock file, either found locally or on another system, the `lookup-bound` command can be used. For example, to scan the requirements defined for an open-source repository, we can provide the .git url:

```bash
fetter lookup-bound https://github.com/ModelTC/LightLLM.git
```

### Systen-Wide Package Allow Listing

Given a lock or requirements file, such as derived above, we can use that file to measure if the current environment conforms. For example, to derive a requirements file for all packages on my system I can do the following:

```bash
$ fetter derive --anchor lower write -o /tmp/bound.txt
```

To validate my environment against the current state of packages, I can use the `fetter validate` command. Convenient options such as `--subset` and `--superset` permit the observed package to be subset or superset (respectively) of the bound requirements.

```bash
$ fetter validate --bound /tmp/bound.txt
```

### Conclusion

There are a few other things we can do with `fetter` not shown here: we can set up automatic validation of package installation conformity to an allow list that is run on every command, or we can use `fetter` as a persistent agent that sends scan results at some periodicity.