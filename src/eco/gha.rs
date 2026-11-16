//! GitHub Actions scanner.
//!
//! Origins are repository roots: a directory containing `.github/workflows` and `.git` (a directory, or a file for worktrees and submodules). GitHub only runs workflows from a repository's root, so copies elsewhere (vendored subprojects, build output, editor extensions) are not origins. Sites are each workflow file in `.github/workflows` and each local composite `action.yml` under `.github` (`SiteKind::Manifest`). Packages are the actions referenced by `uses:`.
//!
//! A package's name is `owner/repo` (OSV names actions by repository, not subpath). Its version is the ref as written (`v4`, `v4.1.0`, `main`), except that a SHA pin with a full version comment (`@<sha> # v4.1.1`) takes the comment's version, with the SHA kept in `direct_url`. Refs are interpreted when auditing; see `osv_match::RefKind`.

use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use serde_json::Value as JsonValue;

use crate::eco::Detect;
use crate::eco::DirItem;
use crate::eco::EcosystemScanner;
use crate::ecosystem::Ecosystem;
use crate::origin_site::Origin;
use crate::origin_site::Site;
use crate::origin_site::SiteKind;
use crate::osv_match::is_sha;
use crate::osv_match::RefKind;
use crate::package::Package;
use crate::package_durl::DirectURL;
use crate::package_durl::VcsInfo;
use crate::path_shared::PathShared;
use crate::util::path_normalize;
use crate::util::FlagLog;
use crate::util::ResultDynError;
use crate::util::ScanConfig;
use crate::version_spec::VersionSpec;

const GITHUB: &str = ".github";
const WORKFLOWS: &str = "workflows";
const GIT: &str = ".git";
const ACTION_FILES: [&str; 2] = ["action.yml", "action.yaml"];

fn is_yaml(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "yml" || e == "yaml")
}

fn workflows_dir(root: &Path) -> PathBuf {
    root.join(GITHUB).join(WORKFLOWS)
}

fn origin(dir: PathBuf) -> Origin {
    Origin::new(Ecosystem::Gha, PathShared::from(dir))
}

//------------------------------------------------------------------------------
/// Collect `uses:` values from a parsed workflow or action: `jobs.*.uses` (reusable workflows), `jobs.*.steps[*].uses`, and `runs.steps[*].uses` (composite actions).
fn uses_from_yaml(doc: &JsonValue) -> Vec<String> {
    let mut out = Vec::new();
    let mut from_steps = |steps: Option<&JsonValue>| {
        for step in steps.and_then(JsonValue::as_array).into_iter().flatten() {
            if let Some(u) = step.get("uses").and_then(JsonValue::as_str) {
                out.push(u.to_string());
            }
        }
    };
    if let Some(jobs) = doc.get("jobs").and_then(JsonValue::as_object) {
        for job in jobs.values() {
            from_steps(job.get("steps"));
        }
    }
    from_steps(doc.get("runs").and_then(|r| r.get("steps")));
    if let Some(jobs) = doc.get("jobs").and_then(JsonValue::as_object) {
        for job in jobs.values() {
            if let Some(u) = job.get("uses").and_then(JsonValue::as_str) {
                out.push(u.to_string());
            }
        }
    }
    out
}

/// Parse one line as `uses: <value> [# comment]`, returning the value and comment. YAML parsing drops comments, so comments (version hints on SHA pins) are read from the raw text.
fn parse_uses_line(line: &str) -> Option<(String, Option<String>)> {
    let t = line.trim_start();
    let t = t.strip_prefix("- ").unwrap_or(t).trim_start();
    let rest = t.strip_prefix("uses:")?.trim();
    let (value, comment) = match rest.split_once(" #") {
        Some((v, c)) => (v, Some(c.trim().to_string()).filter(|c| !c.is_empty())),
        None => (rest, None),
    };
    let value = value
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .to_string();
    (!value.is_empty()).then_some((value, comment))
}

