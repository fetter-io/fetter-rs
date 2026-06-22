

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

I will start with a little bit about me. I started coding in Python back in 2000 while I was in graduate school. Back then I was exploring was of using algorithmic processes to generate musical structures, and Python was a natural fit.

After a number of years in academia I took a position as a software engineer at Research Affiliates, a finance firm located in Newport Beach, California. I went on to build and lead a team of engineers, and was later appointed CTO. It was in that role that that I began to focus on cybersecurity: understanding the threats, the commercial tooling that was available, and exploring the implementation of my own tools.

## Your Work

While I have been a manager of engineers for many years, I have remained very hands-on in software engineering. Now, with agentic coding tools, I have been able to expand my work considerably.





