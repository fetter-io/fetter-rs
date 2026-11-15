//! Chromium-family browser extensions scanner (Chrome, Edge, Brave, Vivaldi, Opera, Arc, Chromium, Yandex).
//!
//! An origin is a browser profile: a directory with a `Preferences` file and a `Secure Preferences` file or an `Extensions` directory (Electron applications keep a `Preferences` file too, but neither of the others). Profiles are found in each browser's user-data directory (`BROWSERS`, per platform), whether it is itself a profile (Opera) or holds them (`Default`, `Profile 1`, ...). The profile is the site (`SiteKind::Installed`).
//!
//! Extensions are read from `extensions.settings` in `Secure Preferences` (macOS, Windows) and `Preferences` (Linux), merged, which also lists unpacked (developer mode) extensions outside the profile; component extensions built into the browser (locations 5 and 10) are excluded. Each extension's `manifest.json` (at its `path`, relative to `Extensions` unless absolute) gives the version and name, with the manifest copy in the settings as a fallback. If neither preferences file has settings, every version directory under `Extensions/<id>/` is read. Packages are named by extension id (32 letters `a`–`p`), which the store and enterprise policies use; `inspect` gives the display names.
//!
//! See `browser` for what is and is not read from a profile.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use serde_json::Map as JsonMap;
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

const PREFERENCES: &str = "Preferences";
const SECURE_PREFERENCES: &str = "Secure Preferences";
const EXTENSIONS: &str = "Extensions";
const MANIFEST: &str = "manifest.json";

/// User-data directories, relative to the platform bases in `DataDirs`: (macOS Application Support, Linux config or home, Windows local or roaming app data).
const MAC_BROWSERS: [&str; 18] = [
    "Google/Chrome",
    "Google/Chrome Beta",
    "Google/Chrome Dev",
    "Google/Chrome Canary",
    "Chromium",
    "Microsoft Edge",
    "Microsoft Edge Beta",
    "Microsoft Edge Dev",
    "Microsoft Edge Canary",
    "BraveSoftware/Brave-Browser",
    "BraveSoftware/Brave-Browser-Beta",
    "BraveSoftware/Brave-Browser-Nightly",
    "Vivaldi",
    "Arc/User Data",
    "com.operasoftware.Opera",
    "com.operasoftware.OperaGX",
    "Yandex/YandexBrowser",
    "Thorium",
];
const LINUX_CONFIG_BROWSERS: [&str; 13] = [
    "google-chrome",
    "google-chrome-beta",
    "google-chrome-unstable",
    "chromium",
    "microsoft-edge",
    "microsoft-edge-beta",
    "microsoft-edge-dev",
    "BraveSoftware/Brave-Browser",
    "BraveSoftware/Brave-Browser-Beta",
    "vivaldi",
    "opera",
    "yandex-browser",
    "thorium",
];
/// Snap and Flatpak installs, relative to the home directory.
const LINUX_HOME_BROWSERS: [&str; 5] = [
    "snap/chromium/common/chromium",
    ".var/app/com.google.Chrome/config/google-chrome",
    ".var/app/org.chromium.Chromium/config/chromium",
    ".var/app/com.brave.Browser/config/BraveSoftware/Brave-Browser",
    ".var/app/com.microsoft.Edge/config/microsoft-edge",
];
const WIN_LOCAL_BROWSERS: [&str; 11] = [
    "Google/Chrome/User Data",
    "Google/Chrome Beta/User Data",
    "Google/Chrome Dev/User Data",
    "Google/Chrome SxS/User Data",
    "Chromium/User Data",
    "Microsoft/Edge/User Data",
    "Microsoft/Edge Beta/User Data",
    "Microsoft/Edge Dev/User Data",
    "BraveSoftware/Brave-Browser/User Data",
    "Vivaldi/User Data",
    "Yandex/YandexBrowser/User Data",
];
const WIN_ROAMING_BROWSERS: [&str; 2] = [
    "Opera Software/Opera Stable",
    "Opera Software/Opera GX Stable",
];