/// Convert a `uses:` value (and its comment, if any) to a Package. Local actions (`./`), Docker images, and values with expressions are excluded.
fn uses_to_package(uses: &str, comment: Option<&str>) -> Option<Package> {
    let u = uses.trim();
    if u.starts_with("./")
        || u.starts_with("../")
        || u.starts_with("docker://")
        || u.contains("${{")
    {
        return None;
    }
    let (path, git_ref) = u.rsplit_once('@')?;
    let mut parts = path.split('/');
    let (owner, repo) = (parts.next()?, parts.next()?);
    if owner.is_empty() || repo.is_empty() || git_ref.is_empty() {
        return None;
    }
    let name = format!("{owner}/{repo}");
    if is_sha(git_ref) {
        // a full version comment (e.g. "v4.1.1", from Dependabot or pinact) gives the version; otherwise the SHA itself (unresolvable when auditing)
        let hint = comment
            .and_then(|c| c.trim_start_matches('#').split_whitespace().next())
            .filter(|c| matches!(RefKind::classify(c), RefKind::Exact(_)));
        let durl = DirectURL {
            url: format!("https://github.com/{name}"),
            vcs_info: Some(VcsInfo {
                commit_id: git_ref.to_string(),
                vcs: "git".to_string(),
                revision: hint.map(str::to_string),
            }),
        };
        let version = hint.unwrap_or(git_ref);
        return Some(Package::new(
            Ecosystem::Gha,
            &name,
            VersionSpec::new(version),
            Some(durl),
        ));
    }
    Some(Package::new(
        Ecosystem::Gha,
        &name,
        VersionSpec::new(git_ref),
        None,
    ))
}

/// Read packages from a workflow or action file. `uses:` values come from the YAML structure, so text in `run:` scripts is never mistaken for a step; if the file does not parse as YAML, values are read line by line instead.
fn packages_from_workflow(content: &str) -> Vec<Package> {
    let line_values: Vec<(String, Option<String>)> =
        content.lines().filter_map(parse_uses_line).collect();
    let mut comments: HashMap<&str, &str> = HashMap::new();
    for (value, comment) in &line_values {
        if let Some(c) = comment {
            comments.entry(value.as_str()).or_insert(c.as_str());
        }
    }
    let values: Vec<String> = match serde_saphyr::from_str::<JsonValue>(content) {
        Ok(doc) => uses_from_yaml(&doc),
        Err(_) => line_values.iter().map(|(v, _)| v.clone()).collect(),
    };
    values
        .iter()
        .filter_map(|v| uses_to_package(v, comments.get(v.as_str()).copied()))
        .collect()
}

/// Local composite action files anywhere under `.github`.
fn action_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let Ok(ft) = e.file_type() else {
            continue;
        };
        let path = e.path();
        if ft.is_dir() {
            action_files(&path, out);
        } else if ft.is_file()
            && path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| ACTION_FILES.contains(&n))
        {
            out.push(path);
        }
    }
}

/// From a user-provided path, find the repository root: the directory containing `.github/workflows`.
fn find_root(p: &Path) -> Option<PathBuf> {
    p.ancestors()
        .find(|d| workflows_dir(d).is_dir())
        .map(Path::to_path_buf)
}

//------------------------------------------------------------------------------
pub(crate) struct GhaScanner;

impl EcosystemScanner for GhaScanner {
    fn detect(&self, dir: &Path, items: &[DirItem]) -> Detect {
        let has = |name: &str| items.iter().any(|i| i.file_name() == Some(name));
        // `.git` may be a directory or (worktrees, submodules) a file
        let is_origin = has(GIT) && has(GITHUB) && workflows_dir(dir).is_dir();
        Detect {
            origins: if is_origin {
                vec![origin(dir.to_path_buf())]
            } else {
                Vec::new()
            },
            ..Default::default()
        }
    }

    /// Accept a repository directory with `.github/workflows`, the `.github` or `workflows` directory, or a workflow file. An explicitly provided path need not be a git repository.
    fn accepts(&self, path: &Path) -> bool {
        let Ok(p) = path_normalize(path, false) else {
            return false;
        };
        if p.is_file() {
            return is_yaml(&p)
                && p.parent().is_some_and(|w| {
                    w.file_name().is_some_and(|n| n == WORKFLOWS)
                        && w.parent()
                            .and_then(|g| g.file_name())
                            .is_some_and(|n| n == GITHUB)
                });
        }
        p.is_dir()
            && find_root(&p).is_some_and(|r| {
                p.starts_with(&r) && p.components().count() <= r.components().count() + 2
            })
    }

