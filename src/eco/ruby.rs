//! Ruby (RubyGems) scanner.
//!
//! Origins are (1) project directories containing a Bundler lockfile (`Gemfile.lock` or `gems.locked`), whose site is the lockfile (`SiteKind::Lockfile`), and (2) gem directories: a directory with `specifications` and `gems` subdirectories (e.g. `~/.gem/ruby/3.3.0`, an rbenv or rvm Ruby's `lib/ruby/gems/3.3.0`, a project's `vendor/bundle/ruby/3.3.0`, or a system location such as `/Library/Ruby/Gems/2.6.0`), whose site is the directory itself (`SiteKind::Installed`). Installed gems are read from their `.gemspec` files (including `specifications/default`, the gems bundled with Ruby), identified by the stub line RubyGems writes first (`# stub: name version platform require_paths`).
//!
//! Gem names are case-sensitive (OSV and rubygems.org treat `Nokogiri` and `nokogiri` as different). Versions have their platform removed (`1.17.1-arm64-darwin` is `1.17.1`), as OSV's matching differs with it.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use crate::eco::head_lines;
use crate::eco::Artifacts;
use crate::eco::Detect;
use crate::eco::DirItem;
use crate::eco::EcosystemScanner;
use crate::eco::Hook;
use crate::eco::HOOK_HEAD_LINES;
use crate::ecosystem::Ecosystem;
use crate::origin_site::Origin;
use crate::origin_site::Site;
use crate::origin_site::SiteKind;
use crate::package::Package;
use crate::package_durl::DirectURL;
use crate::package_durl::VcsInfo;
use crate::path_shared::PathShared;
use crate::util::path_normalize;
use crate::util::FlagLog;
use crate::util::ResultDynError;
use crate::util::ScanConfig;
use crate::version_spec::VersionSpec;

const LOCKFILES: [&str; 2] = ["Gemfile.lock", "gems.locked"];
const MANIFESTS: [&str; 2] = ["Gemfile", "gems.rb"];
const SPECIFICATIONS: &str = "specifications";
const GEMS: &str = "gems";
const RUBYGEMS_ORG: &str = "https://rubygems.org/";

/// Directories that contain gem directories (one per Ruby ABI version, e.g. `2.6.0`) outside home directories, for system and package-manager Rubies.
const SYSTEM_GEM_PARENTS: [&str; 7] = [
    "/Library/Ruby/Gems",
    "/opt/homebrew/lib/ruby/gems",
    "/opt/homebrew/opt/ruby/lib/ruby/gems",
    "/usr/local/lib/ruby/gems",
    "/usr/local/opt/ruby/lib/ruby/gems",
    "/usr/lib/ruby/gems",
    "/var/lib/gems",
];
/// macOS's system Ruby keeps its gems under each framework version.
const MACOS_RUBY_FRAMEWORK: &str = "/System/Library/Frameworks/Ruby.framework/Versions";

//------------------------------------------------------------------------------
// versions and requirements (RubyGems semantics)

#[derive(Debug, Clone, PartialEq, Eq)]
enum Seg {
    Num(u64),
    Str(String),
}

/// A RubyGems version (`Gem::Version`): dot-separated segments of digits or letters; any letter makes it a pre-release. Ordering follows `Gem::Version#<=>`: trailing zeros are insignificant, and a string segment sorts before a numeric one (so `1.0.a` < `1.0`).
#[derive(Debug, Clone)]
pub(crate) struct GemVersion {
    /// The version as written.
    text: String,
    segments: Vec<Seg>,
}

impl GemVersion {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        let text = s.trim();
        if text.is_empty()
            || !text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
            || !text.starts_with(|c: char| c.is_ascii_digit())
        {
            return None;
        }
        // as RubyGems: a `-` is a pre-release marker (`1.0-rc1` is `1.0.pre.rc1`)
        let canonical = text.replace('-', ".pre.");
        let mut segments = Vec::new();
        let mut cur = String::new();
        let flush = |cur: &mut String, segments: &mut Vec<Seg>| -> Option<()> {
            if !cur.is_empty() {
                let seg = if cur.chars().all(|c| c.is_ascii_digit()) {
                    Seg::Num(cur.parse().ok()?)
                } else {
                    Seg::Str(cur.clone())
                };
                segments.push(seg);
                cur.clear();
            }
            Some(())
        };
        for c in canonical.chars() {
            let boundary = match cur.chars().last() {
                Some(p) => c == '.' || p.is_ascii_digit() != c.is_ascii_digit(),
                None => c == '.',
            };
            if boundary {
                flush(&mut cur, &mut segments)?;
            }
            if c != '.' {
                cur.push(c);
            }
        }
        flush(&mut cur, &mut segments)?;
        Some(GemVersion {
            text: text.to_string(),
            segments,
        })
    }

    fn is_prerelease(&self) -> bool {
        self.segments.iter().any(|s| matches!(s, Seg::Str(_)))
    }

    /// Segments with trailing zeros removed from the numeric part and from the part after the first string segment, as `Gem::Version#canonical_segments`.
    fn canonical(&self) -> Vec<Seg> {
        let split = self
            .segments
            .iter()
            .position(|s| matches!(s, Seg::Str(_)))
            .unwrap_or(self.segments.len());
        let trim = |part: &[Seg]| -> Vec<Seg> {
            let mut v = part.to_vec();
            while v.last() == Some(&Seg::Num(0)) {
                v.pop();
            }
            v
        };
        let mut out = trim(&self.segments[..split]);
        out.extend(trim(&self.segments[split..]));
        out
    }

    /// The version without its pre-release part (`Gem::Version#release`).
    fn release(&self) -> GemVersion {
        if !self.is_prerelease() {
            return self.clone();
        }
        let segments: Vec<Seg> = self
            .segments
            .iter()
            .take_while(|s| matches!(s, Seg::Num(_)))
            .cloned()
            .collect();
        GemVersion {
            text: String::new(),
            segments,
        }
    }

    /// The upper bound of `~>` (`Gem::Version#bump`): drop string segments, drop the last segment if there are several, and increment the last.
    fn bump(&self) -> GemVersion {
        let mut segments = self.segments.clone();
        while segments.iter().any(|s| matches!(s, Seg::Str(_))) {
            segments.pop();
        }
        if segments.len() > 1 {
            segments.pop();
        }
        if let Some(Seg::Num(n)) = segments.last_mut() {
            *n += 1;
        } else {
            segments.push(Seg::Num(1));
        }
        GemVersion {
            text: String::new(),
            segments,
        }
    }
}

impl Ord for GemVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        let (a, b) = (self.canonical(), other.canonical());
        let zero = Seg::Num(0);
        for i in 0..a.len().max(b.len()) {
            let (l, r) = (a.get(i).unwrap_or(&zero), b.get(i).unwrap_or(&zero));
            let o = match (l, r) {
                (Seg::Num(x), Seg::Num(y)) => x.cmp(y),
                (Seg::Str(x), Seg::Str(y)) => x.cmp(y),
                (Seg::Str(_), Seg::Num(_)) => Ordering::Less,
                (Seg::Num(_), Seg::Str(_)) => Ordering::Greater,
            };
            if o != Ordering::Equal {
                return o;
            }
        }
        Ordering::Equal
    }
}
impl PartialOrd for GemVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl PartialEq for GemVersion {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for GemVersion {}
impl fmt::Display for GemVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
    Pessimistic,
}

