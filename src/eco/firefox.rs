//! Firefox-family browser extensions scanner (Firefox, LibreWolf, Waterfox, Zen, Floorp, and Thunderbird, which shares the add-on system).
//!
//! An origin is a profile: a directory with `extensions.json` and `prefs.js`, found among the children of each browser's profile directories (`PROFILE_ROOTS`, per platform). The profile is the site (`SiteKind::Installed`). Its packages are the add-ons of type `extension` in `extensions.json`, by add-on id (an email-like id or a `{GUID}`), excluding those built into the browser (locations `app-builtin`, `app-builtin-addons`, `app-system-defaults`, `app-system-addons`). The granted permissions (`userPermissions`), install location, signing state and whether the add-on is active are read for `inspect`.
//!
//! See `browser` for what is and is not read from a profile.

use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use serde_json::Value as JsonValue;

use crate::eco::browser::join_rel;
use crate::eco::browser::DataDirs;
use crate::eco::browser::Grant;
use crate::eco::Artifacts;
use crate::eco::Detect;
use crate::eco::DirItem;
use crate::eco::EcosystemScanner;
use crate::eco::Hook;
use crate::ecosystem::Ecosystem;
use crate::origin_site::Origin;
use crate::origin_site::Site;
use crate::origin_site::SiteKind;
use crate::package::Package;
use crate::path_shared::PathShared;
use crate::util::path_normalize;
use crate::util::FlagLog;
use crate::util::ResultDynError;
use crate::util::ScanConfig;
use crate::version_spec::VersionSpec;

const EXTENSIONS_JSON: &str = "extensions.json";
const PREFS_JS: &str = "prefs.js";

/// Directories whose children are profiles, relative to the platform bases in `DataDirs`.
const MAC_PROFILE_ROOTS: [&str; 6] = [
    "Firefox/Profiles",
    "LibreWolf/Profiles",
    "Waterfox/Profiles",
    "zen/Profiles",
    "Floorp/Profiles",
    "Thunderbird/Profiles",
];
/// Relative to the home directory, on Linux (including Snap and Flatpak installs).
const LINUX_PROFILE_ROOTS: [&str; 9] = [
    ".mozilla/firefox",
    ".librewolf",
    ".waterfox",
    ".zen",
    ".floorp",
    ".thunderbird",
    "snap/firefox/common/.mozilla/firefox",
    ".var/app/org.mozilla.firefox/.mozilla/firefox",
    ".var/app/io.gitlab.librewolf-community/.librewolf",
];
const WIN_PROFILE_ROOTS: [&str; 6] = [
    "Mozilla/Firefox/Profiles",
    "LibreWolf/Profiles",
    "Waterfox/Profiles",
    "zen/Profiles",
    "Floorp/Profiles",
    "Thunderbird/Profiles",
];

/// Locations of add-ons built into the browser.
const BUILTIN_LOCATIONS: [&str; 4] = [
    "app-builtin",
    "app-builtin-addons",
    "app-system-defaults",
    "app-system-addons",
];

//------------------------------------------------------------------------------
fn profile_roots() -> Vec<PathBuf> {
    let d = DataDirs::current();
    let mut out = Vec::new();
    let mut add = |base: &Option<PathBuf>, rels: &[&str]| {
        if let Some(b) = base {
            out.extend(rels.iter().map(|r| join_rel(b, r)).filter(|p| p.is_dir()));
        }
    };
    add(&d.mac_support, &MAC_PROFILE_ROOTS);
    if d.xdg_config.is_some() {
        add(&d.home, &LINUX_PROFILE_ROOTS);
    }
    add(&d.win_roaming, &WIN_PROFILE_ROOTS);
    out
}

fn is_profile(dir: &Path) -> bool {
    dir.join(EXTENSIONS_JSON).is_file() && dir.join(PREFS_JS).is_file()
}

fn origin(dir: PathBuf) -> Origin {
    Origin::new(Ecosystem::Firefox, PathShared::from(dir))
}

fn child_profiles(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .filter(|p| is_profile(p))
        .collect();
    out.sort();
    out
}

//------------------------------------------------------------------------------
/// The non-built-in extensions of a profile's `extensions.json`, as JSON entries with an id and version.
fn addons(profile: &Path) -> Vec<JsonValue> {
    let Some(doc) = fs::read_to_string(profile.join(EXTENSIONS_JSON))
        .ok()
        .and_then(|c| serde_json::from_str::<JsonValue>(&c).ok())
    else {
        return Vec::new();
    };
    let Some(JsonValue::Array(all)) = doc.get("addons").cloned() else {
        return Vec::new();
    };
    all.into_iter()
        .filter(|a| {
            a.get("type").and_then(JsonValue::as_str) == Some("extension")
                && a.get("id").and_then(JsonValue::as_str).is_some()
                && a.get("version").and_then(JsonValue::as_str).is_some()
                && !a
                    .get("location")
                    .and_then(JsonValue::as_str)
                    .is_some_and(|l| BUILTIN_LOCATIONS.contains(&l))
        })
        .collect()
}