/// Extension install locations (Chromium's `ManifestLocation`).
const LOC_INTERNAL: i64 = 1;
const LOC_UNPACKED: i64 = 4;
const LOC_COMPONENT: i64 = 5;
const LOC_COMMAND_LINE: i64 = 8;
const LOC_EXTERNAL_COMPONENT: i64 = 10;

//------------------------------------------------------------------------------
/// The browser user-data directories on this platform that exist.
fn user_data_dirs() -> Vec<PathBuf> {
    let d = DataDirs::current();
    let mut out = Vec::new();
    let mut add = |base: &Option<PathBuf>, rels: &[&str]| {
        if let Some(b) = base {
            out.extend(rels.iter().map(|r| join_rel(b, r)).filter(|p| p.is_dir()));
        }
    };
    add(&d.mac_support, &MAC_BROWSERS);
    add(&d.xdg_config, &LINUX_CONFIG_BROWSERS);
    if d.xdg_config.is_some() {
        add(&d.home, &LINUX_HOME_BROWSERS);
    }
    add(&d.win_local, &WIN_LOCAL_BROWSERS);
    add(&d.win_roaming, &WIN_ROAMING_BROWSERS);
    out
}

/// A profile directory: `Preferences`, and `Secure Preferences` or `Extensions`.
fn is_profile(dir: &Path) -> bool {
    dir.join(PREFERENCES).is_file()
        && (dir.join(SECURE_PREFERENCES).is_file() || dir.join(EXTENSIONS).is_dir())
}

/// The profiles of a user-data directory: itself if a profile, else its child profiles.
fn profiles(user_data: &Path) -> Vec<PathBuf> {
    if is_profile(user_data) {
        return vec![user_data.to_path_buf()];
    }
    let mut out: Vec<PathBuf> = fs::read_dir(user_data)
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

fn origin(dir: PathBuf) -> Origin {
    Origin::new(Ecosystem::Chromium, PathShared::from(dir))
}

/// True for a Chromium extension id: 32 letters `a`–`p`.
pub(crate) fn is_extension_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| (b'a'..=b'p').contains(&b))
}

//------------------------------------------------------------------------------
/// `extensions.settings` of both preferences files, merged per extension (fields of `Secure Preferences` first, then those only in `Preferences`). Nothing else in the files is kept.
fn extension_settings(profile: &Path) -> BTreeMap<String, JsonMap<String, JsonValue>> {
    let mut out: BTreeMap<String, JsonMap<String, JsonValue>> = BTreeMap::new();
    for file in [SECURE_PREFERENCES, PREFERENCES] {
        let Some(settings) = fs::read_to_string(profile.join(file))
            .ok()
            .and_then(|c| serde_json::from_str::<JsonValue>(&c).ok())
            .and_then(|mut v| {
                v.get_mut("extensions")
                    .and_then(|e| e.get_mut("settings"))
                    .map(JsonValue::take)
            })
        else {
            continue;
        };
        let JsonValue::Object(settings) = settings else {
            continue;
        };
        for (id, v) in settings {
            if let JsonValue::Object(fields) = v {
                let entry = out.entry(id).or_default();
                for (k, f) in fields {
                    entry.entry(k).or_insert(f);
                }
            }
        }
    }
    out
}

/// One installed extension: id, directory (if found), manifest, and its settings (empty when read from `Extensions` alone).
struct Installed {
    id: String,
    dir: Option<PathBuf>,
    manifest: JsonValue,
    settings: JsonMap<String, JsonValue>,
}

impl Installed {
    fn version(&self) -> Option<String> {
        if let Some(v) = self.manifest.get("version").and_then(JsonValue::as_str) {
            return Some(v.to_string());
        }
        // a version directory is named `<version>_<n>`
        let name = self.dir.as_ref()?.file_name()?.to_str()?;
        let v = name.rsplit_once('_').map_or(name, |(v, _)| v);
        crate::eco::browser_version::is_chromium_version(v).then(|| v.to_string())
    }

    fn package(&self) -> Option<Package> {
        Some(Package::new(
            Ecosystem::Chromium,
            &self.id,
            VersionSpec::new(&self.version()?),
            None,
        ))
    }

    fn location(&self) -> Option<i64> {
        self.settings.get("location").and_then(JsonValue::as_i64)
    }