/// A RubyGems requirement (`Gem::Requirement`): comma-separated constraints that must all hold, each an operator (`=`, `!=`, `>`, `<`, `>=`, `<=`, `~>`; default `=`) and a version.
#[derive(Debug, Clone)]
pub(crate) struct GemRequirement(Vec<(Op, GemVersion)>);

impl GemRequirement {
    pub(crate) fn parse(s: &str) -> Result<Self, String> {
        let mut out = Vec::new();
        for part in s.split(',') {
            let p = part.trim();
            // longest operators first
            let ops = [
                ("~>", Op::Pessimistic),
                (">=", Op::Ge),
                ("<=", Op::Le),
                ("!=", Op::Ne),
                ("=", Op::Eq),
                (">", Op::Gt),
                ("<", Op::Lt),
            ];
            let (op, rest) = ops
                .iter()
                .find_map(|(t, o)| p.strip_prefix(t).map(|r| (*o, r)))
                .unwrap_or((Op::Eq, p));
            let v = GemVersion::parse(rest)
                .ok_or_else(|| format!("not a RubyGems requirement: {s:?}"))?;
            out.push((op, v));
        }
        Ok(GemRequirement(out))
    }

    pub(crate) fn matches(&self, v: &GemVersion) -> bool {
        self.0.iter().all(|(op, r)| match op {
            Op::Eq => v == r,
            Op::Ne => v != r,
            Op::Gt => v > r,
            Op::Lt => v < r,
            Op::Ge => v >= r,
            Op::Le => v <= r,
            Op::Pessimistic => v >= r && v.release() < r.bump(),
        })
    }

    /// The version, if this requirement names exactly one (`1.2.3` or `= 1.2.3`).
    pub(crate) fn exact(&self) -> Option<&GemVersion> {
        match self.0.as_slice() {
            [(Op::Eq, v)] => Some(v),
            _ => None,
        }
    }
}

//------------------------------------------------------------------------------
// lockfiles

/// Remove a platform from a locked or installed version: `1.17.1-arm64-darwin` is `1.17.1`. (A `-` in the version itself is not written by Bundler, which uses the canonical `.pre.` form.)
fn strip_platform(version: &str) -> &str {
    version.split_once('-').map(|(v, _)| v).unwrap_or(version)
}

fn package(name: &str, version: &str, direct_url: Option<DirectURL>) -> Package {
    Package::new(
        Ecosystem::Ruby,
        name,
        VersionSpec::new(strip_platform(version)),
        direct_url,
    )
}

/// Parse a spec line, `name (version)`, at the specs indentation.
fn parse_spec_entry(line: &str) -> Option<(&str, &str)> {
    let (name, rest) = line.trim().split_once(" (")?;
    Some((name, rest.strip_suffix(')')?))
}

/// The resolved gems of a Bundler lockfile. Gems from `GEM` sections come from rubygems.org (no provenance) or another gem server (its URL); gems from `PLUGIN SOURCE` sections (sources provided by Bundler plugins) carry their source's `remote`; gems from `GIT` sections carry the repository and revision; `PATH` gems (local sources, such as the project's own gem) are excluded. Each name and version appears once, though a lockfile may lock several platform variants.
fn packages_from_lockfile(content: &str) -> Vec<Package> {
    let mut out = HashSet::new();
    let mut section = "";
    let mut remote: Option<String> = None;
    let mut revision: Option<String> = None;
    let mut git_ref: Option<String> = None;
    let mut in_specs = false;
    for line in content.lines() {
        if !line.starts_with(' ') {
            section = line.trim();
            remote = None;
            revision = None;
            git_ref = None;
            in_specs = false;
            continue;
        }
        if !matches!(section, "GEM" | "GIT" | "PLUGIN SOURCE") {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let t = line.trim();
        if indent == 2 {
            in_specs = t == "specs:";
            if let Some((k, v)) = t.split_once(": ") {
                match k {
                    // the first remote names the source; later ones are fallbacks
                    "remote" if remote.is_none() => remote = Some(v.to_string()),
                    "revision" => revision = Some(v.to_string()),
                    "ref" | "branch" | "tag" => git_ref = Some(v.to_string()),
                    _ => {}
                }
            }
            continue;
        }
        if !in_specs || indent != 4 {
            // deeper lines are a gem's dependencies
            continue;
        }
        let Some((name, version)) = parse_spec_entry(t) else {
            continue;
        };
        let direct_url = match section {
            "GIT" => remote.as_ref().map(|url| DirectURL {
                url: url.clone(),
                vcs_info: revision.as_ref().map(|r| VcsInfo {
                    commit_id: r.clone(),
                    vcs: "git".to_string(),
                    revision: git_ref.clone(),
                }),
            }),
            _ => remote
                .as_ref()
                .filter(|r| r.trim_end_matches('/') != RUBYGEMS_ORG.trim_end_matches('/'))
                .map(|url| DirectURL {
                    url: url.clone(),
                    vcs_info: None,
                }),
        };
        out.insert(package(name, version, direct_url));
    }
    out.into_iter().collect()
}

//------------------------------------------------------------------------------
// installed gems

/// What a `.gemspec` file says about an installed gem.
#[derive(Debug, PartialEq)]
struct GemSpec {
    name: String,
    version: String,
    /// Native extension build scripts, run at install (`s.extensions`).
    extensions: Vec<String>,
}

/// The first double-quoted string in `s`.
fn first_quoted(s: &str) -> Option<&str> {
    let start = s.find('"')? + 1;
    let len = s[start..].find('"')?;
    Some(&s[start..start + len])
}

/// All double-quoted strings in `s`.
fn all_quoted(s: &str) -> Vec<String> {
    s.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

/// Parse a `.gemspec` written by RubyGems. The identity is from the first stub line (`# stub: name version platform require_paths`), else from `s.name` and `s.version`; extensions are from a second stub line (`# stub: ext/x/extconf.rb ...`), else from `s.extensions`.
fn parse_gemspec(content: &str) -> Option<GemSpec> {
    let mut name = None;
    let mut version = None;
    let mut extensions = Vec::new();
    let mut stubs = 0;
    for line in content.lines() {
        let t = line.trim();
        if let Some(stub) = t.strip_prefix("# stub: ") {
            stubs += 1;
            if stubs == 1 {
                let mut parts = stub.split_whitespace();
                name = parts.next().map(str::to_string);
                version = parts.next().map(str::to_string);
            } else if stubs == 2 {
                extensions = stub.split_whitespace().map(str::to_string).collect();
            }
        } else if let Some(rest) = t.strip_prefix("s.name = ") {
            name = name.or_else(|| first_quoted(rest).map(str::to_string));
        } else if let Some(rest) = t.strip_prefix("s.version = ") {
            version = version.or_else(|| first_quoted(rest).map(str::to_string));
        } else if let Some(rest) = t.strip_prefix("s.extensions = ") {
            if extensions.is_empty() {
                extensions = all_quoted(rest);
            }
        }
    }
    Some(GemSpec {
        name: name?,
        version: version?,
        extensions,
    })
}

fn is_gem_dir(dir: &Path) -> bool {
    dir.join(SPECIFICATIONS).is_dir() && dir.join(GEMS).is_dir()
}

/// Each installed gem's spec file and its full name (the file stem, `name-version[-platform]`, which also names its directories under `gems`, `extensions` and `cache`).
fn spec_files(gem_dir: &Path) -> Vec<(PathBuf, String)> {
    let specs = gem_dir.join(SPECIFICATIONS);
    [specs.clone(), specs.join("default")]
        .iter()
        .flat_map(|d| fs::read_dir(d).into_iter().flatten().flatten())
        .map(|e| e.path())
        .filter_map(|p| {
            let stem = p
                .file_name()?
                .to_str()?
                .strip_suffix(".gemspec")?
                .to_string();
            Some((p, stem))
        })
        .collect()
}

/// Each installed gem's package, spec file and full name.
fn installed_gems(gem_dir: &Path) -> Vec<(Package, GemSpec, PathBuf, String)> {
    spec_files(gem_dir)
        .into_iter()
        .filter_map(|(fp, full)| {
            let spec = parse_gemspec(&fs::read_to_string(&fp).ok()?)?;
            let p = package(&spec.name, &spec.version, None);
            Some((p, spec, fp, full))
        })
        .collect()
}

fn children(d: &Path) -> Vec<PathBuf> {
    fs::read_dir(d)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect()
}

/// Candidate gem directories of RubyInstaller Rubies (Windows) under `root` (the system drive): `<root>\Ruby<version>[-<arch>]\lib\ruby\gems\<abi>`, e.g. `C:\Ruby33-x64\lib\ruby\gems\3.3.0`.
fn ruby_installer_gem_dirs(root: &Path) -> Vec<PathBuf> {
    children(root)
        .into_iter()
        .filter(|d| {
            d.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.to_ascii_lowercase().starts_with("ruby"))
        })
        .flat_map(|d| children(&d.join("lib").join("ruby").join("gems")))
        .collect()
}

/// Gem directories outside home directories (system and package-manager Rubies, and RubyInstaller on Windows).
fn system_gem_dirs() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = SYSTEM_GEM_PARENTS
        .iter()
        .flat_map(|p| children(Path::new(p)))
        .collect();
    for v in children(Path::new(MACOS_RUBY_FRAMEWORK)) {
        out.extend(children(&v.join("usr/lib/ruby/gems")));
    }
    // Fedora and RHEL
    out.push(PathBuf::from("/usr/share/gems"));
    if cfg!(windows) {
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string());
        out.extend(ruby_installer_gem_dirs(&PathBuf::from(format!(
            "{drive}\\"
        ))));
    }
    out.retain(|d| is_gem_dir(d));
    // macOS reaches one gem directory through several paths (`Versions/Current`, `/usr/lib/ruby`); on Windows, canonical paths are verbatim (`\\?\C:\...`), so are not used
    let mut out: Vec<PathBuf> = if cfg!(windows) {
        out
    } else {
        out.into_iter()
            .map(|d| fs::canonicalize(&d).unwrap_or(d))
            .collect()
    };
    out.sort();
    out.dedup();
    out
}