fn addon_package(a: &JsonValue) -> Option<Package> {
    Some(Package::new(
        Ecosystem::Firefox,
        a.get("id")?.as_str()?,
        VersionSpec::new(a.get("version")?.as_str()?),
        None,
    ))
}

fn strings(v: Option<&JsonValue>) -> Vec<String> {
    v.and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .filter_map(|x| x.as_str().map(str::to_string))
        .collect()
}

/// Permissions and state of an add-on, for `inspect`.
fn addon_grant(a: &JsonValue) -> Grant {
    let up = a.get("userPermissions");
    let location = a.get("location").and_then(JsonValue::as_str).unwrap_or("");
    let from_amo = a
        .get("sourceURI")
        .and_then(JsonValue::as_str)
        .is_some_and(|u| u.starts_with("https://addons.mozilla.org/"));
    let (source, source_flag) = match location {
        "app-profile" if from_amo => ("addons.mozilla.org".to_string(), false),
        "app-profile" => ("profile (not from addons.mozilla.org)".to_string(), false),
        "app-temporary" => ("temporary (about:debugging)".to_string(), true),
        l if l.starts_with("app-system-")
            || l.starts_with("winreg-")
            || l == "app-global" =>
        {
            (format!("sideloaded ({l})"), true)
        }
        l => (l.to_string(), false),
    };
    let mut notes = Vec::new();
    // signedState: 2 signed, 3 system, 4 privileged; 0 missing, -1 broken, -2 unknown
    let unsigned = a
        .get("signedState")
        .and_then(JsonValue::as_i64)
        .is_some_and(|s| s <= 0);
    if unsigned {
        notes.push("unsigned".to_string());
    }
    let active = a.get("active").and_then(JsonValue::as_bool).unwrap_or(true);
    let user_disabled = a
        .get("userDisabled")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);
    Grant {
        display: a
            .get("defaultLocale")
            .and_then(|l| l.get("name"))
            .and_then(JsonValue::as_str)
            .unwrap_or("")
            .to_string(),
        hosts: strings(up.and_then(|u| u.get("origins"))),
        apis: strings(up.and_then(|u| u.get("permissions"))),
        source,
        source_flag: source_flag || unsigned,
        disabled: !active || user_disabled,
        notes,
    }
}

//------------------------------------------------------------------------------
pub(crate) struct FirefoxScanner;

impl EcosystemScanner for FirefoxScanner {
    fn default_origins(&self) -> Vec<Origin> {
        profile_roots()
            .iter()
            .flat_map(|r| child_profiles(r))
            .map(origin)
            .collect()
    }

    /// Claim a profile and do not descend: it holds browsing data, not projects.
    fn detect(&self, dir: &Path, items: &[DirItem]) -> Detect {
        let has = |name: &str| {
            items
                .iter()
                .any(|i| i.file_name() == Some(name) && i.file_type.is_file())
        };
        if has(EXTENSIONS_JSON) && has(PREFS_JS) {
            return Detect {
                origins: vec![origin(dir.to_path_buf())],
                prune: true,
                ..Default::default()
            };
        }
        Detect::default()
    }

    fn accepts(&self, path: &Path) -> bool {
        path_normalize(path, false).is_ok_and(|p| is_profile(&p))
    }

