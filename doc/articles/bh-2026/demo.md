

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

My name is Chris Ariza and I will be demonstrating the `fetter` command-line utility as part of this year's Black Hat Arsenal.

<!-- slide -->

Fetter is a command-line application for "bottom-up" Python vulnerability discovery and allow-list enforcement on Linux and MacOS.

You can learn more about fetter at the GitHub page shown here.

If you want to try it out immediately, you can use pip, pipx or uvx.

<!-- << slide >> -->

A little bit about me first.

<!-- > -->

I am presently CTO at Research Affiliates, a finance firm located in Newport Beach, California.

<!-- > -->

Having been a software engineer for over 20 years, in this role I began to explore the implementation of new cybersecurity tools.

<!-- > -->

My initial focus was on the growing threats to the open-source supply chain. It is for this reason I created fetter.

<!-- << slide >> -->

Beyond fetter I develop a number of other open-source tools

<!-- > -->

For over eight years I have been developing StaticFrame, an alternative Python DataFrame library built on an immutable data model. As performance is critical for such a tool, I implemented numerous optimized routines as Python C extensions in a library called ArrayKit.

<!-- > -->

For over two years I have been working on the Rust-based `fetter` command-line tool I will be demonstrating shortly. More recently I have been working on a Rust / TypeScript web app called Fetter IO that permits aggregating and displaying fetter scan data.

<!-- > -->

This year I created VulnSig.io, a web app and toolkit for representing CVSS vectors as expressive visual glyphs. The VulnSig.io site provides a regularly updated feed of recent CVEs using VulnSig glyphs

<!-- > -->

And most recently I have been working on `disclude`, Rust-based tool specialized for discovering code obfuscation with AST heuristics and LLM review.

<!-- << slide: fetter >> -->

Our focus for today is `fetter`

<!-- > -->

As I already mentioned, `fetter` is a command-line application written in Rust.

<!-- > -->

Even though the core is Rust, I provide a Python package for easy installation.

<!-- > -->

A key feature of `fetter` is that it is designed for bottom-up Python package discovery. Rather than relying on the packages defined in requirements or lock files, fetter discovers every Python executable and, from that, every `site-packages` directory. With each `site-packages` directory, we can find all importable Python packages.

<!-- > -->

Once we have discovered all packages, we can search for specific packages, perform vulnerability discovery, and enforce allow-lists.

<!-- << slide >> -->

Lets move on to the demonstration.

While you can certainly build a Rust binary from source, pre-built Python packages are easy to install. You can install in your current environment with `pip`, use `pipx` to create user-level installation, or use `uvx` for an ephemeral installation.

<!-- << slide >> -->

Fetter functionality is organized under numerous subcommands.

To give you an overview, here are some of the key `fetter` subcommands we will be using today:

`count` for package metrics
`scan` for a full package listing
`search` to find packages based on a pattern
`audit` to check all packages for vulnerabilities
`lookup-name` to find vulnerabilities for any packages
`lookup-bound` to find vulnerabilities in any project or git repo
`validate` to perform allow listing


<!-- << demo >> -->


## Tool demonstration

Lets start with a basic system-wide search. The `fetter count` command, by default, will search your entire system to find all unique Pythons and `site-packages` directories. Using multi-threaded Rust, this is quite fast, but will take a number of seconds depending on your environment.

### System Searching

Lets start with a basic system-wide search. The `fetter count` command, by default, will search your entire system to find all unique Pythons and `site-packages` directories. Using multi-threaded Rust, this is quite fast, but will take a number of seconds depending on your environment.

```bash
$ fetter count
```

What is great about this approach is that you probably do not really know what is on your system! You might have old Python versions, abandoned virtual environments, or system-installed packages that you you did not even know were there.

`fetter` commands permit using an `-e` (short for `--exe`) parameter to optionally specify the Python executables used to discover importable packages. For example, we can provide the current active `python` executable as a value for `-e`:

```bash
$ fetter -e python3 count
```

More than one `-e` argument can be provided:

```bash
$ fetter -e python3 -e ~/.env314-sf/bin/python3 count
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
```

### Deriving a Bound Requirements File

With a full inventory of all packages across many environments, we can derive what I call a "bound" requirements file. For each package, we can find the lowest version and set the requirement to be greater than or equal to that version.

```bash
$ fetter -e python3 derive --anchor lower
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
```

We can use that same parameter and provide a value to only show vulnerabilities with CVSS scores greater than or equal to that value.

```bash
$ fetter audit --cvss=9.8
```

### Enumerating & Purging Packages

Having identified packages with vulnerabilities, we can examine the package and purge its contents. We can examine all the files associated with a package with the `unpack-count` command:

```bash
$ fetter unpack-count --pattern nltk-3.9.1

```

And, with a similar interface, we can entirely remove these packages from all environments with the `purge-pattern` command:

```bash
$ fetter purge-pattern --pattern nltk-3.9.1
```



### Discovering Vulnerabilities for Any Package

While `fetter` provides a unique resource for discovering vulnerabilities in local packages, we can also use it check for vulnerabilities on any named package across all versions. For example, to see all vulnerabilities associated with the Python "cryptography" package we can use the following command:

```bash
$ fetter lookup-name cryptography
```

As before, we can set a minimum CVSS score to filter results.

```bash
$ fetter lookup-name cryptography  --cvss=9.8
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

Convenient options such as `--subset` and `--superset` which permit the observed packages to be either a subset or superset (respectively) of the bound requirements. If we want to permit unrequired packages, we can use the `--superset` flag:

```bash
$ fetter validate --superset --bound /tmp/bound.txt
```


<!-- << slide >> -->

I hope to have shown you that `fetter` offers a powerful resource for local, bottom-up package and vulnerability discovery.

<!-- > -->

While there are a number of tools that can also scan requirements or lock files for vulnerabilities, `fetter` is different in that it finds what is actually on your system.

<!-- > -->

Many approach the open-source supply chain problem by putting barriers between users and package repositories. Such controls are useful, but most can be circumvented. This is why we need a tool like `fetter`.

<!-- > -->

Finally, `fetter` offers the foundational tools to implement system-wide, or even enterprise-wide, Python package allow listing.


<!-- << slide >> -->

Thank you for your attention, and I and hope you enjoy the rest of Black Hat Arsenal.