fn origin(dir: PathBuf) -> Origin {
    Origin::new(Ecosystem::Ruby, PathShared::from(dir))
}

fn find_lockfile(dir: &Path) -> Option<PathBuf> {
    LOCKFILES.iter().map(|n| dir.join(n)).find(|p| p.is_file())
}

//------------------------------------------------------------------------------
pub(crate) struct RubyScanner;

impl EcosystemScanner for RubyScanner {
    fn default_origins(&self) -> Vec<Origin> {
        system_gem_dirs().into_iter().map(origin).collect()
    }

    fn detect(&self, dir: &Path, items: &[DirItem]) -> Detect {
        let has_dir = |name: &str| {
            items
                .iter()
                .any(|i| i.file_type.is_dir() && i.file_name() == Some(name))
        };
        // a gem directory: the installed gems' sources hold lockfiles of their own, which are not projects
        if has_dir(SPECIFICATIONS) && has_dir(GEMS) {
            return Detect {
                origins: vec![origin(dir.to_path_buf())],
                prune: true,
                ..Default::default()
            };
        }
        let has_lock = items
            .iter()
            .any(|i| i.file_name().is_some_and(|n| LOCKFILES.contains(&n)));
        Detect {
            origins: if has_lock {
                vec![origin(dir.to_path_buf())]
            } else {
                Vec::new()
            },
            ..Default::default()
        }
    }