    /// Disabled: non-empty `disable_reasons` (a list, or a bitmask in older versions), or `state` 0.
    fn disabled(&self) -> bool {
        let reasons = match self.settings.get("disable_reasons") {
            Some(JsonValue::Array(a)) => !a.is_empty(),
            Some(JsonValue::Number(n)) => n.as_i64().is_some_and(|n| n != 0),
            _ => false,
        };
        reasons || self.settings.get("state").and_then(JsonValue::as_i64) == Some(0)
    }

    /// The display name: the manifest's `name`, with a `__MSG_key__` placeholder resolved from `_locales/<default_locale>/messages.json`.
    fn display(&self) -> String {
        let name = self
            .manifest
            .get("name")
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        let Some(key) = name
            .strip_prefix("__MSG_")
            .and_then(|n| n.strip_suffix("__"))
        else {
            return name.to_string();
        };
        let locale = self
            .manifest
            .get("default_locale")
            .and_then(JsonValue::as_str)
            .unwrap_or("en");
        self.dir
            .as_ref()
            .and_then(|d| {
                fs::read_to_string(d.join("_locales").join(locale).join("messages.json"))
                    .ok()
            })
            .and_then(|c| serde_json::from_str::<JsonMap<String, JsonValue>>(&c).ok())
            .and_then(|m| {
                m.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(key))
                    .and_then(|(_, v)| v.get("message")?.as_str().map(str::to_string))
            })
            .unwrap_or_else(|| name.to_string())
    }

    /// Permissions and state for `inspect`: the granted permissions (`active_permissions`) if recorded, else the manifest's (with content-script matches as hosts).
    fn grant(&self) -> Grant {
        let strings = |v: Option<&JsonValue>| -> Vec<String> {
            v.and_then(JsonValue::as_array)
                .into_iter()
                .flatten()
                .filter_map(|x| match x {
                    JsonValue::String(s) => Some(s.clone()),
                    // a permission with arguments: `{"fileSystem": ["write"]}`
                    JsonValue::Object(o) => o.keys().next().cloned(),
                    _ => None,
                })
                .collect()
        };
        let mut hosts = Vec::new();
        let mut apis = Vec::new();
        if let Some(ap) = self.settings.get("active_permissions") {
            apis.extend(strings(ap.get("api")));
            apis.extend(strings(ap.get("manifest_permissions")));
            hosts.extend(strings(ap.get("explicit_host")));
            hosts.extend(strings(ap.get("scriptable_host")));
        } else {
            for p in strings(self.manifest.get("permissions")) {
                if p.contains("://") || p == "<all_urls>" {
                    hosts.push(p);
                } else {
                    apis.push(p);
                }
            }
            hosts.extend(strings(self.manifest.get("host_permissions")));
            for cs in self
                .manifest
                .get("content_scripts")
                .and_then(JsonValue::as_array)
                .into_iter()
                .flatten()
            {
                hosts.extend(strings(cs.get("matches")));
            }
        }
        let from_webstore = self
            .settings
            .get("from_webstore")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let (source, source_flag) = match self.location() {
            Some(LOC_INTERNAL) if from_webstore => ("webstore", false),
            Some(LOC_INTERNAL) => ("internal (not from the store)", true),
            Some(2 | 3 | 6) => ("external (sideloaded)", true),
            Some(LOC_UNPACKED) => ("unpacked (developer mode)", true),
            Some(7 | 9) => ("policy", false),
            Some(LOC_COMMAND_LINE) => ("command line", true),
            Some(_) => ("other", false),
            None => ("unknown", false),
        };
        let mut notes = Vec::new();
        if self
            .manifest
            .get("manifest_version")
            .and_then(JsonValue::as_i64)
            == Some(2)
        {
            notes.push("manifest v2".to_string());
        }
        Grant {
            display: self.display(),
            hosts,
            apis,
            source: source.to_string(),
            source_flag,
            disabled: self.disabled(),
            notes,
        }
    }
}

fn read_manifest(dir: &Path) -> Option<JsonValue> {
    serde_json::from_str(&fs::read_to_string(dir.join(MANIFEST)).ok()?).ok()
}

