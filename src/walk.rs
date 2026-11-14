//! A single filesystem walk that dispatches each visited directory to all enabled ecosystem scanners.

use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use rayon::prelude::*;

use crate::eco::DirItem;
use crate::eco::EcosystemScanner;
use crate::origin_site::Origin;
use crate::util::path_home;
use crate::util::path_users;

/// Directory names never descended into, for any scanner.
const SKIP_DIR_NAMES: [&str; 2] = [".git", "__pycache__"];

//------------------------------------------------------------------------------
// Provide absolute paths for directories that should be excluded from search.
fn get_search_exclude_paths(homes: &HashSet<(PathBuf, bool)>) -> HashSet<PathBuf> {
    let mut paths: HashSet<PathBuf> = HashSet::new();

    for (home, _) in homes {
        paths.insert(home.clone().join(".cache"));
        paths.insert(home.clone().join(".npm"));

        if env::consts::OS == "macos" {
            paths.insert(home.clone().join("Library"));
            paths.insert(home.clone().join("Photos"));
            paths.insert(home.clone().join("Pictures"));
            paths.insert(home.clone().join("Downloads"));
            paths.insert(home.clone().join(".Trash"));
        } else if env::consts::OS == "linux" {
            paths.insert(home.clone().join(".local/share/Trash"));
        } else if env::consts::OS == "windows" {
            // Exclude noisy AppData subtrees so the recursive home walk stays
            // fast; per-user Python installs live under AppData\Local\Programs,
            // which is intentionally not excluded here.
            let local = home.clone().join("AppData").join("Local");
            paths.insert(local.join("Temp"));
            paths.insert(local.join("Microsoft"));
            paths.insert(local.join("Packages"));
        }
    }
    paths
}

// Get one or more home dirs to search: the home dir itself (not recursive), and each directory within it (recursive).
fn get_origins_home(all_users: bool) -> HashSet<(PathBuf, bool)> {
    let mut paths: HashSet<(PathBuf, bool)> = HashSet::new();

    let origin = match all_users {
        true => path_users(),
        false => path_home(),
    };
    match origin {
        Some(home) => {
            paths.insert((home.clone(), false));
            // collect all directories in the user's home directory
            match fs::read_dir(home) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            paths.insert((path, true));
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error reading home: {e}");
                }
            }
        }
        None => {
            eprintln!("Error getting origin");
        }
    }
    paths
}

//------------------------------------------------------------------------------
/// Read `dir` once, give its entries to every scanner, and (if `recurse` and no scanner prunes) descend into child directories in parallel. Symlinked directories are never followed.
fn walk_inner(
    dir: &Path,
    exclude_paths: &HashSet<PathBuf>,
    recurse: bool,
    scanners: &[&dyn EcosystemScanner],
) -> Vec<Origin> {
    if exclude_paths.contains(dir) {
        return Vec::new();
    }
    let items: Vec<DirItem> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .filter_map(|e| {
                e.file_type().ok().map(|file_type| DirItem {
                    path: e.path(),
                    file_type,
                })
            })
            .collect(),
        Err(e) => {
            eprintln!("Error reading {dir:?}: {e}");
            return Vec::new();
        }
    };
    let mut origins = Vec::new();
    let mut prune = false;
    let mut skip: Vec<&'static str> = SKIP_DIR_NAMES.to_vec();
    for scanner in scanners {
        let detect = scanner.detect(dir, &items);
        origins.extend(detect.origins);
        prune |= detect.prune;
        skip.extend(detect.skip_children);
    }
    if recurse && !prune {
        // `file_type` does not follow symlinks, so symlinked dirs are skipped
        let children: Vec<&DirItem> = items
            .iter()
            .filter(|item| {
                item.file_type.is_dir()
                    && !item.file_name().is_some_and(|n| skip.contains(&n))
            })
            .collect();
        // rayon's work stealing balances nested parallelism across deep and shallow subtrees
        origins.par_extend(children.into_par_iter().flat_map_iter(|item| {
            walk_inner(&item.path, exclude_paths, recurse, scanners)
        }));
    }
    origins
}

/// Walk the provided `(directory, recursive)` roots in parallel, collecting Origins from all scanners.
fn walk_roots(
    roots: &HashSet<(PathBuf, bool)>,
    exclude_paths: &HashSet<PathBuf>,
    scanners: &[&dyn EcosystemScanner],
) -> HashSet<Origin> {
    roots
        .par_iter()
        .filter(|(path, _)| path.is_dir())
        .flat_map_iter(|(path, recurse)| {
            walk_inner(path, exclude_paths, *recurse, scanners)
        })
        .collect()
}