    /// Accept a lockfile or Gemfile, a directory with a lockfile, or a gem directory.
    fn accepts(&self, path: &Path) -> bool {
        let Ok(p) = path_normalize(path, false) else {
            return false;
        };
        if p.is_file() {
            return p
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| LOCKFILES.contains(&n) || MANIFESTS.contains(&n));
        }
        p.is_dir() && (find_lockfile(&p).is_some() || is_gem_dir(&p))
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
        if is_gem_dir(&dir) || find_lockfile(&dir).is_some() {
            Ok(origin(dir))
        } else {
            Err(format!(
                "No Gemfile.lock found in {dir:?}, and it is not a gem directory; run `bundle lock`"
            )
            .into())
        }
    }

    fn origin_to_sites(
        &self,
        origin: &Origin,
        _config: &ScanConfig,
        _log: FlagLog,
    ) -> Vec<Site> {
        let dir = origin.as_path();
        if is_gem_dir(dir) {
            return vec![Site::new(
                Ecosystem::Ruby,
                SiteKind::Installed,
                PathShared::from(dir.to_path_buf()),
            )];
        }
        find_lockfile(dir)
            .map(|fp| {
                Site::new(Ecosystem::Ruby, SiteKind::Lockfile, PathShared::from(fp))
            })
            .into_iter()
            .collect()
    }

    fn site_to_packages(&self, site: &Site) -> Vec<Package> {
        match site.kind {
            SiteKind::Installed => {
                let unique: HashSet<Package> = installed_gems(site.as_path())
                    .into_iter()
                    .map(|(p, ..)| p)
                    .collect();
                unique.into_iter().collect()
            }
            _ => fs::read_to_string(site.as_path())
                .map(|c| packages_from_lockfile(&c))
                .unwrap_or_default(),
        }
    }

    /// Installed gems with native extensions (whose build scripts run at install; not default gems, which are built with Ruby), gems that ship a RubyGems plugin (`lib/rubygems_plugin.rb`, loaded by `gem` commands), and plugins installed in the gem directory's `plugins`.
    fn site_to_hooks(&self, site: &Site) -> Vec<Hook> {
        if site.kind != SiteKind::Installed {
            return Vec::new();
        }
        let dir = site.as_path();
        let mut hooks = Vec::new();
        for (p, spec, spec_fp, full) in installed_gems(dir) {
            // default gems are built with Ruby itself; their extensions do not run at install
            let is_default = spec_fp
                .parent()
                .is_some_and(|d| d.file_name().is_some_and(|n| n == "default"));
            if !spec.extensions.is_empty() && !is_default {
                hooks.push(Hook {
                    name: format!("{p} extensions"),
                    detail: spec.extensions.join(", "),
                });
            }
            let plugin = dir.join(GEMS).join(&full).join("lib/rubygems_plugin.rb");
            if plugin.is_file() {
                if let Ok(detail) = head_lines(&plugin, HOOK_HEAD_LINES) {
                    hooks.push(Hook {
                        name: format!("{p} rubygems_plugin.rb"),
                        detail,
                    });
                }
            }
        }
        for e in fs::read_dir(dir.join("plugins"))
            .into_iter()
            .flatten()
            .flatten()
        {
            let fp = e.path();
            if fp.extension().is_some_and(|x| x == "rb") {
                if let (Some(name), Ok(detail)) = (
                    fp.file_name().and_then(|n| n.to_str()),
                    head_lines(&fp, HOOK_HEAD_LINES),
                ) {
                    hooks.push(Hook {
                        name: format!("plugins/{name}"),
                        detail,
                    });
                }
            }
        }
        hooks.sort();
        hooks
    }

    /// For a gem directory, each gem's directories (`gems/<full name>` and its built `extensions`) with their files, plus its spec and cached `.gem` file. A gem installed for several platforms has all of them.
    fn site_to_artifacts(
        &self,
        site: &Site,
        wanted: &HashSet<&Package>,
    ) -> Vec<(Package, Artifacts)> {
        if site.kind != SiteKind::Installed {
            return Vec::new();
        }
        let dir = site.as_path();
        // extensions/<platform>/<abi>/<full name>
        let ext_dirs: Vec<PathBuf> = fs::read_dir(dir.join("extensions"))
            .into_iter()
            .flatten()
            .flatten()
            .flat_map(|plat| fs::read_dir(plat.path()).into_iter().flatten().flatten())
            .map(|abi| abi.path())
            .collect();
        let mut found: HashMap<Package, (Vec<PathBuf>, Vec<PathBuf>)> = HashMap::new();
        for (p, _, spec_fp, full) in installed_gems(dir) {
            if !wanted.contains(&p) {
                continue;
            }
            let (dirs, files) = found.entry(p).or_default();
            let gem_dir = dir.join(GEMS).join(&full);
            if gem_dir.is_dir() {
                dirs.push(gem_dir);
            }
            dirs.extend(
                ext_dirs
                    .iter()
                    .map(|a| a.join(&full))
                    .filter(|d| d.is_dir()),
            );
            files.push(spec_fp);
            let cached = dir.join("cache").join(format!("{full}.gem"));
            if cached.is_file() {
                files.push(cached);
            }
        }
        found
            .into_iter()
            .map(|(p, (mut dirs, files))| {
                dirs.sort();
                let mut a = Artifacts::from_dirs(dirs, &[]);
                a.files.extend(files.into_iter().map(|f| (f, true)));
                a.files.sort();
                (p, a)
            })
            .collect()
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// `(a, b, a <=> b)`, generated with Ruby 2.6.10 (RubyGems 3.0.3.1) `Gem::Version`, with `<=>` replaced by RubyGems master's (which compares canonical segments; 3.0.3 differs for `1.0.a` vs `1.a.2`).
    const CMP: &[(&str, &str, i32)] = &[
        ("1", "1.0", 0),
        ("1", "1.0.0", 0),
        ("1", "1.0.1", -1),
        ("1", "1.0.a", 1),
        ("1", "1.0.0.rc1", 1),
        ("1", "1.0.0.pre", 1),
        ("1", "1.0.0.pre.1", 1),
        ("1", "1.0.0.beta2", 1),
        ("1", "1.0.0.beta10", 1),
        ("1", "1.1", -1),
        ("1", "1.10", -1),
        ("1", "1.9", -1),
        ("1", "2.0.0.rc1", -1),
        ("1", "2.0.0", -1),
        ("1", "1.0.0-rc1", 1),
        ("1", "7.1.3.4", -1),
        ("1", "0.9.12", 1),
        ("1", "1.a.2", 1),
        ("1", "1.0.b1", 1),
        ("1.0", "1.0.0", 0),
        ("1.0", "1.0.1", -1),
        ("1.0", "1.0.a", 1),
        ("1.0", "1.0.0.rc1", 1),
        ("1.0", "1.0.0.pre", 1),
        ("1.0", "1.0.0.pre.1", 1),
        ("1.0", "1.0.0.beta2", 1),
        ("1.0", "1.0.0.beta10", 1),
        ("1.0", "1.1", -1),
        ("1.0", "1.10", -1),
        ("1.0", "1.9", -1),
        ("1.0", "2.0.0.rc1", -1),
        ("1.0", "2.0.0", -1),
        ("1.0", "1.0.0-rc1", 1),
        ("1.0", "7.1.3.4", -1),
        ("1.0", "0.9.12", 1),
        ("1.0", "1.a.2", 1),
        ("1.0", "1.0.b1", 1),
        ("1.0.0", "1.0.1", -1),
        ("1.0.0", "1.0.a", 1),
        ("1.0.0", "1.0.0.rc1", 1),
        ("1.0.0", "1.0.0.pre", 1),
        ("1.0.0", "1.0.0.pre.1", 1),
        ("1.0.0", "1.0.0.beta2", 1),
        ("1.0.0", "1.0.0.beta10", 1),
        ("1.0.0", "1.1", -1),
        ("1.0.0", "1.10", -1),
        ("1.0.0", "1.9", -1),
        ("1.0.0", "2.0.0.rc1", -1),
        ("1.0.0", "2.0.0", -1),
        ("1.0.0", "1.0.0-rc1", 1),
        ("1.0.0", "7.1.3.4", -1),
        ("1.0.0", "0.9.12", 1),
        ("1.0.0", "1.a.2", 1),
        ("1.0.0", "1.0.b1", 1),
        ("1.0.1", "1.0.a", 1),
        ("1.0.1", "1.0.0.rc1", 1),
        ("1.0.1", "1.0.0.pre", 1),
        ("1.0.1", "1.0.0.pre.1", 1),
        ("1.0.1", "1.0.0.beta2", 1),
        ("1.0.1", "1.0.0.beta10", 1),
        ("1.0.1", "1.1", -1),
        ("1.0.1", "1.10", -1),
        ("1.0.1", "1.9", -1),
        ("1.0.1", "2.0.0.rc1", -1),
        ("1.0.1", "2.0.0", -1),
        ("1.0.1", "1.0.0-rc1", 1),
        ("1.0.1", "7.1.3.4", -1),
        ("1.0.1", "0.9.12", 1),
        ("1.0.1", "1.a.2", 1),
        ("1.0.1", "1.0.b1", 1),
        ("1.0.a", "1.0.0.rc1", -1),
        ("1.0.a", "1.0.0.pre", -1),
        ("1.0.a", "1.0.0.pre.1", -1),
        ("1.0.a", "1.0.0.beta2", -1),
        ("1.0.a", "1.0.0.beta10", -1),
        ("1.0.a", "1.1", -1),
        ("1.0.a", "1.10", -1),
        ("1.0.a", "1.9", -1),
        ("1.0.a", "2.0.0.rc1", -1),
        ("1.0.a", "2.0.0", -1),
        ("1.0.a", "1.0.0-rc1", -1),
        ("1.0.a", "7.1.3.4", -1),
        ("1.0.a", "0.9.12", 1),
        ("1.0.a", "1.a.2", -1),
        ("1.0.a", "1.0.b1", -1),
        ("1.0.0.rc1", "1.0.0.pre", 1),
        ("1.0.0.rc1", "1.0.0.pre.1", 1),
        ("1.0.0.rc1", "1.0.0.beta2", 1),
        ("1.0.0.rc1", "1.0.0.beta10", 1),
        ("1.0.0.rc1", "1.1", -1),
        ("1.0.0.rc1", "1.10", -1),
        ("1.0.0.rc1", "1.9", -1),
        ("1.0.0.rc1", "2.0.0.rc1", -1),
        ("1.0.0.rc1", "2.0.0", -1),
        ("1.0.0.rc1", "1.0.0-rc1", 1),
        ("1.0.0.rc1", "7.1.3.4", -1),
        ("1.0.0.rc1", "0.9.12", 1),
        ("1.0.0.rc1", "1.a.2", 1),
        ("1.0.0.rc1", "1.0.b1", 1),
        ("1.0.0.pre", "1.0.0.pre.1", -1),
        ("1.0.0.pre", "1.0.0.beta2", 1),
        ("1.0.0.pre", "1.0.0.beta10", 1),
        ("1.0.0.pre", "1.1", -1),
        ("1.0.0.pre", "1.10", -1),
        ("1.0.0.pre", "1.9", -1),
        ("1.0.0.pre", "2.0.0.rc1", -1),
        ("1.0.0.pre", "2.0.0", -1),
        ("1.0.0.pre", "1.0.0-rc1", 1),
        ("1.0.0.pre", "7.1.3.4", -1),
        ("1.0.0.pre", "0.9.12", 1),
        ("1.0.0.pre", "1.a.2", 1),
        ("1.0.0.pre", "1.0.b1", 1),
        ("1.0.0.pre.1", "1.0.0.beta2", 1),
        ("1.0.0.pre.1", "1.0.0.beta10", 1),
        ("1.0.0.pre.1", "1.1", -1),
        ("1.0.0.pre.1", "1.10", -1),
        ("1.0.0.pre.1", "1.9", -1),
        ("1.0.0.pre.1", "2.0.0.rc1", -1),
        ("1.0.0.pre.1", "2.0.0", -1),
        ("1.0.0.pre.1", "1.0.0-rc1", 1),
        ("1.0.0.pre.1", "7.1.3.4", -1),
        ("1.0.0.pre.1", "0.9.12", 1),
        ("1.0.0.pre.1", "1.a.2", 1),
        ("1.0.0.pre.1", "1.0.b1", 1),
        ("1.0.0.beta2", "1.0.0.beta10", -1),
        ("1.0.0.beta2", "1.1", -1),
        ("1.0.0.beta2", "1.10", -1),
        ("1.0.0.beta2", "1.9", -1),
        ("1.0.0.beta2", "2.0.0.rc1", -1),
        ("1.0.0.beta2", "2.0.0", -1),
        ("1.0.0.beta2", "1.0.0-rc1", -1),
        ("1.0.0.beta2", "7.1.3.4", -1),
        ("1.0.0.beta2", "0.9.12", 1),
        ("1.0.0.beta2", "1.a.2", 1),
        ("1.0.0.beta2", "1.0.b1", 1),
        ("1.0.0.beta10", "1.1", -1),
        ("1.0.0.beta10", "1.10", -1),
        ("1.0.0.beta10", "1.9", -1),
        ("1.0.0.beta10", "2.0.0.rc1", -1),
        ("1.0.0.beta10", "2.0.0", -1),
        ("1.0.0.beta10", "1.0.0-rc1", -1),
        ("1.0.0.beta10", "7.1.3.4", -1),
        ("1.0.0.beta10", "0.9.12", 1),
        ("1.0.0.beta10", "1.a.2", 1),
        ("1.0.0.beta10", "1.0.b1", 1),
        ("1.1", "1.10", -1),
        ("1.1", "1.9", -1),
        ("1.1", "2.0.0.rc1", -1),
        ("1.1", "2.0.0", -1),
        ("1.1", "1.0.0-rc1", 1),
        ("1.1", "7.1.3.4", -1),
        ("1.1", "0.9.12", 1),
        ("1.1", "1.a.2", 1),
        ("1.1", "1.0.b1", 1),
        ("1.10", "1.9", 1),
        ("1.10", "2.0.0.rc1", -1),
        ("1.10", "2.0.0", -1),
        ("1.10", "1.0.0-rc1", 1),
        ("1.10", "7.1.3.4", -1),
        ("1.10", "0.9.12", 1),
        ("1.10", "1.a.2", 1),
        ("1.10", "1.0.b1", 1),
        ("1.9", "2.0.0.rc1", -1),
        ("1.9", "2.0.0", -1),
        ("1.9", "1.0.0-rc1", 1),
        ("1.9", "7.1.3.4", -1),
        ("1.9", "0.9.12", 1),
        ("1.9", "1.a.2", 1),
        ("1.9", "1.0.b1", 1),
        ("2.0.0.rc1", "2.0.0", -1),
        ("2.0.0.rc1", "1.0.0-rc1", 1),
        ("2.0.0.rc1", "7.1.3.4", -1),
        ("2.0.0.rc1", "0.9.12", 1),
        ("2.0.0.rc1", "1.a.2", 1),
        ("2.0.0.rc1", "1.0.b1", 1),
        ("2.0.0", "1.0.0-rc1", 1),
        ("2.0.0", "7.1.3.4", -1),
        ("2.0.0", "0.9.12", 1),
        ("2.0.0", "1.a.2", 1),
        ("2.0.0", "1.0.b1", 1),
        ("1.0.0-rc1", "7.1.3.4", -1),
        ("1.0.0-rc1", "0.9.12", 1),
        ("1.0.0-rc1", "1.a.2", 1),
        ("1.0.0-rc1", "1.0.b1", 1),
        ("7.1.3.4", "0.9.12", 1),
        ("7.1.3.4", "1.a.2", 1),
        ("7.1.3.4", "1.0.b1", 1),
        ("0.9.12", "1.a.2", -1),
        ("0.9.12", "1.0.b1", -1),
        ("1.a.2", "1.0.b1", -1),
    ];

    /// `(requirement, version, satisfied)`, generated likewise with `Gem::Requirement#satisfied_by?` (comma-separated parts passed as separate requirements).
    const REQ: &[(&str, &str, bool)] = &[
        ("~> 1.0", "1", true),
        ("~> 1.0", "1.0", true),
        ("~> 1.0", "1.0.0", true),
        ("~> 1.0", "1.0.1", true),
        ("~> 1.0", "1.0.a", false),
        ("~> 1.0", "1.0.0.rc1", false),
        ("~> 1.0", "1.0.0.pre", false),
        ("~> 1.0", "1.0.0.pre.1", false),
        ("~> 1.0", "1.0.0.beta2", false),
        ("~> 1.0", "1.0.0.beta10", false),
        ("~> 1.0", "1.1", true),
        ("~> 1.0", "1.10", true),
        ("~> 1.0", "1.9", true),
        ("~> 1.0", "2.0.0.rc1", false),
        ("~> 1.0", "2.0.0", false),
        ("~> 1.0", "1.0.0-rc1", false),
        ("~> 1.0", "7.1.3.4", false),
        ("~> 1.0", "0.9.12", false),
        ("~> 1.0", "1.a.2", false),
        ("~> 1.0", "1.0.b1", false),
        ("~> 1.0.0", "1", true),
        ("~> 1.0.0", "1.0", true),
        ("~> 1.0.0", "1.0.0", true),
        ("~> 1.0.0", "1.0.1", true),
        ("~> 1.0.0", "1.0.a", false),
        ("~> 1.0.0", "1.0.0.rc1", false),
        ("~> 1.0.0", "1.0.0.pre", false),
        ("~> 1.0.0", "1.0.0.pre.1", false),
        ("~> 1.0.0", "1.0.0.beta2", false),
        ("~> 1.0.0", "1.0.0.beta10", false),
        ("~> 1.0.0", "1.1", false),
        ("~> 1.0.0", "1.10", false),
        ("~> 1.0.0", "1.9", false),
        ("~> 1.0.0", "2.0.0.rc1", false),
        ("~> 1.0.0", "2.0.0", false),
        ("~> 1.0.0", "1.0.0-rc1", false),
        ("~> 1.0.0", "7.1.3.4", false),
        ("~> 1.0.0", "0.9.12", false),
        ("~> 1.0.0", "1.a.2", false),
        ("~> 1.0.0", "1.0.b1", false),
        ("~> 1", "1", true),
        ("~> 1", "1.0", true),
        ("~> 1", "1.0.0", true),
        ("~> 1", "1.0.1", true),
        ("~> 1", "1.0.a", false),
        ("~> 1", "1.0.0.rc1", false),
        ("~> 1", "1.0.0.pre", false),
        ("~> 1", "1.0.0.pre.1", false),
        ("~> 1", "1.0.0.beta2", false),
        ("~> 1", "1.0.0.beta10", false),
        ("~> 1", "1.1", true),
        ("~> 1", "1.10", true),
        ("~> 1", "1.9", true),
        ("~> 1", "2.0.0.rc1", false),
        ("~> 1", "2.0.0", false),
        ("~> 1", "1.0.0-rc1", false),
        ("~> 1", "7.1.3.4", false),
        ("~> 1", "0.9.12", false),
        ("~> 1", "1.a.2", false),
        ("~> 1", "1.0.b1", false),
        (">= 1.0, < 2", "1", true),
        (">= 1.0, < 2", "1.0", true),
        (">= 1.0, < 2", "1.0.0", true),
        (">= 1.0, < 2", "1.0.1", true),
        (">= 1.0, < 2", "1.0.a", false),
        (">= 1.0, < 2", "1.0.0.rc1", false),
        (">= 1.0, < 2", "1.0.0.pre", false),
        (">= 1.0, < 2", "1.0.0.pre.1", false),
        (">= 1.0, < 2", "1.0.0.beta2", false),
        (">= 1.0, < 2", "1.0.0.beta10", false),
        (">= 1.0, < 2", "1.1", true),
        (">= 1.0, < 2", "1.10", true),
        (">= 1.0, < 2", "1.9", true),
        (">= 1.0, < 2", "2.0.0.rc1", true),
        (">= 1.0, < 2", "2.0.0", false),
        (">= 1.0, < 2", "1.0.0-rc1", false),
        (">= 1.0, < 2", "7.1.3.4", false),
        (">= 1.0, < 2", "0.9.12", false),
        (">= 1.0, < 2", "1.a.2", false),
        (">= 1.0, < 2", "1.0.b1", false),
        ("!= 1.0.1", "1", true),
        ("!= 1.0.1", "1.0", true),
        ("!= 1.0.1", "1.0.0", true),
        ("!= 1.0.1", "1.0.1", false),
        ("!= 1.0.1", "1.0.a", true),
        ("!= 1.0.1", "1.0.0.rc1", true),
        ("!= 1.0.1", "1.0.0.pre", true),
        ("!= 1.0.1", "1.0.0.pre.1", true),
        ("!= 1.0.1", "1.0.0.beta2", true),
        ("!= 1.0.1", "1.0.0.beta10", true),
        ("!= 1.0.1", "1.1", true),
        ("!= 1.0.1", "1.10", true),
        ("!= 1.0.1", "1.9", true),
        ("!= 1.0.1", "2.0.0.rc1", true),
        ("!= 1.0.1", "2.0.0", true),
        ("!= 1.0.1", "1.0.0-rc1", true),
        ("!= 1.0.1", "7.1.3.4", true),
        ("!= 1.0.1", "0.9.12", true),
        ("!= 1.0.1", "1.a.2", true),
        ("!= 1.0.1", "1.0.b1", true),
        ("= 1.0", "1", true),
        ("= 1.0", "1.0", true),
        ("= 1.0", "1.0.0", true),
        ("= 1.0", "1.0.1", false),
        ("= 1.0", "1.0.a", false),
        ("= 1.0", "1.0.0.rc1", false),
        ("= 1.0", "1.0.0.pre", false),
        ("= 1.0", "1.0.0.pre.1", false),
        ("= 1.0", "1.0.0.beta2", false),
        ("= 1.0", "1.0.0.beta10", false),
        ("= 1.0", "1.1", false),
        ("= 1.0", "1.10", false),
        ("= 1.0", "1.9", false),
        ("= 1.0", "2.0.0.rc1", false),
        ("= 1.0", "2.0.0", false),
        ("= 1.0", "1.0.0-rc1", false),
        ("= 1.0", "7.1.3.4", false),
        ("= 1.0", "0.9.12", false),
        ("= 1.0", "1.a.2", false),
        ("= 1.0", "1.0.b1", false),
        ("1.0", "1", true),
        ("1.0", "1.0", true),
        ("1.0", "1.0.0", true),
        ("1.0", "1.0.1", false),
        ("1.0", "1.0.a", false),
        ("1.0", "1.0.0.rc1", false),
        ("1.0", "1.0.0.pre", false),
        ("1.0", "1.0.0.pre.1", false),
        ("1.0", "1.0.0.beta2", false),
        ("1.0", "1.0.0.beta10", false),
        ("1.0", "1.1", false),
        ("1.0", "1.10", false),
        ("1.0", "1.9", false),
        ("1.0", "2.0.0.rc1", false),
        ("1.0", "2.0.0", false),
        ("1.0", "1.0.0-rc1", false),
        ("1.0", "7.1.3.4", false),
        ("1.0", "0.9.12", false),
        ("1.0", "1.a.2", false),
        ("1.0", "1.0.b1", false),
        ("> 1.0.a", "1", true),
        ("> 1.0.a", "1.0", true),
        ("> 1.0.a", "1.0.0", true),
        ("> 1.0.a", "1.0.1", true),
        ("> 1.0.a", "1.0.a", false),
        ("> 1.0.a", "1.0.0.rc1", true),
        ("> 1.0.a", "1.0.0.pre", true),
        ("> 1.0.a", "1.0.0.pre.1", true),
        ("> 1.0.a", "1.0.0.beta2", true),
        ("> 1.0.a", "1.0.0.beta10", true),
        ("> 1.0.a", "1.1", true),
        ("> 1.0.a", "1.10", true),
        ("> 1.0.a", "1.9", true),
        ("> 1.0.a", "2.0.0.rc1", true),
        ("> 1.0.a", "2.0.0", true),
        ("> 1.0.a", "1.0.0-rc1", true),
        ("> 1.0.a", "7.1.3.4", true),
        ("> 1.0.a", "0.9.12", false),
        ("> 1.0.a", "1.a.2", true),
        ("> 1.0.a", "1.0.b1", true),
        ("~> 2.0.0.rc1", "1", false),
        ("~> 2.0.0.rc1", "1.0", false),
        ("~> 2.0.0.rc1", "1.0.0", false),
        ("~> 2.0.0.rc1", "1.0.1", false),
        ("~> 2.0.0.rc1", "1.0.a", false),
        ("~> 2.0.0.rc1", "1.0.0.rc1", false),
        ("~> 2.0.0.rc1", "1.0.0.pre", false),
        ("~> 2.0.0.rc1", "1.0.0.pre.1", false),
        ("~> 2.0.0.rc1", "1.0.0.beta2", false),
        ("~> 2.0.0.rc1", "1.0.0.beta10", false),
        ("~> 2.0.0.rc1", "1.1", false),
        ("~> 2.0.0.rc1", "1.10", false),
        ("~> 2.0.0.rc1", "1.9", false),
        ("~> 2.0.0.rc1", "2.0.0.rc1", true),
        ("~> 2.0.0.rc1", "2.0.0", true),
        ("~> 2.0.0.rc1", "1.0.0-rc1", false),
        ("~> 2.0.0.rc1", "7.1.3.4", false),
        ("~> 2.0.0.rc1", "0.9.12", false),
        ("~> 2.0.0.rc1", "1.a.2", false),
        ("~> 2.0.0.rc1", "1.0.b1", false),
        ("<= 1.9", "1", true),
        ("<= 1.9", "1.0", true),
        ("<= 1.9", "1.0.0", true),
        ("<= 1.9", "1.0.1", true),
        ("<= 1.9", "1.0.a", true),
        ("<= 1.9", "1.0.0.rc1", true),
        ("<= 1.9", "1.0.0.pre", true),
        ("<= 1.9", "1.0.0.pre.1", true),
        ("<= 1.9", "1.0.0.beta2", true),
        ("<= 1.9", "1.0.0.beta10", true),
        ("<= 1.9", "1.1", true),
        ("<= 1.9", "1.10", false),
        ("<= 1.9", "1.9", true),
        ("<= 1.9", "2.0.0.rc1", false),
        ("<= 1.9", "2.0.0", false),
        ("<= 1.9", "1.0.0-rc1", true),
        ("<= 1.9", "7.1.3.4", false),
        ("<= 1.9", "0.9.12", true),
        ("<= 1.9", "1.a.2", true),
        ("<= 1.9", "1.0.b1", true),
        ("~> 1.0.0.pre", "1", true),
        ("~> 1.0.0.pre", "1.0", true),
        ("~> 1.0.0.pre", "1.0.0", true),
        ("~> 1.0.0.pre", "1.0.1", true),
        ("~> 1.0.0.pre", "1.0.a", false),
        ("~> 1.0.0.pre", "1.0.0.rc1", true),
        ("~> 1.0.0.pre", "1.0.0.pre", true),
        ("~> 1.0.0.pre", "1.0.0.pre.1", true),
        ("~> 1.0.0.pre", "1.0.0.beta2", false),
        ("~> 1.0.0.pre", "1.0.0.beta10", false),
        ("~> 1.0.0.pre", "1.1", false),
        ("~> 1.0.0.pre", "1.10", false),
        ("~> 1.0.0.pre", "1.9", false),
        ("~> 1.0.0.pre", "2.0.0.rc1", false),
        ("~> 1.0.0.pre", "2.0.0", false),
        ("~> 1.0.0.pre", "1.0.0-rc1", false),
        ("~> 1.0.0.pre", "7.1.3.4", false),
        ("~> 1.0.0.pre", "0.9.12", false),
        ("~> 1.0.0.pre", "1.a.2", false),
        ("~> 1.0.0.pre", "1.0.b1", false),
    ];

    #[test]
    fn test_gem_version_cmp_vectors_a() {
        for (a, b, expected) in CMP {
            let (va, vb) = (GemVersion::parse(a).unwrap(), GemVersion::parse(b).unwrap());
            let got = match va.cmp(&vb) {
                Ordering::Less => -1,
                Ordering::Equal => 0,
                Ordering::Greater => 1,
            };
            assert_eq!(got, *expected, "{a} <=> {b}");
        }
    }

    #[test]
    fn test_gem_requirement_vectors_a() {
        for (r, v, expected) in REQ {
            let req = GemRequirement::parse(r).unwrap();
            assert_eq!(
                req.matches(&GemVersion::parse(v).unwrap()),
                *expected,
                "{r} satisfied by {v}"
            );
        }
    }

    #[test]
    fn test_gem_version_parse_a() {
        assert!(GemVersion::parse("").is_none());
        assert!(GemVersion::parse("abc").is_none());
        assert!(GemVersion::parse("1.0 beta").is_none());
        assert_eq!(
            GemVersion::parse(" 7.1.3.4 ").unwrap().to_string(),
            "7.1.3.4"
        );
        assert!(GemRequirement::parse("~> x").is_err());
        assert_eq!(
            GemRequirement::parse("= 1.2")
                .unwrap()
                .exact()
                .unwrap()
                .to_string(),
            "1.2"
        );
        assert!(GemRequirement::parse(">= 1.2").unwrap().exact().is_none());
    }

    const LOCK: &str = "GIT
  remote: https://github.com/mastodon/webpush.git
  revision: 9631ac63045cfabddacc69fc06e919b4c13eb913
  ref: 9631ac63045cfabddacc69fc06e919b4c13eb913
  specs:
    webpush (1.1.0)
      hkdf (~> 0.2)
      jwt (~> 2.0)

PATH
  remote: .
  specs:
    actioncable (8.2.0.alpha)
      actionpack (= 8.2.0.alpha)

GEM
  remote: https://rubygems.org/
  specs:
    ffi (1.17.1)
    ffi (1.17.1-arm64-darwin)
    ffi (1.17.1-x86_64-linux-gnu)
    nokogiri (1.18.3-arm64-darwin)
      racc (~> 1.4)
    racc (1.8.1)

GEM
  remote: https://gems.example.com/
  specs:
    private-gem (0.1.0)

PLATFORMS
  arm64-darwin
  ruby

DEPENDENCIES
  ffi
  webpush!

CHECKSUMS
  ffi (1.17.1) sha256=abc

RUBY VERSION
  ruby 3.4.1

BUNDLED WITH
  2.6.2
";

    fn names(packages: &[Package]) -> Vec<String> {
        let mut out: Vec<String> = packages.iter().map(|p| p.to_string()).collect();
        out.sort();
        out
    }

    #[test]
    fn test_packages_from_lockfile_plugin_source_a() {
        // format as written by Bundler's plugin source API (Bundler::Plugin::API::Source#to_lock)
        let lock = "PLUGIN SOURCE
  remote: s3://gems.example.com/private
  type: s3
  specs:
    internal-gem (2.1.0)
      rack (>= 2.0)

GEM
  remote: https://rubygems.org/
  specs:
    rack (3.1.8)

PLATFORMS
  ruby

DEPENDENCIES
  internal-gem!
  rack
";
        let packages = packages_from_lockfile(lock);
        assert_eq!(names(&packages), vec!["internal-gem-2.1.0", "rack-3.1.8"]);
        let internal = packages.iter().find(|p| p.name == "internal-gem").unwrap();
        assert_eq!(
            internal.direct_url.as_ref().unwrap().url,
            "s3://gems.example.com/private"
        );
    }

    #[test]
    fn test_ruby_installer_gem_dirs_a() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        for d in [
            "Ruby33-x64/lib/ruby/gems/3.3.0/specifications",
            "Ruby33-x64/lib/ruby/gems/3.3.0/gems",
            "Ruby27/lib/ruby/gems/2.7.0/specifications",
            "Program Files/lib/ruby/gems/9.9.9",
        ] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        let mut found = ruby_installer_gem_dirs(root);
        found.sort();
        assert_eq!(
            found,
            vec![
                root.join("Ruby27/lib/ruby/gems/2.7.0"),
                root.join("Ruby33-x64/lib/ruby/gems/3.3.0"),
            ]
        );
        // only complete gem directories become origins
        assert_eq!(found.iter().filter(|d| is_gem_dir(d)).count(), 1);
    }

    #[test]
    fn test_packages_from_lockfile_a() {
        let packages = packages_from_lockfile(LOCK);
        assert_eq!(
            names(&packages),
            vec![
                "ffi-1.17.1",
                "nokogiri-1.18.3",
                "private-gem-0.1.0",
                "racc-1.8.1",
                "webpush-1.1.0"
            ]
        );
        let get = |n: &str| packages.iter().find(|p| p.name == n).unwrap();
        assert!(get("ffi").direct_url.is_none());
        let git = get("webpush").direct_url.as_ref().unwrap();
        assert_eq!(git.url, "https://github.com/mastodon/webpush.git");
        assert_eq!(
            git.vcs_info.as_ref().unwrap().commit_id,
            "9631ac63045cfabddacc69fc06e919b4c13eb913"
        );
        assert_eq!(
            get("private-gem").direct_url.as_ref().unwrap().url,
            "https://gems.example.com/"
        );
    }

    const GEMSPEC: &str = r#"# -*- encoding: utf-8 -*-
