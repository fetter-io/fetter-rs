//! Windows-only end-to-end "interactive session" test.
//!
//! There is no local Windows machine in the loop, so this exercises the demo
//! command set (`doc/articles/bh-arsenal-2026/demo.md`) the way a user would in
//! a live session: build a controlled virtual environment, run commands, mutate
//! the environment, and run more commands, confirming output at each step
//! (JSON for `validate`, delimited text for the reporting commands).
//!
//! The environment is fabricated offline: the venv is created with
//! `--without-pip` and packages are simulated by writing `.dist-info`
//! directories directly into `site-packages`. No network / PyPI access is
//! required, so results are fully deterministic.
#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::tempdir;

/// Absolute path to the freshly built `fetter` binary (provided by Cargo).
const FETTER: &str = env!("CARGO_BIN_EXE_fetter");

/// Run a command, returning (stdout, stderr, success).
fn run(cmd: &mut Command) -> (String, String, bool) {
    let out = cmd.output().expect("failed to spawn command");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

/// Locate a base Python interpreter to bootstrap the test venv.
fn base_python() -> String {
    for name in ["python", "python3", "py"] {
        let ok = Command::new(name)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            return name.to_string();
        }
    }
    panic!("no base python interpreter found on PATH");
}

/// Invoke `fetter -e <py>` with caching disabled (so environment mutations are
/// always observed) and an isolated cache directory. Asserts success; returns
/// stdout.
fn fetter(py: &Path, cache: &Path, args: &[&str]) -> String {
    let mut c = Command::new(FETTER);
    c.arg("-e")
        .arg(py)
        .arg("--cache-duration")
        .arg("0")
        .arg("--cache-directory")
        .arg(cache)
        .args(args);
    let (stdout, stderr, ok) = run(&mut c);
    assert!(ok, "`fetter {args:?}` failed; stderr:\n{stderr}");
    stdout
}

/// Create a `<name>-<version>.dist-info` package in `site`. Each `record` entry
/// is a (relative path, size) pair written to the RECORD file and materialized
/// on disk (RECORD drives `unpack-count`/`unpack-files`).
fn add_package(site: &Path, name: &str, version: &str, record: &[(&str, &str)]) {
    let di = site.join(format!("{name}-{version}.dist-info"));
    fs::create_dir_all(&di).unwrap();
    fs::write(
        di.join("METADATA"),
        format!("Metadata-Version: 2.1\nName: {name}\nVersion: {version}\n"),
    )
    .unwrap();
    let mut rec = String::new();
    for (rel, size) in record {
        rec.push_str(&format!("{rel},,{size}\n"));
        let fp = site.join(rel);
        if let Some(parent) = fp.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        // Materialize the listed file, but never clobber a file already written
        // for this package (e.g. the real METADATA when it appears in RECORD).
        if !fp.exists() {
            fs::write(&fp, "x").unwrap();
        }
    }
    fs::write(di.join("RECORD"), rec).unwrap();
}

fn remove_package(site: &Path, name: &str, version: &str) {
    fs::remove_dir_all(site.join(format!("{name}-{version}.dist-info"))).unwrap();
}

fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap()
}

/// Count top-level entries of a validation-digest JSON array by counting the
/// per-record "explain" keys (0 for the empty array "[]").
fn digest_len(json: &str) -> usize {
    json.matches("\"explain\"").count()
}