/// Discover all Origins for the provided scanners with a single walk. If `roots` is empty, the home directories (or all users' directories) are walked plus each scanner's own roots, followed by each scanner's default Origins. If `roots` is provided, only those directories are walked (recursively).
pub(crate) fn discover(
    scanners: &[&dyn EcosystemScanner],
    all_users: bool,
    roots: &[PathBuf],
) -> HashSet<Origin> {
    let homes = get_origins_home(all_users);
    let exclude = get_search_exclude_paths(&homes);

    if !roots.is_empty() {
        let roots: HashSet<(PathBuf, bool)> =
            roots.iter().map(|r| (r.clone(), true)).collect();
        return walk_roots(&roots, &exclude, scanners);
    }
    let mut roots = homes;
    for scanner in scanners {
        roots.extend(scanner.roots());
    }
    let mut origins = walk_roots(&roots, &exclude, scanners);
    for scanner in scanners {
        origins.extend(scanner.default_origins());
    }
    origins
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::eco::Detect;
    use crate::ecosystem::Ecosystem;
    use crate::origin_site::Site;
    use crate::package::Package;
    use crate::path_shared::PathShared;
    use crate::util::FlagLog;
    use crate::util::ResultDynError;
    use crate::util::ScanConfig;
    use std::fs::File;
    use tempfile::tempdir;

    /// A scanner that claims any directory containing `marker.txt` as an Origin, and prunes directories containing `stop.txt`.
    struct MarkerScanner;

    impl EcosystemScanner for MarkerScanner {
        fn detect(&self, dir: &Path, items: &[DirItem]) -> Detect {
            let has = |name: &str| items.iter().any(|i| i.file_name() == Some(name));
            let mut origins = Vec::new();
            if has("marker.txt") {
                origins.push(Origin::new(
                    Ecosystem::Npm,
                    PathShared::from(dir.to_path_buf()),
                ));
            }
            Detect {
                origins,
                prune: has("stop.txt"),
                skip_children: if has("skip.txt") {
                    vec!["skipped"]
                } else {
                    Vec::new()
                },
            }
        }
        fn accepts(&self, _path: &Path) -> bool {
            false
        }
        fn normalize_origin(&self, _path: &Path) -> ResultDynError<Origin> {
            unimplemented!()
        }
        fn origin_to_sites(
            &self,
            _o: &Origin,
            _c: &ScanConfig,
            _l: FlagLog,
        ) -> Vec<Site> {
            Vec::new()
        }
        fn site_to_packages(&self, _site: &Site) -> Vec<Package> {
            Vec::new()
        }
    }

    fn touch(fp: PathBuf) {
        if let Some(parent) = fp.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        File::create(fp).unwrap();
    }

    fn walk(root: &Path, recurse: bool) -> Vec<PathBuf> {
        let mut roots = HashSet::new();
        roots.insert((root.to_path_buf(), recurse));
        let scanners: [&dyn EcosystemScanner; 1] = [&MarkerScanner];
        let mut found: Vec<PathBuf> = walk_roots(&roots, &HashSet::new(), &scanners)
            .into_iter()
            .map(|o| o.as_path().strip_prefix(root).unwrap().to_path_buf())
            .collect();
        found.sort();
        found
    }

    #[test]
    fn test_get_search_exclude_paths_a() {
        let mut homes = HashSet::<(PathBuf, bool)>::new();
        homes.insert((path_home().unwrap(), true));
        let post = get_search_exclude_paths(&homes);
        // one home yields .cache + .npm plus 3 Windows / 1 Linux / 5 macOS extras
        assert!(post.len() > 2);
    }

    #[test]
    fn test_walk_a() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        touch(root.join("marker.txt"));
        touch(root.join("a/marker.txt"));
        touch(root.join("a/b/c/marker.txt"));
        touch(root.join("d/other.txt"));

        assert_eq!(
            walk(root, true),
            vec![
                PathBuf::from(""),
                PathBuf::from("a"),
                PathBuf::from("a/b/c")
            ]
        );
        // not recursive: only the root is inspected
        assert_eq!(walk(root, false), vec![PathBuf::from("")]);
    }

    #[test]
    fn test_walk_prune_a() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        touch(root.join("a/marker.txt"));
        touch(root.join("a/stop.txt"));
        touch(root.join("a/b/marker.txt")); // below a pruned dir
        touch(root.join("c/d/marker.txt"));
        // a pruning dir is still reported; its children are not visited
        assert_eq!(
            walk(root, true),
            vec![PathBuf::from("a"), PathBuf::from("c/d")]
        );
    }

    #[test]
    fn test_walk_skip_children_a() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        touch(root.join("a/skip.txt"));
        touch(root.join("a/skipped/marker.txt")); // skipped by name
        touch(root.join("a/kept/marker.txt")); // sibling still visited
        touch(root.join("b/skipped/marker.txt")); // no skip.txt in b: visited
        assert_eq!(
            walk(root, true),
            vec![PathBuf::from("a/kept"), PathBuf::from("b/skipped")]
        );
    }

    #[test]
    fn test_walk_skip_names_a() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        touch(root.join(".git/marker.txt"));
        touch(root.join("x/__pycache__/marker.txt"));
        touch(root.join("x/y/marker.txt"));
        assert_eq!(walk(root, true), vec![PathBuf::from("x/y")]);
    }

    #[test]
    fn test_walk_exclude_a() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        touch(root.join("a/marker.txt"));
        touch(root.join("b/marker.txt"));
        let mut roots = HashSet::new();
        roots.insert((root.to_path_buf(), true));
        let mut exclude = HashSet::new();
        exclude.insert(root.join("a"));
        let scanners: [&dyn EcosystemScanner; 1] = [&MarkerScanner];
        let found = walk_roots(&roots, &exclude, &scanners);
        assert_eq!(found.len(), 1);
        assert!(found.iter().next().unwrap().as_path().ends_with("b"));
    }

    #[cfg(unix)]
    #[test]
    fn test_walk_symlink_a() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        touch(root.join("real/marker.txt"));
        std::os::unix::fs::symlink(root.join("real"), root.join("link")).unwrap();
        // the symlinked dir is not followed
        assert_eq!(walk(root, true), vec![PathBuf::from("real")]);
    }

    #[test]
    fn test_discover_roots_a() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        touch(root.join("a/marker.txt"));
        touch(root.join("b/c/marker.txt"));
        let scanners: [&dyn EcosystemScanner; 1] = [&MarkerScanner];
        // only the provided root is walked: nothing from home dirs
        let found = discover(&scanners, false, &[root.join("b")]);
        assert_eq!(found.len(), 1);
        assert!(found.iter().next().unwrap().as_path().ends_with("b/c"));
    }

    #[test]
    fn test_walk_missing_root_a() {
        let dir = tempdir().unwrap();
        assert!(walk(&dir.path().join("does-not-exist"), true).is_empty());
    }
}