# stub: nokogiri 1.13.8 x86_64-darwin lib
# stub: ext/nokogiri/extconf.rb

Gem::Specification.new do |s|
  s.name = "nokogiri".freeze
  s.version = "1.13.8"
  s.extensions = ["ext/nokogiri/extconf.rb".freeze]
end
"#;

    #[test]
    fn test_parse_gemspec_a() {
        assert_eq!(
            parse_gemspec(GEMSPEC).unwrap(),
            GemSpec {
                name: "nokogiri".to_string(),
                version: "1.13.8".to_string(),
                extensions: vec!["ext/nokogiri/extconf.rb".to_string()],
            }
        );
        // without a stub line
        let s = parse_gemspec(
            "  s.name = \"CFPropertyList\".freeze\n  s.version = \"2.3.6\"\n",
        )
        .unwrap();
        assert_eq!(
            (s.name.as_str(), s.version.as_str()),
            ("CFPropertyList", "2.3.6")
        );
        assert!(parse_gemspec("nothing here").is_none());
    }

    /// A gem directory with nokogiri (native extension, built, cached) and a default gem with a RubyGems plugin.
    fn make_gem_dir(root: &Path) -> PathBuf {
        let d = root.join("gems").join("3.3.0");
        let w = |rel: &str, content: &str| {
            let fp = d.join(rel);
            fs::create_dir_all(fp.parent().unwrap()).unwrap();
            fs::write(fp, content).unwrap();
        };
        w(
            "specifications/nokogiri-1.13.8-x86_64-darwin.gemspec",
            GEMSPEC,
        );
        w("gems/nokogiri-1.13.8-x86_64-darwin/lib/nokogiri.rb", "");
        w("gems/nokogiri-1.13.8-x86_64-darwin/README.md", "");
        w(
            "extensions/x86_64-darwin/3.3.0/nokogiri-1.13.8-x86_64-darwin/gem.build_complete",
            "",
        );
        w("cache/nokogiri-1.13.8-x86_64-darwin.gem", "");
        w(
            "specifications/default/json-2.7.1.gemspec",
            "# stub: json 2.7.1 ruby lib\n# stub: ext/json/ext/generator/extconf.rb ext/json/ext/parser/extconf.rb\n",
        );
        w(
            "gems/json-2.7.1/lib/rubygems_plugin.rb",
            "Gem.post_install { }\n",
        );
        w("plugins/bundler_plugin.rb", "require 'x'\n");
        d
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
    fn test_gem_dir_a() {
        let dir = tempdir().unwrap();
        let g = make_gem_dir(dir.path());
        let d = RubyScanner.detect(&g, &items_of(&g));
        assert!(d.prune);
        assert_eq!(d.origins.len(), 1);
        assert!(RubyScanner.accepts(&g));
        let o = RubyScanner.normalize_origin(&g).unwrap();
        let sites = RubyScanner.origin_to_sites(
            &o,
            &ScanConfig::new(false, false),
            FlagLog(false),
        );
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].kind, SiteKind::Installed);
        let packages = RubyScanner.site_to_packages(&sites[0]);
        assert_eq!(names(&packages), vec!["json-2.7.1", "nokogiri-1.13.8"]);

        let hooks = RubyScanner.site_to_hooks(&sites[0]);
        let hook_names: Vec<&str> = hooks.iter().map(|h| h.name.as_str()).collect();
        // json is a default gem: its extensions are not reported
        assert_eq!(
            hook_names,
            vec![
                "json-2.7.1 rubygems_plugin.rb",
                "nokogiri-1.13.8 extensions",
                "plugins/bundler_plugin.rb"
            ]
        );
        assert_eq!(hooks[1].detail, "ext/nokogiri/extconf.rb");

        let wanted: HashSet<&Package> = packages.iter().collect();
        let mut arts = RubyScanner.site_to_artifacts(&sites[0], &wanted);
        arts.sort_by(|a, b| a.0.cmp(&b.0));
        let (p, a) = &arts[1];
        assert_eq!(p.name, "nokogiri");
        // gem dir and built extension dir; their 3 files plus the spec and cached gem
        assert_eq!(a.dirs.len(), 2);
        assert_eq!(a.files.len(), 5);
        // a default gem has its spec and its gem dir
        assert_eq!(arts[0].1.dirs.len(), 1);
    }

    #[test]
    fn test_project_a() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Gemfile.lock"), LOCK).unwrap();
        fs::write(
            dir.path().join("Gemfile"),
            "source 'https://rubygems.org'\n",
        )
        .unwrap();
        let d = RubyScanner.detect(dir.path(), &items_of(dir.path()));
        assert!(!d.prune);
        assert_eq!(d.origins.len(), 1);
        assert!(RubyScanner.accepts(&dir.path().join("Gemfile")));
        let o = RubyScanner
            .normalize_origin(&dir.path().join("Gemfile.lock"))
            .unwrap();
        let sites = RubyScanner.origin_to_sites(
            &o,
            &ScanConfig::new(false, false),
            FlagLog(false),
        );
        assert_eq!(sites[0].kind, SiteKind::Lockfile);
        assert_eq!(RubyScanner.site_to_packages(&sites[0]).len(), 5);
        assert!(RubyScanner.site_to_hooks(&sites[0]).is_empty());

        let empty = tempdir().unwrap();
        assert!(RubyScanner.normalize_origin(empty.path()).is_err());
    }
}
