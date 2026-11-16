//! Helpers shared by the browser extension scanners (`chromium`, `firefox`): where browsers keep their data on each platform, and how an extension's permissions are judged for `inspect`.
//!
//! Browser profiles hold private data (history, cookies, saved passwords, caches). The scanners read only the files that describe extensions: for Chromium, `Preferences` and `Secure Preferences` (only their `extensions.settings`) and each extension's `manifest.json` and locale messages; for Firefox, `extensions.json`. Nothing else in a profile is opened, and nothing but the packages found is kept.

use std::env;
use std::path::PathBuf;

use crate::util::path_home;

//------------------------------------------------------------------------------
/// Platform base directories for application data, in which browsers keep their profiles: on macOS `~/Library/Application Support`; on Linux `~/.config` and the home directory itself (for dot-directories such as `~/.mozilla`); on Windows `%LOCALAPPDATA%` and `%APPDATA%`.
pub(crate) struct DataDirs {
    pub(crate) home: Option<PathBuf>,
    pub(crate) mac_support: Option<PathBuf>,
    pub(crate) xdg_config: Option<PathBuf>,
    pub(crate) win_local: Option<PathBuf>,
    pub(crate) win_roaming: Option<PathBuf>,
}

impl DataDirs {
    pub(crate) fn current() -> Self {
        let home = path_home();
        let env_dir =
            |k: &str| env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let under_home = |parts: &[&str]| {
            home.as_ref()
                .map(|h| parts.iter().fold(h.clone(), |p, s| p.join(s)))
        };
        let mac = cfg!(target_os = "macos");
        let win = cfg!(windows);
        DataDirs {
            mac_support: under_home(&["Library", "Application Support"]).filter(|_| mac),
            xdg_config: (!mac && !win)
                .then(|| env_dir("XDG_CONFIG_HOME").or_else(|| under_home(&[".config"])))
                .flatten(),
            win_local: win
                .then(|| {
                    env_dir("LOCALAPPDATA").or_else(|| under_home(&["AppData", "Local"]))
                })
                .flatten(),
            win_roaming: win
                .then(|| {
                    env_dir("APPDATA").or_else(|| under_home(&["AppData", "Roaming"]))
                })
                .flatten(),
            home,
        }
    }
}

/// Join a `/`-separated relative path onto a base.
pub(crate) fn join_rel(base: &std::path::Path, rel: &str) -> PathBuf {
    rel.split('/').fold(base.to_path_buf(), |p, s| p.join(s))
}

//------------------------------------------------------------------------------
// permissions

/// True if a match pattern grants access to every site: `<all_urls>`, or a host of `*` (`*://*/*`, `https://*/`, `file:///*`).
pub(crate) fn is_broad_host(pattern: &str) -> bool {
    let p = pattern.trim();
    if p == "<all_urls>" || p == "*" {
        return true;
    }
    let Some((scheme, rest)) = p.split_once("://") else {
        return false;
    };
    let host = rest.split('/').next().unwrap_or("");
    host == "*" || (scheme == "file" && host.is_empty())
}

/// API permissions that give an extension control well beyond the pages it is on; with any of these, or access to every site, an extension is reported by `inspect`.
pub(crate) const STRONG_APIS: [&str; 4] =
    ["debugger", "management", "nativeMessaging", "proxy"];

/// API permissions listed in an `inspect` detail when present: those that read or alter browsing data or traffic.
pub(crate) const NOTABLE_APIS: [&str; 17] = [
    "clipboardRead",
    "contentSettings",
    "cookies",
    "debugger",
    "declarativeNetRequestWithHostAccess",
    "desktopCapture",
    "downloads",
    "history",
    "management",
    "nativeMessaging",
    "privacy",
    "proxy",
    "scripting",
    "tabCapture",
    "webNavigation",
    "webRequest",
    "webRequestBlocking",
];

/// An extension's permissions and state, as the scanners read them, for `inspect`.
#[derive(Debug, Default)]
pub(crate) struct Grant {
    /// Human-readable name.
    pub(crate) display: String,
    /// Host match patterns granted (including content-script matches).
    pub(crate) hosts: Vec<String>,
    /// API permissions granted.
    pub(crate) apis: Vec<String>,
    /// How it was installed (`webstore`, `unpacked`, `policy`, `addons.mozilla.org`, ...).
    pub(crate) source: String,
    /// True if the install source is itself a signal: developer mode, sideloading, or a temporary load.
    pub(crate) source_flag: bool,
    pub(crate) disabled: bool,
    /// Other notes (`manifest v2`, `unsigned`).
    pub(crate) notes: Vec<String>,
}

impl Grant {
    /// An `inspect` detail if the extension can read every site, has a strong API, or was installed from a flagged source; else `None`.
    pub(crate) fn detail(&self) -> Option<String> {
        let mut broad: Vec<&str> = self
            .hosts
            .iter()
            .map(String::as_str)
            .filter(|h| is_broad_host(h))
            .collect();
        broad.sort();
        broad.dedup();
        let strong = self.apis.iter().any(|a| STRONG_APIS.contains(&a.as_str()));
        if broad.is_empty() && !strong && !self.source_flag {
            return None;
        }
        let mut apis: Vec<&str> = self
            .apis
            .iter()
            .map(String::as_str)
            .filter(|a| NOTABLE_APIS.contains(a))
            .collect();
        apis.sort();
        apis.dedup();
        let mut parts = vec![if self.display.is_empty() {
            "(no name)".to_string()
        } else {
            self.display.clone()
        }];
        if !broad.is_empty() {
            parts.push(format!("all sites: {}", broad.join(", ")));
        }
        if !apis.is_empty() {
            parts.push(format!("permissions: {}", apis.join(", ")));
        }
        parts.push(format!("source: {}", self.source));
        parts.extend(self.notes.iter().cloned());
        if self.disabled {
            parts.push("disabled".to_string());
        }
        Some(parts.join("; "))
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_broad_host_a() {
        for p in [
            "<all_urls>",
            "*://*/*",
            "https://*/",
            "http://*/*",
            "file:///*",
            "*",
        ] {
            assert!(is_broad_host(p), "{p}");
        }
        for p in [
            "https://*.google.com/*",
            "https://example.com/*",
            "chrome://favicon/*",
            "storage",
        ] {
            assert!(!is_broad_host(p), "{p}");
        }
    }

    #[test]
    fn test_grant_detail_a() {
        let mut g = Grant {
            display: "Okta".into(),
            hosts: vec![
                "https://*/".into(),
                "http://*/".into(),
                "https://okta.com/*".into(),
            ],
            apis: vec![
                "tabs".into(),
                "cookies".into(),
                "webRequest".into(),
                "storage".into(),
            ],
            source: "addons.mozilla.org".into(),
            ..Default::default()
        };
        assert_eq!(
            g.detail().unwrap(),
            "Okta; all sites: http://*/, https://*/; permissions: cookies, webRequest; source: addons.mozilla.org"
        );
        g.hosts = vec!["https://okta.com/*".into()];
        assert!(g.detail().is_none());
        g.apis.push("nativeMessaging".into());
        g.disabled = true;
        assert_eq!(
            g.detail().unwrap(),
            "Okta; permissions: cookies, nativeMessaging, webRequest; source: addons.mozilla.org; disabled"
        );
        let g = Grant {
            display: "Dev".into(),
            source: "unpacked".into(),
            source_flag: true,
            notes: vec!["manifest v2".into()],
            ..Default::default()
        };
        assert_eq!(g.detail().unwrap(), "Dev; source: unpacked; manifest v2");
    }
}
