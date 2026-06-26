

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

I started coding in Python 26 years ago while I was in graduate school. Back then I was exploring the usage of algorithmic processes to generate music, and Python was a natural fit.

After a number of years in academia I took a position as a software engineer at Research Affiliates, a finance firm in Newport Beach, California. I went on to build and lead a team of engineers, and was later appointed CTO. It was in that role that I began to focus on cybersecurity: understanding the threats, getting familiar with commercial tools, and exploring the implementation of new tools. The open-source supply chain stood out as a significant risk, inspiring my work on `fetter`.

## Your Work

While I have been in leadership roles for many years, I have remained very active in software engineering. Now, with agentic-coding tools, I have been able to expand my work considerably.

For over eight years I have been developing StaticFrame, an alternative Python DataFrame library built on an immutable data model. This library was critical to developing maintainable systems for portfolio construction, data ingestion, and analytics. Further, as performance is critical for such libraries, I implemented numerous optimized routines as Python C extensions in a library called ArrayKit.

My search for performance led me (like many) to Rust. After some initial experiments I set out to implement a tool to do bottom-up Python package discovery and vulnerability analysis, which became the `fetter` tool I will be demonstrating shortly.

More recently I have been exploring broader cybersecurity and developer tools. In Rust I created a tool called `disclude` which is specialized for discovering code obfuscation with AST heuristics and LLM review. Using typescript, I built a tool called VulnSig that encodes CVSS scores as distinct visual glyphs. And in Python I built an AI agent called Spinwright that, via a GitHub action, performs iterative performance analysis and optimization.

## Tool demonstration

Our focus for today is `fetter`, a command-line application written in Rust. Fetter is designed for bottom-up Python package discovery. Rather than relying just on the packages named in requirements or lock files, fetter discovers every Python executable and, from that, every site-packages directory and importable Python package. Once we have discovered all packages, we can do vulnerability discovery, allow-list enforcement, and a number of other things.

Now there are numerous other tools that attempt to solve related problems by putting barriers between users and package repositories. My view, however, is that every one of those barriers can be circumvented, sometimes trivially, and as such, there is still a need for a tool that discovers packages from the ground up.

### System Searching

Lets start with a basic system-wide search. The `fetter count` command, by default, will search your entire system to find all unique Pythons and `site-packages` directories. Using multi-threaded Rust, this is quite fast, but will take a number of seconds depending on your environment.

```bash
$ pip3 install fetter
$ fetter count
             Count
Executables  46
Sites        46
Packages     595
```

What is great about this approach is that you probably do not really know what is on your system! You might have old Python versions, abandoned virtual environments, or system-installed packages that you you did not even know were there.

`fetter` commands permit using an `-e` (short for `--exe`) parameter to optionally specify the Python executables used to discover importable packages. For example, we can provide the current active `python` executable as a value for `-e`:

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

Through this mechanism, each call to `fetter` establishes a context of one or more Python environments.


### Listing and Searching all Packages

The `fetter` CLI has many subcommands beyond `count`. Another is `scan`: given the context of one or all Python environments, list all packages and their environment directory.

```bash
$ fetter scan
```

Rather than return all results, we can search for packages matching a pattern. For example, to find all versions of NumPy on my system I can do the following:

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

### Deriving a Bound Requirements File

With a full inventory of all packages across many environments, we can derive what I call a "bound" requirements file. By specifying a "lower" anchor we will find the lowest version for each package and set the requirement to be greater than or equal to that version.

```bash
$ fetter -e python3 derive --anchor lower
# via fetter
fetter>=3.4.0
pip>=25.2
```

We will see later how we can use these files as a system-wide allow list.


### Discovering Local Vulnerabilities

Given a set of packages we can check if any have vulnerabilities. This is done efficiently using multi-threaded queries to the Open Source Vulnerability database. The `audit` command, for example, lists all vulnerabilities in all packages on your system:

```bash
$ fetter audit
```

You will probably see a lot results. Notice that, for each vulnerability, we get a number of reference URLs, the CVSS score, and the CVSS vector.

Now if we just want to see the vulnerabilities with the highest CVSS scores, we can use the `--cvss` flag without arguments. With an argument, we can set a minimum CVSS score.


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


### Enumerating & Purging Packages

Having identified packages with vulnerabilities, we can examine the package and purge its contents. We can examine all the files associated with a package with the `unpack-count` command:

```bash
$ fetter unpack-count --pattern torch-2.3.1
Package      Site                                         Files  Dirs
torch-2.3.1  ~/.env311-sage/lib/python3.11/site-packages  12652  2
```

And, with a similar interface, we can entirely remove these packages from all environments with the `purge-pattern` command:

```bash
$ fetter purge-pattern --pattern torch-2.3.1
```

### Discovering Package Vulnerabilities

While `fetter` provides a unique resource for discovering vulnerabilities in local packages, we can also use it check for vulnerabilities on any named package. For example, to see all vulnerabilities associated with the "cryptography" package we can use the following command:

```bash
$ fetter lookup-name cryptography
```

As before, we can set a minimum CVSS score to filter results.

```bash
$ fetter lookup-name cryptography  --cvss="9.8"
```

If we want to scan packages defined in a requirements or lock file, either found locally or on another system, the `lookup-bound` command can be used. For example, to scan the requirements defined for an open-source repository, we can simply provide the `.git` url:

```bash
fetter lookup-bound https://github.com/ModelTC/LightLLM.git
```

### Systen-Wide Package Allow Listing

Given a lock or requirements file such as derived above, we can use that file to measure conformity to the current environment. For example, to write out a bound requirements file for all packages on my system I can do the following:

```bash
$ fetter derive --anchor lower write -o /tmp/bound.txt
```

To validate my environment against the current state of packages, I can use the `fetter validate` command and provide this file. Of course, this will fully validate unless we change the installed packages or the bound requirements. We can remove a few lines and then validate to see that we now have a few "Unrequired" parameters.

```bash
$ sed -i '' '19,20d' /tmp/bound.txt
$ fetter validate --bound /tmp/bound.txt
```

Convenient options such as `--subset` and `--superset` permit the observed packages to be either a subset or superset (respectively) of the bound requirements. If we want to permit unrequired packages, we can use the `--superset` flag:

```bash
$ fetter validate --superset --bound /tmp/bound.txt
```

### Conclusion

I hope to have shown you that `fetter` offers a powerful resource for local, bottom-up package and vulnerability discovery. These tools can be used on local or arbitrary packages and project requirements. With a bound requirements file, system-wide, cross-project allow-list monitoring is also possible.
