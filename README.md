# fetter

<a href="https://github.com/flexatone/vigilnaut/actions/workflows/ci.yml">
    <img style="display: inline!important" src="https://img.shields.io/github/actions/workflow/status/flexatone/vigilnaut/ci.yml?branch=default&label=CI&logo=Github"></img>
</a>

System-wide Python package discovery and allow listing.



## What is New in Fetter

### 0.9.0

Support `--requirement` in requirements files.


### 0.8.0

Package and DepSpec comparisons now remove user.

Package and DepSpec comparisons now accept matching either on requested_revision or commit_id.

URLs are now shown in DepSpec displays.

Delimited file output no longer pads spaces.


### 0.7.0

Validate display now shows paths properly.

Updated validate json output to terminate line and flush buffer.


### 0.6.0

Package and dependency keys are case insensitive.

Improved URL validation between dependency and package by removing user components.

Improved validation JSON output to provided labelled objects.

Improved valiation output to show sorted missing packages.

Renamed validation explain values.

Implemented support for nested requirements.txt.


### 0.5.0

Implemented search command with basic wildcard matching.

Implemented `Arc`-wrapped `PathBuf` for sharable site paths.

Added explanation column to validation results.

Added support for both `--subset` and `--superset` validations.

Implemented `ValidationDigest` for simplified JSON serialization.

Added `JSON` CLI output option for validation results.