#[test]
fn windows_interactive_session() {
    // Scratch space that is removed when `dir` is dropped.
    let dir = tempdir().unwrap();
    let tmp = dir.path();
    let venv = tmp.join("venv");
    let cache = tmp.join("cache");
    fs::create_dir_all(&cache).unwrap();
    let bound = tmp.join("bound.txt");
    let bound_s = bound.to_str().unwrap();

    // Small helper to run a reporting command that writes a delimited file and
    // return its contents.
    let write_file = tmp.join("out.csv");
    let write_s = write_file.to_str().unwrap();

    // --- bootstrap a controlled, offline virtual environment ---
    let base = base_python();
    let (_o, e, ok) = run(Command::new(&base)
        .arg("-m")
        .arg("venv")
        .arg("--without-pip")
        .arg(&venv));
    assert!(ok, "venv creation failed; stderr:\n{e}");

    let py = venv.join("Scripts").join("python.exe");
    assert!(
        py.is_file(),
        "expected Windows venv interpreter at {}",
        py.display()
    );

    // Discover site-packages the same way fetter does.
    let (site_out, _e, ok) = run(Command::new(&py)
        .arg("-c")
        .arg("import site;print(site.getsitepackages()[0])"));
    assert!(ok, "could not resolve site-packages");
    let site = PathBuf::from(site_out.trim());
    assert!(site.is_dir(), "site-packages not a dir: {}", site.display());

    // Two controlled packages; foo has files so unpack-count can enumerate them.
    add_package(
        &site,
        "foo",
        "1.0.0",
        &[
            ("foo/__init__.py", "1"),
            ("foo-1.0.0.dist-info/METADATA", "40"),
        ],
    );
    add_package(&site, "bar", "2.3.0", &[]);

    // ---------------------------------------------------------------------
    // Session 1: inspect the fresh environment.
    // ---------------------------------------------------------------------
    // count -> one exe, one site, two packages.
    fetter(&py, &cache, &["count", "write", "-o", write_s, "-d", ","]);
    let count = read(&write_file);
    assert!(count.contains("Executables,1"), "count:\n{count}");
    assert!(count.contains("Sites,1"), "count:\n{count}");
    assert!(count.contains("Packages,2"), "count:\n{count}");

    // scan -> both packages listed (sorted).
    fetter(&py, &cache, &["scan", "write", "-o", write_s, "-d", ","]);
    let scan = read(&write_file);
    assert!(scan.contains("bar-2.3.0"), "scan:\n{scan}");
    assert!(scan.contains("foo-1.0.0"), "scan:\n{scan}");
    let scan_rows = scan.lines().skip(1).filter(|l| !l.is_empty()).count();
    assert_eq!(scan_rows, 2, "expected 2 scan rows:\n{scan}");

    // search -> pattern matches foo only.
    fetter(
        &py,
        &cache,
        &["search", "-p", "foo*", "write", "-o", write_s, "-d", ","],
    );
    let search = read(&write_file);
    assert!(search.contains("foo-1.0.0"), "search:\n{search}");
    assert!(!search.contains("bar-2.3.0"), "search:\n{search}");

    // derive a lower-bound requirements file for use as an allow-list.
    fetter(
        &py,
        &cache,
        &["derive", "--anchor", "lower", "write", "-o", bound_s],
    );
    let bound_txt = read(&bound);
    assert!(bound_txt.contains("foo>=1.0.0"), "bound:\n{bound_txt}");
    assert!(bound_txt.contains("bar>=2.3.0"), "bound:\n{bound_txt}");

    // validate against the derived file -> environment is conformant.
    let v = fetter(&py, &cache, &["validate", "--bound", bound_s, "json"]);
    assert_eq!(v.trim(), "[]", "expected valid environment, got:\n{v}");

    // unpack-count -> foo's files/dirs are enumerated from RECORD.
    fetter(
        &py,
        &cache,
        &[
            "unpack-count",
            "--pattern",
            "foo-1.0.0",
            "write",
            "-o",
            write_s,
            "-d",
            ",",
        ],
    );
    let unpack = read(&write_file);
    let foo_row = unpack
        .lines()
        .find(|l| l.starts_with("foo-1.0.0,"))
        .unwrap_or_else(|| panic!("no foo row in unpack:\n{unpack}"));
    // Trailing fields are Files,Dirs (the Site column contains no comma).
    let fields: Vec<&str> = foo_row.rsplitn(3, ',').collect();
    let dirs: usize = fields[0].trim().parse().unwrap();
    let files: usize = fields[1].trim().parse().unwrap();
    assert!(files >= 2, "expected >=2 files, unpack:\n{unpack}");
    assert!(dirs >= 1, "expected >=1 dir, unpack:\n{unpack}");

    // ---------------------------------------------------------------------
    // Session 2: mutate the environment -> install an unrequired package.
    // ---------------------------------------------------------------------
    add_package(&site, "baz", "9.9.9", &[]);

    let v = fetter(&py, &cache, &["validate", "--bound", bound_s, "json"]);
    assert_eq!(digest_len(&v), 1, "expected one finding:\n{v}");
    assert!(v.contains("\"explain\":\"Unrequired\""), "validate:\n{v}");
    assert!(v.contains("baz-9.9.9"), "validate:\n{v}");

    // --superset permits unrequired packages -> conformant again.
    let v = fetter(
        &py,
        &cache,
        &["validate", "--superset", "--bound", bound_s, "json"],
    );
    assert_eq!(v.trim(), "[]", "superset should pass:\n{v}");

    // ---------------------------------------------------------------------
    // Session 3: mutate again -> uninstall a required package.
    // ---------------------------------------------------------------------
    remove_package(&site, "baz", "9.9.9");
    remove_package(&site, "foo", "1.0.0");

    // count now reflects a single remaining package (bar).
    fetter(&py, &cache, &["count", "write", "-o", write_s, "-d", ","]);
    let count = read(&write_file);
    assert!(
        count.contains("Packages,1"),
        "count after removal:\n{count}"
    );

    let v = fetter(&py, &cache, &["validate", "--bound", bound_s, "json"]);
    assert_eq!(digest_len(&v), 1, "expected one finding:\n{v}");
    assert!(v.contains("\"explain\":\"Missing\""), "validate:\n{v}");
    assert!(v.contains("foo>=1.0.0"), "validate:\n{v}");

    // --subset permits missing packages -> conformant again.
    let v = fetter(
        &py,
        &cache,
        &["validate", "--subset", "--bound", bound_s, "json"],
    );
    assert_eq!(v.trim(), "[]", "subset should pass:\n{v}");
}
