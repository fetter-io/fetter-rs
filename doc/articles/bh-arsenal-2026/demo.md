

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

Greetings. My name is Chris Ariza and I will be demonstrating the fetter command-line utility as part of Black Hat Arsenal 2026.

I will start with a little bit about me. I started coding in Python back in 2000 while I was in graduate school. Back then I was exploring the usage of algorithmic processes to generate musical structures, and Python was a natural fit.

After a number of years in academia I took a position as a software engineer at Research Affiliates, a finance firm located in Newport Beach, California. I went on to build and lead a team of engineers, and was later appointed CTO. It was in that role that I really began to focus on cybersecurity: understanding the threats, getting familiar with commercial tooling that was available, and exploring the implementation of my own tools.

## Your Work

While I have been a manager of engineers for many years, I have remained very active in software engineering. Now, with agentic coding tools, I have been able to expand my work considerably.

For over eight years I have been developing StaticFrame, an alternative Python DataFrame library built on an immutable data model. This library was critical to developing maintainable systems for portfolio construction, data ingestion, and analytics reporting.

Because performance is critical for such libraries, that work led me to implement Python C extensions in a library called ArrayKit.

Inevitably, if you are looking for performance, you are likely to turn to Rust. After some initial experiments I set out to implement a tool to do bottom-up Python package discovery and vulnerability analysis, which became the `fetter` tool I will be demonstrating shortly.

More recently I have been using Rust to build a tool, called `disclude`, specialized for discovering code obfuscation. Using typescript, I have built a tool for visualy encoding CVSS scores called VulnSig. And my most recent project in Python has been an AI agent focused on Python performance optimization deployed as a GitHub action.

## Tool demonstration

Our focus for today is `fetter`, a command-line application written in Rust. Fetter is designed for bottom-up Python package discovery. Rather than relying on the Python packages as defined in requirements or lock files, fetter discovers every Python executable and, from that, every site-packages directory and every importable Python package. Once we have discovered all packages, we can do vulnerability discovery, allow-list enforcement, and number of other things.

### System Searching

Lets start with a basic system-wide search. The `fetter count` command, by default, will search your entire system to find all unique Pythons and `site-packages`. Using multi-threaded Rust, this is quite fast.

```bash
$ pip3 install fetter
$ fetter count
             Count
Executables  46
Sites        46
Packages     595
```

What is great about this approach is that you probably do not really know what is on your system! You might have old Python versions, abandoned virtual environments, or system-installed packages that you have did not know were there.

Many `fetter` commands permit using the `-e` parameter to optionally specify the Python executables used to discover importable packages. For example, to use the current active `python` executable:

```bash
$ fetter -e python3  count
             Count
Executables  1
Sites        1
Packages     2
```

More than one `-e` argument can be provided:

```bash
{.env314-temp}{default} % fetter -e python3 -e ~/.env314-sf/bin/python3 count
             Count
Executables  2
Sites        2
Packages     103
```

Each call to `fetter` establishes a context of one or more Python environments; either full system wide or with specifically targetted environments.

### Listing and Searching all Packages

Given the context of full-system search or a more narrow selection, we can list all packages, as well as all virtual environments within within which those packages reside.

```bash
$ fetter scan
```

Instead of listing all packages, we can search for packages matching a pattern. For example, to find all version of NumPy on my system, I can do the followingL

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


### Discovering Vulnerabilities

Given the context of full-system search or a more narrow selection, we can check if any package has a vulnerability. This is done efficiently using multi-threaded queries to the Open Source Vulnerabiltiy database.
