    fn normalize_origin(&self, path: &Path) -> ResultDynError<Origin> {
        let p = path_normalize(path, true)?;
        if is_profile(&p) {
            Ok(origin(p))
        } else {
            Err(format!(
                "Not a Firefox profile (no {EXTENSIONS_JSON} and {PREFS_JS}): {p:?}"
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
        let p = origin.as_path();
        if is_profile(p) {
            vec![Site::new(
                Ecosystem::Firefox,
                SiteKind::Installed,
                PathShared::from(p.to_path_buf()),
            )]
        } else {
            Vec::new()
        }
    }

    fn site_to_packages(&self, site: &Site) -> Vec<Package> {
        let unique: HashSet<Package> = addons(site.as_path())
            .iter()
            .filter_map(addon_package)
            .collect();
        unique.into_iter().collect()
    }

    /// Add-ons that can read every site, hold a strong permission, are unsigned, or were sideloaded or loaded temporarily (see `browser::Grant::detail`).
    fn site_to_hooks(&self, site: &Site) -> Vec<Hook> {
        let mut hooks: Vec<Hook> = addons(site.as_path())
            .iter()
            .filter_map(|a| {
                Some(Hook {
                    name: addon_package(a)?.to_string(),
                    detail: addon_grant(a).detail()?,
                })
            })
            .collect();
        hooks.sort();
        hooks.dedup();
        hooks
    }

    /// Each add-on's installed file (`.xpi`) or directory, from its `path`.
    fn site_to_artifacts(
        &self,
        site: &Site,
        wanted: &HashSet<&Package>,
    ) -> Vec<(Package, Artifacts)> {
        let mut found: HashMap<Package, Artifacts> = HashMap::new();
        for a in addons(site.as_path()) {
            let (Some(p), Some(path)) = (
                addon_package(&a),
                a.get("path").and_then(JsonValue::as_str).map(PathBuf::from),
            ) else {
                continue;
            };
            if !wanted.contains(&p) {
                continue;
            }
            let arts = if path.is_dir() {
                Artifacts::from_dirs(vec![path], &[])
            } else {
                let exists = path.is_file();
                Artifacts {
                    files: vec![(path, exists)],
                    dirs: Vec::new(),
                }
            };
            let e = found.entry(p).or_default();
            e.files.extend(arts.files);
            e.dirs.extend(arts.dirs);
        }
        found.into_iter().collect()
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    fn make_profile(root: &Path) -> PathBuf {
        let profile = root.join("abcd1234.default-release");
        fs::create_dir_all(profile.join("extensions")).unwrap();
        let xpi = profile.join("extensions/plugin@okta.com.xpi");
        fs::write(&xpi, "zip").unwrap();
        fs::write(profile.join(PREFS_JS), "// not read\n").unwrap();
        let doc = json!({"schemaVersion": 36, "addons": [
            {"id": "plugin@okta.com", "version": "6.49.0", "type": "extension", "location": "app-profile",
             "active": true, "signedState": 2, "path": xpi.to_string_lossy(),
             "sourceURI": "https://addons.mozilla.org/firefox/downloads/file/1/okta.xpi",
             "defaultLocale": {"name": "Okta Browser Plugin"},
             "userPermissions": {"permissions": ["tabs", "cookies", "storage", "webRequest"], "origins": ["https://*/", "http://*/"]}},
            {"id": "{a4c4eda4-fb84-4a84-b4a1-f7c1cbf2a1ad}", "version": "1.0a1", "type": "extension", "location": "app-temporary",
             "active": false, "signedState": 0, "defaultLocale": {"name": "My Dev Add-on"},
             "userPermissions": {"permissions": ["storage"], "origins": []}},
            {"id": "quiet@example.org", "version": "2.0", "type": "extension", "location": "app-profile", "active": true, "signedState": 2,
             "sourceURI": "https://addons.mozilla.org/x.xpi", "userPermissions": {"permissions": ["storage"], "origins": ["https://example.org/*"]}},
            {"id": "webcompat@mozilla.org", "version": "153.9.0", "type": "extension", "location": "app-builtin-addons", "active": true,
             "userPermissions": {"permissions": ["tabs"], "origins": ["<all_urls>"]}},
            {"id": "en-US@dictionaries.addons.mozilla.org", "version": "1.0", "type": "dictionary", "location": "app-profile"},
        ]});
        fs::write(profile.join(EXTENSIONS_JSON), doc.to_string()).unwrap();
        profile
    }

    #[test]
    fn test_packages_a() {
        let dir = tempdir().unwrap();
        let profile = make_profile(dir.path());
        let o = FirefoxScanner.normalize_origin(&profile).unwrap();
        let sites = FirefoxScanner.origin_to_sites(
            &o,
            &ScanConfig::new(false, false),
            FlagLog(false),
        );
        let mut names: Vec<String> = FirefoxScanner
            .site_to_packages(&sites[0])
            .iter()
            .map(|p| p.to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "plugin@okta.com-6.49.0",
                "quiet@example.org-2.0",
                "{a4c4eda4-fb84-4a84-b4a1-f7c1cbf2a1ad}-1.0a1",
            ]
        );
    }

    #[test]
    fn test_hooks_a() {
        let dir = tempdir().unwrap();
        let profile = make_profile(dir.path());
        let site = Site::new(
            Ecosystem::Firefox,
            SiteKind::Installed,
            PathShared::from(profile),
        );
        let hooks = FirefoxScanner.site_to_hooks(&site);
        let got: Vec<(&str, &str)> = hooks
            .iter()
            .map(|h| (h.name.as_str(), h.detail.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                (
                    "plugin@okta.com-6.49.0",
                    "Okta Browser Plugin; all sites: http://*/, https://*/; permissions: cookies, webRequest; source: addons.mozilla.org"
                ),
                (
                    "{a4c4eda4-fb84-4a84-b4a1-f7c1cbf2a1ad}-1.0a1",
                    "My Dev Add-on; source: temporary (about:debugging); unsigned; disabled"
                ),
            ]
        );
    }

    #[test]
    fn test_detect_and_artifacts_a() {
        let dir = tempdir().unwrap();
        let profile = make_profile(dir.path());
        let items: Vec<DirItem> = fs::read_dir(&profile)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                DirItem {
                    path: e.path(),
                    file_type: e.file_type().unwrap(),
                }
            })
            .collect();
        let d = FirefoxScanner.detect(&profile, &items);
        assert_eq!(d.origins.len(), 1);
        assert!(d.prune);
        assert_eq!(child_profiles(dir.path()), std::slice::from_ref(&profile));
        let site = Site::new(
            Ecosystem::Firefox,
            SiteKind::Installed,
            PathShared::from(profile),
        );
        let pkgs = FirefoxScanner.site_to_packages(&site);
        let wanted: HashSet<&Package> = pkgs.iter().collect();
        let arts = FirefoxScanner.site_to_artifacts(&site, &wanted);
        // only the Okta add-on records a path
        assert_eq!(arts.len(), 1);
        assert!(arts[0].1.files[0].1);
    }
}