    fn normalize_origin(&self, path: &Path) -> ResultDynError<Origin> {
        let p = path_normalize(path, true)?;
        find_root(&p)
            .map(origin)
            .ok_or_else(|| format!("No .github/workflows found for {p:?}").into())
    }

    /// Each workflow file (top level of `.github/workflows`, as GitHub reads them) and each local `action.yml` under `.github`.
    fn origin_to_sites(
        &self,
        origin: &Origin,
        _config: &ScanConfig,
        _log: FlagLog,
    ) -> Vec<Site> {
        let root = origin.as_path();
        let mut files: Vec<PathBuf> = fs::read_dir(workflows_dir(root))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.path())
            .filter(|p| is_yaml(p))
            .collect();
        action_files(&root.join(GITHUB), &mut files);
        files
            .into_iter()
            .map(|f| Site::new(Ecosystem::Gha, SiteKind::Manifest, PathShared::from(f)))
            .collect()
    }

    fn site_to_packages(&self, site: &Site) -> Vec<Package> {
        let Ok(content) = fs::read_to_string(site.as_path()) else {
            return Vec::new();
        };
        let unique: HashSet<Package> =
            packages_from_workflow(&content).into_iter().collect();
        unique.into_iter().collect()
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const WORKFLOW: &str = r#"name: CI
on: [push]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-python@b64ffcaf5b410884ad320a9cfac8866006a109aa # v4.8.0
        with:
          python-version: "3.12"
      - name: pinned, major comment
        uses: actions/cache@0c45773b623bea8c8e75f6c82b208c3cf94ea4f9 # v4
      - uses: "codecov/codecov-action@v4.0.1"
      - uses: github/codeql-action/init@v3
      - uses: actions/checkout@master
      - uses: ./.github/actions/setup
      - uses: docker://alpine:3.19
      - uses: ${{ matrix.action }}
      - run: |
          echo "uses: not/an-action@v1"
  release:
    uses: curvenote/actions/.github/workflows/publish.yml@v1
"#;

    fn names(packages: &[Package]) -> Vec<String> {
        let mut v: Vec<String> = packages.iter().map(|p| p.to_string()).collect();
        v.sort();
        v.dedup();
        v
    }

    #[test]
    fn test_packages_from_workflow_a() {
        let packages = packages_from_workflow(WORKFLOW);
        assert_eq!(
            names(&packages),
            vec![
                // pinned with only a major comment: the SHA (unresolvable)
                "actions/cache-0c45773b623bea8c8e75f6c82b208c3cf94ea4f9",
                "actions/checkout-master",
                "actions/checkout-v4",
                // pinned with a full version comment: the comment's version
                "actions/setup-python-v4.8.0",
                "codecov/codecov-action-v4.0.1",
                // subpath action and reusable workflow: named by repository
                "curvenote/actions-v1",
                "github/codeql-action-v3",
            ]
        );
        assert!(packages.iter().all(|p| p.ecosystem == Ecosystem::Gha));
        let py = packages
            .iter()
            .find(|p| p.name == "actions/setup-python")
            .unwrap();
        let vcs = py.direct_url.clone().unwrap().vcs_info.unwrap();
        assert_eq!(vcs.commit_id, "b64ffcaf5b410884ad320a9cfac8866006a109aa");
        assert_eq!(vcs.revision.as_deref(), Some("v4.8.0"));
        let co = packages
            .iter()
            .find(|p| p.to_string() == "actions/checkout-v4")
            .unwrap();
        assert_eq!(co.direct_url, None);
    }

    #[test]
    fn test_packages_from_composite_action_a() {
        let action = r#"name: setup
runs:
  using: composite
  steps:
    - uses: actions/setup-node@v4
    - uses: pnpm/action-setup@a7487c7e89a18df4991f7f222e4898a00d66ddda # v4.1.0
"#;
        assert_eq!(
            names(&packages_from_workflow(action)),
            vec!["actions/setup-node-v4", "pnpm/action-setup-v4.1.0"]
        );
    }

    #[test]
    fn test_packages_from_invalid_yaml_a() {
        // not YAML (e.g. a template): fall back to lines
        let content = "jobs: {{ template }}\n  steps:\n    - uses: actions/checkout@v4 # pinned\n\t- bad: [";
        assert_eq!(
            names(&packages_from_workflow(content)),
            vec!["actions/checkout-v4"]
        );
    }

    #[test]
    fn test_parse_uses_line_a() {
        assert_eq!(
            parse_uses_line("      - uses: a/b@v1 # v1.2.3"),
            Some(("a/b@v1".to_string(), Some("v1.2.3".to_string())))
        );
        assert_eq!(
            parse_uses_line("    uses: 'a/b@main'"),
            Some(("a/b@main".to_string(), None))
        );
        assert_eq!(parse_uses_line("    run: echo"), None);
    }

    #[test]
    fn test_uses_to_package_a() {
        assert!(uses_to_package("actions/checkout", None).is_none());
        assert!(uses_to_package("./local@v1", None).is_none());
        assert!(uses_to_package("solo@v1", None).is_none());
        let p = uses_to_package("Owner/Repo/sub/path@v2", None).unwrap();
        assert_eq!(p.name, "Owner/Repo");
        assert_eq!(p.key, "owner/repo");
    }

    fn items_of(dir: &Path) -> Vec<DirItem> {
        fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| DirItem {
                path: e.path(),
                file_type: e.file_type().unwrap(),
            })
            .collect()
    }

    fn make_repo(root: &Path, git: bool) {
        fs::create_dir_all(workflows_dir(root)).unwrap();
        fs::write(workflows_dir(root).join("ci.yml"), WORKFLOW).unwrap();
        fs::write(workflows_dir(root).join("README.md"), "not a workflow").unwrap();
        let actions = root.join(".github/actions/setup");
        fs::create_dir_all(&actions).unwrap();
        fs::write(
            actions.join("action.yml"),
            "runs:\n  using: composite\n  steps:\n    - uses: actions/setup-node@v4\n",
        )
        .unwrap();
        if git {
            fs::create_dir(root.join(GIT)).unwrap();
        }
    }

    #[test]
    fn test_detect_a() {
        let dir = tempdir().unwrap();
        let repo = dir.path().join("repo");
        make_repo(&repo, true);
        let d = GhaScanner.detect(&repo, &items_of(&repo));
        assert_eq!(d.origins.len(), 1);
        assert!(!d.prune);
        // not a repository root: not an origin
        let copy = dir.path().join("copy");
        make_repo(&copy, false);
        assert!(GhaScanner
            .detect(&copy, &items_of(&copy))
            .origins
            .is_empty());
        // a .git file (worktree or submodule) counts
        let wt = dir.path().join("worktree");
        make_repo(&wt, false);
        fs::write(wt.join(GIT), "gitdir: ../repo/.git/worktrees/wt").unwrap();
        assert_eq!(GhaScanner.detect(&wt, &items_of(&wt)).origins.len(), 1);
    }

    #[test]
    fn test_origin_to_sites_a() {
        let dir = tempdir().unwrap();
        let repo = dir.path().to_path_buf();
        make_repo(&repo, true);
        let config = ScanConfig::new(false, false);
        let mut sites =
            GhaScanner.origin_to_sites(&origin(repo), &config, FlagLog(false));
        sites.sort();
        // the workflow and the composite action; README.md is not a workflow
        assert_eq!(sites.len(), 2);
        assert!(sites.iter().all(|s| s.kind == SiteKind::Manifest));
        let all: Vec<Package> = sites
            .iter()
            .flat_map(|s| GhaScanner.site_to_packages(s))
            .collect();
        assert!(names(&all).contains(&"actions/setup-node-v4".to_string()));
    }

    #[test]
    fn test_accepts_normalize_a() {
        let dir = tempdir().unwrap();
        let repo = dir.path().join("repo");
        make_repo(&repo, false);
        for p in [
            repo.clone(),
            repo.join(GITHUB),
            workflows_dir(&repo),
            workflows_dir(&repo).join("ci.yml"),
        ] {
            assert!(GhaScanner.accepts(&p), "{p:?}");
            assert_eq!(
                GhaScanner.normalize_origin(&p).unwrap().as_path(),
                repo,
                "{p:?}"
            );
        }
        // a non-workflow file, a deep subdirectory, and a dir without workflows are not accepted
        assert!(!GhaScanner.accepts(&workflows_dir(&repo).join("README.md")));
        assert!(!GhaScanner.accepts(&repo.join(".github/actions/setup")));
        assert!(!GhaScanner.accepts(dir.path()));
    }
}
