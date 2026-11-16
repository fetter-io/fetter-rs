//! Go scanner.
//!
//! An origin is a directory containing a `go.mod` whose `module` path is a valid module path (templates such as `module ${PROJECT}` are not projects). Its site is the `go.mod` itself (`SiteKind::Manifest`): since Go 1.17, `go.mod` lists the module's full build list (direct and `// indirect` requirements), so it is the resolution. `go.sum` is not read, as it also records modules outside the build list. Packages are module paths (case-sensitive) with versions as written (`v1.2.3`, pseudo-versions, `+incompatible`).

use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::eco::Detect;
use crate::eco::DirItem;
use crate::eco::EcosystemScanner;
use crate::ecosystem::Ecosystem;
use crate::origin_site::Origin;
use crate::origin_site::Site;
use crate::origin_site::SiteKind;
use crate::package::Package;
use crate::path_shared::PathShared;
use crate::util::path_home;
use crate::util::path_normalize;
use crate::util::FlagLog;
use crate::util::ResultDynError;
use crate::util::ScanConfig;
use crate::version_spec::VersionSpec;

const GO_MOD: &str = "go.mod";

/// Directories beside a `go.mod` that hold no projects: `vendor` (copied dependency sources) and `testdata` (ignored by the go tool; often holds fixture modules).
const SKIP_CHILDREN: [&str; 2] = ["vendor", "testdata"];

//------------------------------------------------------------------------------
/// The module cache (`$GOMODCACHE`, else `$GOPATH/pkg/mod`, else `~/go/pkg/mod`), which holds the `go.mod` of every downloaded module version.
fn mod_cache() -> Option<&'static Path> {
    static MOD_CACHE: OnceLock<Option<PathBuf>> = OnceLock::new();
    MOD_CACHE
        .get_or_init(|| {
            let var = |name: &str| env::var_os(name).filter(|v| !v.is_empty());
            if let Some(p) = var("GOMODCACHE") {
                return Some(PathBuf::from(p));
            }
            // GOPATH may be a list; the module cache is under its first entry
            let gopath = match var("GOPATH") {
                Some(p) => env::split_paths(&p).next(),
                None => path_home().map(|h| h.join("go")),
            };
            gopath.map(|g| g.join("pkg").join("mod"))
        })
        .as_deref()
}

fn origin(dir: PathBuf) -> Origin {
    Origin::new(Ecosystem::Go, PathShared::from(dir))
}

/// True if `path` can be a module path: non-empty and made of the characters Go permits in module paths (letters, digits, and `-._~/+`), not starting or ending with `/`.
fn is_module_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.ends_with('/')
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._~/+".contains(c))
}

//------------------------------------------------------------------------------
// versions

/// A Go module version (`v1.2.3`, a pseudo-version, `v2.0.0+incompatible`) as semver, without its `v`.
pub(crate) fn semver_of(version: &str) -> Option<semver::Version> {
    semver::Version::parse(version.strip_prefix('v')?).ok()
}

/// Remove the `v` from each version in a requirement (`">=v1.2.3 <v2"` becomes `">=1.2.3 <2"`), so that it parses with npm range semantics: a `v` directly followed by a digit and not preceded by an alphanumeric character.
pub(crate) fn strip_version_v(spec: &str) -> String {
    let chars: Vec<char> = spec.chars().collect();
    let mut out = String::with_capacity(spec.len());
    for (i, c) in chars.iter().enumerate() {
        let next_digit = chars.get(i + 1).is_some_and(|n| n.is_ascii_digit());
        let prev_alnum = i > 0 && chars[i - 1].is_ascii_alphanumeric();
        if (*c == 'v' || *c == 'V') && next_digit && !prev_alnum {
            continue;
        }
        out.push(*c);
    }
    out
}