/// The extensions installed in a profile (see the module documentation), sorted by id and version.
fn installed(profile: &Path) -> Vec<Installed> {
    let ext_root = profile.join(EXTENSIONS);
    let settings = extension_settings(profile);
    let mut out = Vec::new();
    if settings.is_empty() {
        // no preferences: every version directory of every extension
        for e in fs::read_dir(&ext_root).into_iter().flatten().flatten() {
            let id = e.file_name().to_string_lossy().to_string();
            if !is_extension_id(&id) {
                continue;
            }
            for v in fs::read_dir(e.path()).into_iter().flatten().flatten() {
                let dir = v.path();
                if let Some(manifest) = read_manifest(&dir) {
                    out.push(Installed {
                        id: id.clone(),
                        dir: Some(dir),
                        manifest,
                        settings: JsonMap::new(),
                    });
                }
            }
        }
    } else {
        for (id, s) in settings {
            let loc = s.get("location").and_then(JsonValue::as_i64);
            if matches!(loc, Some(LOC_COMPONENT | LOC_EXTERNAL_COMPONENT))
                || !is_extension_id(&id)
            {
                continue;
            }
            let dir = s.get("path").and_then(JsonValue::as_str).map(|p| {
                let pb = PathBuf::from(p);
                if pb.is_absolute() {
                    pb
                } else {
                    join_rel(&ext_root, p)
                }
            });
            let manifest = dir
                .as_deref()
                .and_then(read_manifest)
                .or_else(|| s.get("manifest").cloned());
            let Some(manifest) = manifest else {
                continue;
            };
            out.push(Installed {
                id,
                dir: dir.filter(|d| d.is_dir()),
                manifest,
                settings: s,
            });
        }
    }
    out.sort_by(|a, b| (&a.id, a.version()).cmp(&(&b.id, b.version())));
    out
}

//------------------------------------------------------------------------------
pub(crate) struct ChromiumScanner;

impl EcosystemScanner for ChromiumScanner {
    fn default_origins(&self) -> Vec<Origin> {
        user_data_dirs()
            .iter()
            .flat_map(|d| profiles(d))
            .map(origin)
            .collect()
    }

    /// Claim a profile and do not descend: it holds browsing data, not projects.
    fn detect(&self, dir: &Path, items: &[DirItem]) -> Detect {
        let has = |name: &str, dir_wanted: bool| {
            items.iter().any(|i| {
                i.file_name() == Some(name)
                    && if dir_wanted {
                        i.file_type.is_dir()
                    } else {
                        i.file_type.is_file()
                    }
            })
        };
        if has(PREFERENCES, false)
            && (has(SECURE_PREFERENCES, false) || has(EXTENSIONS, true))
        {
            return Detect {
                origins: vec![origin(dir.to_path_buf())],
                prune: true,
                ..Default::default()
            };
        }
        Detect::default()
    }

    /// Accept a profile directory, or a user-data directory holding profiles.
    fn accepts(&self, path: &Path) -> bool {
        let Ok(p) = path_normalize(path, false) else {
            return false;
        };
        p.is_dir() && (is_profile(&p) || !profiles(&p).is_empty())
    }

    fn normalize_origin(&self, path: &Path) -> ResultDynError<Origin> {
        let p = path_normalize(path, true)?;
        if is_profile(&p) {
            return Ok(origin(p));
        }
        // a user-data directory: its first profile (`Default` sorts first among `Default`, `Profile N`)
        let mut found = profiles(&p);
        found.sort_by_key(|d| d.file_name().is_none_or(|n| n != "Default"));
        found
            .into_iter()
            .next()
            .map(origin)
            .ok_or_else(|| format!("Not a Chromium browser profile: {p:?}").into())
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
                Ecosystem::Chromium,
                SiteKind::Installed,
                PathShared::from(p.to_path_buf()),
            )]
        } else {
            Vec::new()
        }
    }

    fn site_to_packages(&self, site: &Site) -> Vec<Package> {
        let unique: HashSet<Package> = installed(site.as_path())
            .iter()
            .filter_map(Installed::package)
            .collect();
        unique.into_iter().collect()
    }

    /// Extensions that can read every site, hold a strong permission, or were installed outside the store (see `browser::Grant::detail`).
    fn site_to_hooks(&self, site: &Site) -> Vec<Hook> {
        installed(site.as_path())
            .iter()
            .filter_map(|i| {
                let detail = i.grant().detail()?;
                Some(Hook {
                    name: i.package()?.to_string(),
                    detail,
                })
            })
            .collect()
    }

    /// Each extension's version directory (for an unpacked extension, its source directory).
    fn site_to_artifacts(
        &self,
        site: &Site,
        wanted: &HashSet<&Package>,
    ) -> Vec<(Package, Artifacts)> {
        let mut found: HashMap<Package, Vec<PathBuf>> = HashMap::new();
        for i in installed(site.as_path()) {
            if let (Some(p), Some(d)) = (i.package(), i.dir.clone()) {
                if wanted.contains(&p) {
                    found.entry(p).or_default().push(d);
                }
            }
        }
        found
            .into_iter()
            .map(|(p, mut dirs)| {
                dirs.sort();
                (p, Artifacts::from_dirs(dirs, &[]))
            })
            .collect()
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    const WEBSTORE: &str = "cjpalhdlnbpafiamejdnhcphjbkeiagm";
    const UNPACKED: &str = "abcdefghijklmnopabcdefghijklmnop";
    const COMPONENT: &str = "mhjfbmdgcfjbbpaeojofohoefgiehjai";

    fn write_ext(dir: &Path, manifest: JsonValue) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(MANIFEST), manifest.to_string()).unwrap();
    }

    /// A profile with a store extension (localized name, old version directory left behind), an unpacked extension outside the profile, and a component extension.
    fn make_profile(root: &Path) -> PathBuf {
        let profile = root.join("Default");
        let ext = profile.join(EXTENSIONS);
        write_ext(
            &ext.join(WEBSTORE).join("1.62.0_0"),
            json!({"manifest_version": 3, "name": "__MSG_extName__", "default_locale": "en", "version": "1.62.0",
                   "permissions": ["storage"], "host_permissions": ["<all_urls>"]}),
        );
        let loc = ext.join(WEBSTORE).join("1.62.0_0/_locales/en");
        fs::create_dir_all(&loc).unwrap();
        fs::write(
            loc.join("messages.json"),
            r#"{"extName": {"message": "uBlock Origin Lite"}}"#,
        )
        .unwrap();
        write_ext(
            &ext.join(WEBSTORE).join("1.61.0_0"),
            json!({"name": "x", "version": "1.61.0"}),
        );
        let dev = root.join("src/my-ext");
        write_ext(
            &dev,
            json!({"manifest_version": 2, "name": "Dev Tool", "version": "0.1",
                               "permissions": ["tabs", "nativeMessaging"]}),
        );
        let settings = json!({"extensions": {"settings": {
            WEBSTORE: {"location": 1, "from_webstore": true, "path": format!("{WEBSTORE}/1.62.0_0"),
                       "active_permissions": {"api": ["storage", "scripting"], "explicit_host": ["<all_urls>"], "manifest_permissions": [], "scriptable_host": []}},
            UNPACKED: {"location": 4, "path": dev.to_string_lossy(), "disable_reasons": [1]},
            COMPONENT: {"location": 5, "path": "/Applications/Browser.app/resources/pdf", "manifest": {"name": "PDF", "version": "1"}},
        }}});
        fs::write(profile.join(SECURE_PREFERENCES), settings.to_string()).unwrap();
        fs::write(
            profile.join(PREFERENCES),
            r#"{"browser": {"other": "not read"}}"#,
        )
        .unwrap();
        profile
    }

    fn names(packages: &[Package]) -> Vec<String> {
        let mut out: Vec<String> = packages.iter().map(|p| p.to_string()).collect();
        out.sort();
        out
    }

    #[test]
    fn test_is_extension_id_a() {
        assert!(is_extension_id(WEBSTORE));
        assert!(!is_extension_id("Temp"));
        assert!(!is_extension_id("cjpalhdlnbpafiamejdnhcphjbkeiagz"));
    }

    #[test]
    fn test_packages_a() {
        let dir = tempdir().unwrap();
        let profile = make_profile(dir.path());
        let o = ChromiumScanner.normalize_origin(&profile).unwrap();
        let sites = ChromiumScanner.origin_to_sites(
            &o,
            &ScanConfig::new(false, false),
            FlagLog(false),
        );
        assert_eq!(sites.len(), 1);
        let pkgs = ChromiumScanner.site_to_packages(&sites[0]);
        // the component extension and the left-behind version are excluded
        assert_eq!(
            names(&pkgs),
            [format!("{UNPACKED}-0.1"), format!("{WEBSTORE}-1.62.0")]
        );
        assert!(pkgs.iter().all(|p| p.ecosystem == Ecosystem::Chromium));
    }

    #[test]
    fn test_hooks_a() {
        let dir = tempdir().unwrap();
        let profile = make_profile(dir.path());
        let site = Site::new(
            Ecosystem::Chromium,
            SiteKind::Installed,
            PathShared::from(profile),
        );
        let hooks = ChromiumScanner.site_to_hooks(&site);
        let details: Vec<(&str, &str)> = hooks
            .iter()
            .map(|h| (h.name.as_str(), h.detail.as_str()))
            .collect();
        assert_eq!(
            details,
            [
                (
                    format!("{UNPACKED}-0.1").as_str(),
                    "Dev Tool; permissions: nativeMessaging; source: unpacked (developer mode); manifest v2; disabled"
                ),
                (
                    format!("{WEBSTORE}-1.62.0").as_str(),
                    "uBlock Origin Lite; all sites: <all_urls>; permissions: scripting; source: webstore"
                ),
            ]
        );
    }

    #[test]
    fn test_without_settings_a() {
        // no preferences settings: every version directory under Extensions
        let dir = tempdir().unwrap();
        let profile = make_profile(dir.path());
        fs::write(profile.join(SECURE_PREFERENCES), "{}").unwrap();
        let site = Site::new(
            Ecosystem::Chromium,
            SiteKind::Installed,
            PathShared::from(profile),
        );
        assert_eq!(
            names(&ChromiumScanner.site_to_packages(&site)),
            [format!("{WEBSTORE}-1.61.0"), format!("{WEBSTORE}-1.62.0")]
        );
    }

    #[test]
    fn test_detect_a() {
        let dir = tempdir().unwrap();
        let user_data = dir.path().join("Chrome");
        let profile = make_profile(&user_data);
        // an Electron app: Preferences only
        let app = dir.path().join("SomeApp");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join(PREFERENCES), "{}").unwrap();
        let items = |d: &Path| -> Vec<DirItem> {
            fs::read_dir(d)
                .unwrap()
                .map(|e| {
                    let e = e.unwrap();
                    DirItem {
                        path: e.path(),
                        file_type: e.file_type().unwrap(),
                    }
                })
                .collect()
        };
        let d = ChromiumScanner.detect(&profile, &items(&profile));
        assert_eq!(d.origins.len(), 1);
        assert!(d.prune);
        assert!(ChromiumScanner
            .detect(&app, &items(&app))
            .origins
            .is_empty());
        assert!(ChromiumScanner.accepts(&user_data));
        assert!(!ChromiumScanner.accepts(&app));
        assert_eq!(
            ChromiumScanner
                .normalize_origin(&user_data)
                .unwrap()
                .as_path(),
            path_normalize(&profile, true).unwrap()
        );
    }

    #[test]
    fn test_artifacts_a() {
        let dir = tempdir().unwrap();
        let profile = make_profile(dir.path());
        let site = Site::new(
            Ecosystem::Chromium,
            SiteKind::Installed,
            PathShared::from(profile),
        );
        let pkgs = ChromiumScanner.site_to_packages(&site);
        let wanted: HashSet<&Package> = pkgs.iter().collect();
        let arts = ChromiumScanner.site_to_artifacts(&site, &wanted);
        assert_eq!(arts.len(), 2);
        let store = arts.iter().find(|(p, _)| p.name == WEBSTORE).unwrap();
        assert!(store.1.dirs[0].ends_with("1.62.0_0"));
        assert_eq!(store.1.files.len(), 2);
    }
}