/// Escape a module path for the module proxy: each uppercase letter becomes `!` and its lowercase (`github.com/BurntSushi/toml` is `github.com/!burnt!sushi/toml`).
pub(crate) fn proxy_escape(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if c.is_ascii_uppercase() {
            out.push('!');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

//------------------------------------------------------------------------------
// go.mod parsing

/// The parts of a `go.mod` that determine its packages.
#[derive(Debug, Default, PartialEq)]
struct GoMod {
    module: Option<String>,
    /// `(path, version)` from `require` directives.
    requires: Vec<(String, String)>,
    /// `(old path, old version if given, replacement)` from `replace` directives.
    replaces: Vec<(String, Option<String>, Replacement)>,
}

#[derive(Debug, PartialEq)]
enum Replacement {
    /// A module path and version.
    Module(String, String),
    /// A local directory (`./x`, `../x`, or absolute): source not from a registry.
    Local,
}

/// Split a line into tokens, removing a trailing `//` comment and the quotes of quoted tokens (`"path"` or `` `path` ``).
fn tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    let mut cur = String::new();
    while let Some(c) = chars.next() {
        match c {
            '/' if chars.peek() == Some(&'/') => break,
            '"' | '`' => {
                // quoted token; Go string escapes are not expected in module paths
                for q in chars.by_ref() {
                    if q == c {
                        break;
                    }
                    cur.push(q);
                }
            }
            c if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// True if a replacement target is a local directory rather than a module path: Go treats targets starting with `./`, `../`, or `/` (or a Windows drive or `.\`/`..\`) as directories.
fn is_local_target(t: &str) -> bool {
    t.starts_with("./")
        || t.starts_with("../")
        || t.starts_with('/')
        || t.starts_with(".\\")
        || t.starts_with("..\\")
        || t == "."
        || t == ".."
        || (t.len() >= 2 && t.as_bytes()[1] == b':')
}

fn parse_replace(args: &[String]) -> Option<(String, Option<String>, Replacement)> {
    let arrow = args.iter().position(|a| a == "=>")?;
    let (old, new) = (&args[..arrow], &args[arrow + 1..]);
    let old_path = old.first()?.clone();
    let old_version = old.get(1).cloned();
    let target = new.first()?;
    let replacement = if is_local_target(target) || new.len() < 2 {
        Replacement::Local
    } else {
        Replacement::Module(target.clone(), new[1].clone())
    };
    Some((old_path, old_version, replacement))
}

fn parse_go_mod(content: &str) -> GoMod {
    let mut gm = GoMod::default();
    // the directive of the block being read, if inside `directive ( ... )`
    let mut block: Option<String> = None;
    for line in content.lines() {
        let toks = tokens(line);
        if toks.is_empty() {
            continue;
        }
        let (directive, args): (String, &[String]) = match &block {
            Some(d) => {
                if toks[0] == ")" {
                    block = None;
                    continue;
                }
                (d.clone(), &toks[..])
            }
            None => {
                if toks.len() == 2 && toks[1] == "(" {
                    block = Some(toks[0].clone());
                    continue;
                }
                (toks[0].clone(), &toks[1..])
            }
        };
        match directive.as_str() {
            "module" => gm.module = args.first().cloned(),
            "require" if args.len() >= 2 => {
                gm.requires.push((args[0].clone(), args[1].clone()));
            }
            "replace" => {
                if let Some(r) = parse_replace(args) {
                    gm.replaces.push(r);
                }
            }
            // go, toolchain, godebug, exclude, retract, tool, ignore: no packages
            _ => {}
        }
    }
    gm
}

/// The packages of a `go.mod`: its requirements after applying `replace` directives (a version-specific replacement takes precedence over a path-wide one). Modules replaced by a local directory, and the main module itself, are excluded.
fn packages_from_go_mod(content: &str) -> Vec<Package> {
    let gm = parse_go_mod(content);
    let mut out = HashSet::new();
    for (path, version) in &gm.requires {
        if gm.module.as_deref() == Some(path.as_str()) {
            continue;
        }
        let replacement = gm
            .replaces
            .iter()
            .find(|(p, v, _)| p == path && v.as_deref() == Some(version.as_str()))
            .or_else(|| {
                gm.replaces
                    .iter()
                    .find(|(p, v, _)| p == path && v.is_none())
            })
            .map(|(_, _, r)| r);
        let (p, v) = match replacement {
            Some(Replacement::Local) => continue,
            Some(Replacement::Module(p, v)) => (p, v),
            None => (path, version),
        };
        out.insert(Package::new(Ecosystem::Go, p, VersionSpec::new(v), None));
    }
    out.into_iter().collect()
}

/// The `module` path of a `go.mod` file, if it is a valid module path.
fn module_of(fp: &Path) -> Option<String> {
    let content = fs::read_to_string(fp).ok()?;
    parse_go_mod(&content).module.filter(|m| is_module_path(m))
}

//------------------------------------------------------------------------------
pub(crate) struct GoScanner;

impl EcosystemScanner for GoScanner {
    fn cache_dirs(&self) -> Vec<PathBuf> {
        mod_cache().map(Path::to_path_buf).into_iter().collect()
    }

    fn detect(&self, dir: &Path, items: &[DirItem]) -> Detect {
        let fp = dir.join(GO_MOD);
        if !items.iter().any(|i| i.file_name() == Some(GO_MOD)) {
            return Detect::default();
        }
        let origins = if module_of(&fp).is_some() {
            vec![origin(dir.to_path_buf())]
        } else {
            Vec::new()
        };
        Detect {
            origins,
            prune: false,
            skip_children: SKIP_CHILDREN.to_vec(),
        }
    }

    /// Accept a `go.mod` file or a directory containing one.
    fn accepts(&self, path: &Path) -> bool {
        let Ok(p) = path_normalize(path, false) else {
            return false;
        };
        if p.is_file() {
            return p.file_name().is_some_and(|n| n == GO_MOD);
        }
        p.is_dir() && p.join(GO_MOD).is_file()
    }

    fn normalize_origin(&self, path: &Path) -> ResultDynError<Origin> {
        let p = path_normalize(path, true)?;
        let dir = if p.is_file() {
            p.parent()
                .ok_or("Cannot get parent directory")?
                .to_path_buf()
        } else {
            p
        };
        let fp = dir.join(GO_MOD);
        if !fp.is_file() {
            return Err(format!("No go.mod found in {dir:?}").into());
        }
        if module_of(&fp).is_none() {
            return Err(format!("{fp:?} does not declare a valid module path").into());
        }
        Ok(origin(dir))
    }

    fn origin_to_sites(
        &self,
        origin: &Origin,
        _config: &ScanConfig,
        _log: FlagLog,
    ) -> Vec<Site> {
        let fp = origin.as_path().join(GO_MOD);
        if fp.is_file() {
            vec![Site::new(
                Ecosystem::Go,
                SiteKind::Manifest,
                PathShared::from(fp),
            )]
        } else {
            Vec::new()
        }
    }

    fn site_to_packages(&self, site: &Site) -> Vec<Package> {
        fs::read_to_string(site.as_path())
            .map(|c| packages_from_go_mod(&c))
            .unwrap_or_default()
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const GO_MOD_A: &str = r#"module go.etcd.io/etcd/server/v3

go 1.27

toolchain go1.27.4

godebug (
	winsymlink=0
)

require (
	github.com/coreos/go-systemd/v22 v22.7.0
	github.com/xiang90/probing v0.0.0-20221125231312-a49e3df8f510 // indirect
	go.etcd.io/etcd/api/v3 v3.8.0-alpha.0
	"github.com/quoted/mod" v1.0.0
)

require github.com/single/line v1.2.3 // indirect

require go.etcd.io/etcd/server/v3 v3.0.0

replace go.etcd.io/etcd/api/v3 => ../api

replace (
	github.com/coreos/go-systemd/v22 v22.7.0 => github.com/fork/go-systemd/v22 v22.7.1
	github.com/single/line => github.com/other/line v1.2.4
)

exclude github.com/excluded/mod v0.1.0

retract [v1.0.0, v1.0.5]

tool (
	golang.org/x/tools/cmd/stringer
)
"#;

    fn names(packages: &[Package]) -> Vec<String> {
        let mut out: Vec<String> = packages.iter().map(|p| p.to_string()).collect();
        out.sort();
        out
    }

    #[test]
    fn test_tokens_a() {
        assert_eq!(
            tokens("\tgithub.com/a/b v1.0.0 // indirect"),
            vec!["github.com/a/b", "v1.0.0"]
        );
        assert_eq!(tokens(r#""a b" v1"#), vec!["a b", "v1"]);
        assert_eq!(tokens("// only a comment"), Vec::<String>::new());
        assert_eq!(tokens("require ("), vec!["require", "("]);
    }

    #[test]
    fn test_parse_go_mod_a() {
        let gm = parse_go_mod(GO_MOD_A);
        assert_eq!(gm.module.as_deref(), Some("go.etcd.io/etcd/server/v3"));
        assert_eq!(gm.requires.len(), 6);
        assert_eq!(gm.replaces.len(), 3);
        assert_eq!(
            gm.replaces[0],
            (
                "go.etcd.io/etcd/api/v3".to_string(),
                None,
                Replacement::Local
            )
        );
    }

    #[test]
    fn test_packages_from_go_mod_a() {
        let packages = packages_from_go_mod(GO_MOD_A);
        assert_eq!(
            names(&packages),
            vec![
                // replaced by version-specific module replacement
                "github.com/fork/go-systemd/v22-v22.7.1",
                // replaced by path-wide module replacement
                "github.com/other/line-v1.2.4",
                "github.com/quoted/mod-v1.0.0",
                "github.com/xiang90/probing-v0.0.0-20221125231312-a49e3df8f510",
            ]
        );
        // versions round-trip exactly
        assert!(packages
            .iter()
            .any(|p| p.version.to_string() == "v0.0.0-20221125231312-a49e3df8f510"));
    }

    #[test]
    fn test_packages_from_go_mod_version_specific_replace_a() {
        let content = "module m\nrequire a.com/x v1.0.0\nreplace a.com/x v0.9.0 => ./x\n";
        // the replacement names another version, so it does not apply
        assert_eq!(
            names(&packages_from_go_mod(content)),
            vec!["a.com/x-v1.0.0"]
        );
    }

    #[test]
    fn test_semver_of_a() {
        assert_eq!(semver_of("v1.2.3").unwrap().to_string(), "1.2.3");
        assert!(semver_of("v0.0.0-20221125231312-a49e3df8f510").is_some());
        assert!(semver_of("v2.0.0+incompatible").is_some());
        assert!(semver_of("1.2.3").is_none());
        assert!(semver_of("v1.2").is_none());
    }

    #[test]
    fn test_strip_version_v_a() {
        assert_eq!(strip_version_v(">=v1.2.3 <v2"), ">=1.2.3 <2");
        assert_eq!(strip_version_v("v1.0.0 || ^v2"), "1.0.0 || ^2");
        assert_eq!(
            strip_version_v("v0.0.0-20221125231312-a49e3df8f510"),
            "0.0.0-20221125231312-a49e3df8f510"
        );
        assert_eq!(strip_version_v("*"), "*");
    }

    #[test]
    fn test_proxy_escape_a() {
        assert_eq!(
            proxy_escape("github.com/BurntSushi/toml"),
            "github.com/!burnt!sushi/toml"
        );
        assert_eq!(proxy_escape("golang.org/x/net"), "golang.org/x/net");
    }

    #[test]
    fn test_is_module_path_a() {
        assert!(is_module_path("github.com/cli/cli/v2"));
        assert!(is_module_path("demo"));
        assert!(!is_module_path("${PROJECT}"));
        assert!(!is_module_path(""));
        assert!(!is_module_path("/abs"));
    }

    #[test]
    fn test_is_local_target_a() {
        assert!(is_local_target("./x"));
        assert!(is_local_target("../../.."));
        assert!(is_local_target("/abs/path"));
        assert!(is_local_target("C:\\src\\x"));
        assert!(!is_local_target("github.com/a/b"));
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

    #[test]
    fn test_detect_a() {
        let dir = tempdir().unwrap();
        let proj = dir.path().join("proj");
        fs::create_dir(&proj).unwrap();
        fs::write(proj.join(GO_MOD), GO_MOD_A).unwrap();
        let d = GoScanner.detect(&proj, &items_of(&proj));
        assert_eq!(d.origins.len(), 1);
        assert_eq!(d.skip_children, vec!["vendor", "testdata"]);

        // a template is not a project
        let tmpl = dir.path().join("tmpl");
        fs::create_dir(&tmpl).unwrap();
        fs::write(tmpl.join(GO_MOD), "module ${PROJECT}\n\ngo 1.17\n").unwrap();
        let d = GoScanner.detect(&tmpl, &items_of(&tmpl));
        assert!(d.origins.is_empty());

        let empty = dir.path().join("empty");
        fs::create_dir(&empty).unwrap();
        let d = GoScanner.detect(&empty, &items_of(&empty));
        assert!(d.origins.is_empty() && d.skip_children.is_empty());
    }

    #[test]
    fn test_origin_to_sites_and_packages_a() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(GO_MOD), GO_MOD_A).unwrap();
        assert!(GoScanner.accepts(dir.path()));
        assert!(GoScanner.accepts(&dir.path().join(GO_MOD)));
        let o = GoScanner
            .normalize_origin(&dir.path().join(GO_MOD))
            .unwrap();
        assert_eq!(o.as_path(), dir.path());
        let sites =
            GoScanner.origin_to_sites(&o, &ScanConfig::new(false, false), FlagLog(false));
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].kind, SiteKind::Manifest);
        assert_eq!(GoScanner.site_to_packages(&sites[0]).len(), 4);
    }

    #[test]
    fn test_normalize_origin_template_a() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(GO_MOD), "module ${PROJECT}\n").unwrap();
        assert!(GoScanner.normalize_origin(dir.path()).is_err());
    }
}
