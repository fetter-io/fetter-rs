//! What changed between two scans of one system, and whether the changes matter (`monitor-diff`).
//!
//! A `ScanDiff` lists origins, sites, packages and hooks added or removed; packages are compared per site, so a package whose version changed at a site is one change (upgraded, downgraded, or changed when the versions cannot be ordered or only the source differs). A `DiffReport` evaluates a diff against the baseline's audit and validation: new vulnerabilities (from a changed package, or newly published against an unchanged one), new hooks, downgrades and new allow-list violations are alerts.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;

use crate::allow_list::compare_versions;
use crate::cyclonedx::rfc3339;
use crate::eco::Hook;
use crate::ecosystem::Ecosystem;
use crate::origin_site::Origin;
use crate::origin_site::Site;
use crate::origin_site::SiteKind;
use crate::osv_vulns::VulnInfo;
use crate::package::Package;
use crate::scan_fs::ScanFS;
use crate::validation_report::ValidationDigestRecord;

//------------------------------------------------------------------------------
// the diff

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Change {
    Added,
    Removed,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum PackageChangeKind {
    Added,
    Removed,
    Upgraded,
    Downgraded,
    /// The version changed but cannot be ordered (e.g. a GitHub Actions ref), or only the source (direct URL, commit) changed.
    Changed,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct OriginChange {
    pub change: Change,
    pub ecosystem: Ecosystem,
    pub path: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SiteChange {
    pub change: Change,
    pub ecosystem: Ecosystem,
    pub kind: SiteKind,
    pub path: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PackageChange {
    pub change: PackageChangeKind,
    pub ecosystem: Ecosystem,
    /// The path of the site at which the package changed.
    pub site: String,
    /// The package before (absent if added).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Package>,
    /// The package after (absent if removed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Package>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct HookChange {
    pub change: Change,
    pub ecosystem: Ecosystem,
    pub site: String,
    pub hook: Hook,
}

/// The differences between two scans; every list is sorted (by path, then ecosystem and name).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct ScanDiff {
    #[serde(default)]
    pub origins: Vec<OriginChange>,
    #[serde(default)]
    pub sites: Vec<SiteChange>,
    #[serde(default)]
    pub packages: Vec<PackageChange>,
    /// Only compared when both scans collected hooks.
    #[serde(default)]
    pub hooks: Vec<HookChange>,
}

/// Elements of `now` not in `base`, and of `base` not in `now`, in order.
fn added_removed<'a, T: Ord>(
    base: &'a BTreeSet<T>,
    now: &'a BTreeSet<T>,
) -> (Vec<&'a T>, Vec<&'a T>) {
    (
        now.difference(base).collect(),
        base.difference(now).collect(),
    )
}

type SiteIndex<'a> = BTreeMap<(&'a Site, Ecosystem, &'a str), BTreeSet<&'a Package>>;

/// Packages by site and key.
fn site_index(sfs: &ScanFS) -> SiteIndex<'_> {
    let mut index: SiteIndex = BTreeMap::new();
    for (package, sites) in &sfs.package_to_sites {
        for site in sites {
            index
                .entry((site, package.ecosystem, package.key.as_str()))
                .or_default()
                .insert(package);
        }
    }
    index
}

impl ScanDiff {
    pub(crate) fn new(base: &ScanFS, now: &ScanFS) -> Self {
        let origin_set = |s: &ScanFS| {
            s.origin_to_sites
                .keys()
                .cloned()
                .collect::<BTreeSet<Origin>>()
        };
        let (ob, on) = (origin_set(base), origin_set(now));
        let (added, removed) = added_removed(&ob, &on);
        let origin_change = |change, o: &Origin| OriginChange {
            change,
            ecosystem: o.ecosystem,
            path: o.path.to_string(),
        };
        let mut origins: Vec<OriginChange> = added
            .into_iter()
            .map(|o| origin_change(Change::Added, o))
            .chain(
                removed
                    .into_iter()
                    .map(|o| origin_change(Change::Removed, o)),
            )
            .collect();
        origins.sort_by(|a, b| (&a.path, a.ecosystem).cmp(&(&b.path, b.ecosystem)));

        let site_set = |s: &ScanFS| {
            s.site_to_origins
                .keys()
                .cloned()
                .collect::<BTreeSet<Site>>()
        };
        let (sb, sn) = (site_set(base), site_set(now));
        let (added, removed) = added_removed(&sb, &sn);
        let site_change = |change, s: &Site| SiteChange {
            change,
            ecosystem: s.ecosystem,
            kind: s.kind,
            path: s.path.to_string(),
        };
        let mut sites: Vec<SiteChange> = added
            .into_iter()
            .map(|s| site_change(Change::Added, s))
            .chain(removed.into_iter().map(|s| site_change(Change::Removed, s)))
            .collect();
        sites.sort_by(|a, b| (&a.path, a.ecosystem).cmp(&(&b.path, b.ecosystem)));

        let (ib, in_) = (site_index(base), site_index(now));
        let empty = BTreeSet::new();
        let keys: BTreeSet<_> = ib.keys().chain(in_.keys()).collect();
        let mut packages = Vec::new();
        for key in keys {
            let (site, eco, _) = key;
            let pb = ib.get(key).unwrap_or(&empty);
            let pn = in_.get(key).unwrap_or(&empty);
            let (added, removed) = added_removed(pb, pn);
            let change =
                |change, from: Option<&Package>, to: Option<&Package>| PackageChange {
                    change,
                    ecosystem: *eco,
                    site: site.path.to_string(),
                    from: from.cloned(),
                    to: to.cloned(),
                };
            if let ([to], [from]) = (added.as_slice(), removed.as_slice()) {
                let kind = if from.version == to.version {
                    PackageChangeKind::Changed
                } else {
                    match compare_versions(from, to) {
                        Some(std::cmp::Ordering::Less) => PackageChangeKind::Upgraded,
                        Some(std::cmp::Ordering::Greater) => {
                            PackageChangeKind::Downgraded
                        }
                        _ => PackageChangeKind::Changed,
                    }
                };
                packages.push(change(kind, Some(from), Some(to)));
            } else {
                for p in removed {
                    packages.push(change(PackageChangeKind::Removed, Some(p), None));
                }
                for p in added {
                    packages.push(change(PackageChangeKind::Added, None, Some(p)));
                }
            }
        }

        let mut hooks = Vec::new();
        if let (Some(hb), Some(hn)) = (&base.site_to_hooks, &now.site_to_hooks) {
            let sites: BTreeSet<&Site> = hb.keys().chain(hn.keys()).collect();
            for site in sites {
                let set = |m: &HashMap<Site, Vec<Hook>>| -> BTreeSet<Hook> {
                    m.get(site)
                        .map(|v| v.iter().cloned().collect())
                        .unwrap_or_default()
                };
                let (b, n) = (set(hb), set(hn));
                let (added, removed) = added_removed(&b, &n);
                let change = |change, hook: &Hook| HookChange {
                    change,
                    ecosystem: site.ecosystem,
                    site: site.path.to_string(),
                    hook: hook.clone(),
                };
                hooks.extend(added.into_iter().map(|h| change(Change::Added, h)));
                hooks.extend(removed.into_iter().map(|h| change(Change::Removed, h)));
            }
        }
        ScanDiff {
            origins,
            sites,
            packages,
            hooks,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.origins.is_empty()
            && self.sites.is_empty()
            && self.packages.is_empty()
            && self.hooks.is_empty()
    }

    fn count(&self, kind: PackageChangeKind) -> usize {
        self.packages.iter().filter(|p| p.change == kind).count()
    }
}

//------------------------------------------------------------------------------
// evaluation

/// The vulnerability IDs of one package, as stored with a baseline.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PackageVulns {
    pub package: Package,
    pub ids: Vec<String>,
}

/// The audit of a scan: IDs per package, and the details of each ID.
#[derive(Debug, Clone, Default)]
pub(crate) struct Audit {
    pub(crate) vulns: Vec<PackageVulns>,
    pub(crate) infos: HashMap<String, VulnInfo>,
}

impl Audit {
    pub(crate) fn from_report(report: crate::audit_report::AuditReport) -> Self {
        let mut audit = Audit::default();
        for r in report.records {
            if r.vuln_ids.is_empty() {
                continue;
            }
            let mut ids = r.vuln_ids;
            ids.sort();
            ids.dedup();
            audit.infos.extend(r.vuln_infos);
            audit.vulns.push(PackageVulns {
                package: r.package,
                ids,
            });
        }
        audit.vulns.sort_by(|a, b| a.package.cmp(&b.package));
        audit
    }
}

/// Why a vulnerability is new.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VulnSource {
    /// Its package was added or changed since the baseline.
    Introduced,
    /// Its package was present at the baseline; the advisory is new.
    Published,
    /// There is no baseline audit to compare to (a new baseline).
    Existing,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct VulnFinding {
    pub package: Package,
    pub id: String,
    pub source: VulnSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    pub url: String,
    /// The paths of the sites holding the package in the scan reported, sorted (omitted if none are known).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sites: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct VulnResolved {
    pub package: Package,
    pub id: String,
    /// The paths of the sites that held the package in the baseline, sorted (omitted if none are known).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sites: Vec<String>,
}

/// The paths of the sites holding `package` in `scan`, sorted and unique.
fn package_sites(scan: &ScanFS, package: &Package) -> Vec<String> {
    let mut sites: Vec<String> = scan
        .package_to_sites
        .get(package)
        .map(|ss| ss.iter().map(|s| s.path.to_string()).collect())
        .unwrap_or_default();
    sites.sort();
    sites.dedup();
    sites
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// New vulnerabilities, new hooks, downgrades, or new allow-list violations.
    Alert,
    /// Other changes.
    Changed,
    Unchanged,
}

/// The baseline a scan is compared to.
pub(crate) struct BaselineRef<'a> {
    pub(crate) scan: &'a ScanFS,
    pub(crate) time: Duration,
    /// None if the baseline's audit was unavailable.
    pub(crate) vulns: Option<&'a [PackageVulns]>,
    /// None if the baseline was not validated.
    pub(crate) violations: Option<&'a [ValidationDigestRecord]>,
}

/// The evaluated changes since a baseline.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct DiffReport {
    pub status: Status,
    /// Time (since the Unix epoch) of the baseline; absent when this report establishes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_time: Option<Duration>,
    /// Time of the scan reported.
    pub time: Duration,
    pub changes: ScanDiff,
    #[serde(default)]
    pub new_vulns: Vec<VulnFinding>,
    #[serde(default)]
    pub resolved_vulns: Vec<VulnResolved>,
    /// Present if the vulnerability audit failed; vulnerabilities were not compared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_error: Option<String>,
    /// Allow-list violations not in the baseline (only with a bound).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub new_violations: Vec<ValidationDigestRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved_violations: Vec<ValidationDigestRecord>,
}

impl DiffReport {
    /// Evaluate `now` against a baseline (None establishes one: everything found is reported as existing).
    pub(crate) fn new(
        base: Option<&BaselineRef>,
        now: &ScanFS,
        time: Duration,
        audit: Result<&Audit, String>,
        violations: Option<&[ValidationDigestRecord]>,
    ) -> Self {
        let changes = base.map(|b| ScanDiff::new(b.scan, now)).unwrap_or_default();

        let (mut new_vulns, mut resolved_vulns, audit_error) =
            (Vec::new(), Vec::new(), audit.as_ref().err().cloned());
        if let Ok(audit) = audit {
            let base_vulns = base.and_then(|b| b.vulns);
            // an advisory is matched by package name, not version: one that still affects an upgraded package is neither new nor resolved
            let ids_of = |vs: &[PackageVulns]| -> HashSet<(Ecosystem, String, String)> {
                vs.iter()
                    .flat_map(|pv| pv.ids.iter().map(|id| vuln_key(&pv.package, id)))
                    .collect()
            };
            let base_ids = base_vulns.map(ids_of).unwrap_or_default();
            let base_audited: HashSet<&Package> = base_vulns
                .into_iter()
                .flatten()
                .map(|pv| &pv.package)
                .collect();
            for pv in &audit.vulns {
                for id in &pv.ids {
                    if base_ids.contains(&vuln_key(&pv.package, id)) {
                        continue;
                    }
                    let source = match (base, base_vulns) {
                        (Some(b), Some(_))
                            if b.scan.package_to_sites.contains_key(&pv.package)
                                || base_audited.contains(&pv.package) =>
                        {
                            VulnSource::Published
                        }
                        (Some(_), Some(_)) => VulnSource::Introduced,
                        _ => VulnSource::Existing,
                    };
                    let info = audit.infos.get(id);
                    new_vulns.push(VulnFinding {
                        package: pv.package.clone(),
                        id: id.clone(),
                        source,
                        summary: info.and_then(|i| i.summary.clone()),
                        severity: info
                            .and_then(|i| i.cvss_details.as_ref())
                            .map(|c| c.get_prime()),
                        score: info
                            .and_then(|i| i.cvss_details.as_ref())
                            .and_then(|c| c.get_max_score()),
                        url: format!("https://osv.dev/vulnerability/{id}"),
                        sites: package_sites(now, &pv.package),
                    });
                }
            }
            if let Some(bv) = base_vulns {
                let now_ids = ids_of(&audit.vulns);
                for pv in bv {
                    for id in &pv.ids {
                        if !now_ids.contains(&vuln_key(&pv.package, id)) {
                            resolved_vulns.push(VulnResolved {
                                package: pv.package.clone(),
                                id: id.clone(),
                                sites: base
                                    .map(|b| package_sites(b.scan, &pv.package))
                                    .unwrap_or_default(),
                            });
                        }
                    }
                }
            }
        }

        let (mut new_violations, mut resolved_violations) = (Vec::new(), Vec::new());
        if let Some(now_v) = violations {
            let now_set: BTreeSet<&ValidationDigestRecord> = now_v.iter().collect();
            let base_set: BTreeSet<&ValidationDigestRecord> = base
                .and_then(|b| b.violations)
                .into_iter()
                .flatten()
                .collect();
            new_violations = now_set
                .difference(&base_set)
                .map(|v| (*v).clone())
                .collect();
            if base.and_then(|b| b.violations).is_some() {
                resolved_violations = base_set
                    .difference(&now_set)
                    .map(|v| (*v).clone())
                    .collect();
            }
        }

        let mut report = DiffReport {
            status: Status::Unchanged,
            baseline_time: base.map(|b| b.time),
            time,
            changes,
            new_vulns,
            resolved_vulns,
            audit_error,
            new_violations,
            resolved_violations,
        };
        report.status = report.evaluate();
        report
    }

    fn evaluate(&self) -> Status {
        let alert = !self.new_vulns.is_empty()
            || !self.new_violations.is_empty()
            || self.changes.hooks.iter().any(|h| h.change == Change::Added)
            || self.changes.count(PackageChangeKind::Downgraded) > 0;
        if alert {
            Status::Alert
        } else if !self.changes.is_empty()
            || !self.resolved_vulns.is_empty()
            || !self.resolved_violations.is_empty()
            || self.audit_error.is_some()
        {
            Status::Changed
        } else {
            Status::Unchanged
        }
    }

    pub fn is_alert(&self) -> bool {
        self.status == Status::Alert
    }

    /// A plain-text summary (for terminals, logs, and mail sent by cron), alerts first. Package changes beyond `MAX_LINES` are counted, not listed.
    pub fn to_text(&self) -> String {
        const MAX_LINES: usize = 100;
        let mut s = String::new();
        let status = match self.status {
            Status::Alert => "ALERT",
            Status::Changed => "changed",
            Status::Unchanged => "unchanged",
        };
        let when = |d: &Duration| rfc3339(d.as_secs());
        match &self.baseline_time {
            Some(b) => {
                let _ = writeln!(
                    s,
                    "fetter monitor-diff: {status} ({} to {})",
                    when(b),
                    when(&self.time)
                );
            }
            None => {
                let _ = writeln!(
                    s,
                    "fetter monitor-diff: {status} (baseline established {})",
                    when(&self.time)
                );
            }
        }
        let section = |s: &mut String, title: &str, lines: Vec<String>| {
            if lines.is_empty() {
                return;
            }
            let _ = writeln!(s, "\n{title} ({}):", lines.len());
            let n = lines.len();
            for l in lines.into_iter().take(MAX_LINES) {
                let _ = writeln!(s, "  {l}");
            }
            if n > MAX_LINES {
                let _ = writeln!(s, "  ... and {} more", n - MAX_LINES);
            }
        };
        let pkg = |p: &Package| format!("{} {}", p.ecosystem, p);
        section(
            &mut s,
            "New vulnerabilities",
            self.new_vulns
                .iter()
                .map(|v| {
                    let source = match v.source {
                        VulnSource::Introduced => "introduced",
                        VulnSource::Published => "published",
                        VulnSource::Existing => "existing",
                    };
                    let mut l = format!("[{source}] {} {}", pkg(&v.package), v.id);
                    if let Some(score) = v.score {
                        let _ = write!(l, " (CVSS {score:.1})");
                    }
                    if let Some(summary) = &v.summary {
                        let _ = write!(l, ": {summary}");
                    }
                    l
                })
                .collect(),
        );
        section(
            &mut s,
            "New hooks",
            self.changes
                .hooks
                .iter()
                .filter(|h| h.change == Change::Added)
                .map(|h| {
                    format!(
                        "{} {}: {}: {}",
                        h.ecosystem, h.site, h.hook.name, h.hook.detail
                    )
                })
                .collect(),
        );
        let version_change = |p: &PackageChange| {
            let name =
                p.to.as_ref()
                    .or(p.from.as_ref())
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
            let v = |p: &Option<Package>| {
                p.as_ref()
                    .map(|p| p.version.to_string())
                    .unwrap_or_default()
            };
            format!(
                "{} {}: {} {} -> {}",
                p.ecosystem,
                p.site,
                name,
                v(&p.from),
                v(&p.to)
            )
        };
        section(
            &mut s,
            "Downgrades",
            self.changes
                .packages
                .iter()
                .filter(|p| p.change == PackageChangeKind::Downgraded)
                .map(version_change)
                .collect(),
        );
        section(
            &mut s,
            "New allow-list violations",
            self.new_violations.iter().map(violation_line).collect(),
        );
        if let Some(e) = &self.audit_error {
            let _ = writeln!(s, "\nVulnerability audit unavailable: {e}");
        }
        section(
            &mut s,
            "Package changes",
            self.changes
                .packages
                .iter()
                .filter(|p| p.change != PackageChangeKind::Downgraded)
                .map(|p| match p.change {
                    PackageChangeKind::Added => format!(
                        "+ {} {}: {}",
                        p.ecosystem,
                        p.site,
                        p.to.as_ref().map(|p| p.to_string()).unwrap_or_default()
                    ),
                    PackageChangeKind::Removed => format!(
                        "- {} {}: {}",
                        p.ecosystem,
                        p.site,
                        p.from.as_ref().map(|p| p.to_string()).unwrap_or_default()
                    ),
                    PackageChangeKind::Upgraded => format!("^ {}", version_change(p)),
                    _ => format!("~ {}", version_change(p)),
                })
                .collect(),
        );
        section(
            &mut s,
            "Removed hooks",
            self.changes
                .hooks
                .iter()
                .filter(|h| h.change == Change::Removed)
                .map(|h| format!("{} {}: {}", h.ecosystem, h.site, h.hook.name))
                .collect(),
        );
        let sign = |c: Change| if c == Change::Added { "+" } else { "-" };
        section(
            &mut s,
            "Sites",
            self.changes
                .sites
                .iter()
                .map(|c| {
                    format!(
                        "{} {} {} {}",
                        sign(c.change),
                        c.ecosystem,
                        c.kind.as_str(),
                        c.path
                    )
                })
                .collect(),
        );
        section(
            &mut s,
            "Origins",
            self.changes
                .origins
                .iter()
                .map(|c| format!("{} {} {}", sign(c.change), c.ecosystem, c.path))
                .collect(),
        );
        section(
            &mut s,
            "Resolved vulnerabilities",
            self.resolved_vulns
                .iter()
                .map(|v| format!("{} {}", pkg(&v.package), v.id))
                .collect(),
        );
        section(
            &mut s,
            "Resolved allow-list violations",
            self.resolved_violations
                .iter()
                .map(violation_line)
                .collect(),
        );
        s
    }
}

fn vuln_key(package: &Package, id: &str) -> (Ecosystem, String, String) {
    (package.ecosystem, package.key.clone(), id.to_string())
}

fn violation_line(v: &ValidationDigestRecord) -> String {
    let j = serde_json::to_value(v).unwrap_or_default();
    let field = |k: &str| j.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let mut l = format!("{} {}", field("ecosystem"), field("explain"));
    for k in ["package", "dependency"] {
        let f = field(k);
        if !f.is_empty() {
            let _ = write!(l, " {k} {f}");
        }
    }
    l
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::path_shared::PathShared;
    use crate::util::ScanConfig;
    use crate::version_spec::VersionSpec;

    fn pkg(eco: Ecosystem, name: &str, version: &str) -> Package {
        Package::new(eco, name, VersionSpec::new(version), None)
    }

    fn py_site() -> Site {
        Site::python(PathShared::from("/env/lib/python3.12/site-packages"))
    }

    fn npm_site() -> Site {
        Site::new(
            Ecosystem::Npm,
            SiteKind::Installed,
            PathShared::from("/proj/node_modules"),
        )
    }

    fn scan(packages: &[(Package, Site)], hooks: Option<Vec<(Site, Hook)>>) -> ScanFS {
        let mut origin_to_sites: HashMap<Origin, Vec<Site>> = HashMap::new();
        let mut package_to_sites: HashMap<Package, Vec<Site>> = HashMap::new();
        for (p, s) in packages {
            let origin = Origin::new(
                s.ecosystem,
                PathShared::from(format!("/origin/{}", s.ecosystem)),
            );
            origin_to_sites.entry(origin).or_default().push(s.clone());
            package_to_sites
                .entry(p.clone())
                .or_default()
                .push(s.clone());
        }
        let mut sfs = ScanFS::from_parts(
            origin_to_sites,
            package_to_sites,
            ScanConfig::new(false, false),
            "hash".to_string(),
        );
        sfs.site_to_hooks = hooks.map(|hs| {
            let mut m: HashMap<Site, Vec<Hook>> = HashMap::new();
            for (s, h) in hs {
                m.entry(s).or_default().push(h);
            }
            m
        });
        sfs
    }

    fn hook(name: &str) -> Hook {
        Hook {
            name: name.to_string(),
            detail: "node install.js".to_string(),
        }
    }

    #[test]
    fn test_scan_diff_a() {
        let base = scan(
            &[
                (pkg(Ecosystem::Python, "numpy", "2.1.0"), py_site()),
                (pkg(Ecosystem::Python, "requests", "2.31.0"), py_site()),
                (pkg(Ecosystem::Python, "six", "1.16.0"), py_site()),
                (pkg(Ecosystem::Npm, "lodash", "4.17.20"), npm_site()),
            ],
            Some(vec![]),
        );
        let now = scan(
            &[
                (pkg(Ecosystem::Python, "numpy", "1.26.4"), py_site()),
                (pkg(Ecosystem::Python, "requests", "2.32.3"), py_site()),
                (pkg(Ecosystem::Python, "idna", "3.7"), py_site()),
                (pkg(Ecosystem::Npm, "lodash", "4.17.21"), npm_site()),
            ],
            Some(vec![(npm_site(), hook("lodash postinstall"))]),
        );
        let d = ScanDiff::new(&base, &now);
        let kinds: Vec<(PackageChangeKind, String)> = d
            .packages
            .iter()
            .map(|p| {
                let name = p.to.as_ref().or(p.from.as_ref()).unwrap().name.clone();
                (p.change, name)
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                (PackageChangeKind::Added, "idna".to_string()),
                (PackageChangeKind::Downgraded, "numpy".to_string()),
                (PackageChangeKind::Upgraded, "requests".to_string()),
                (PackageChangeKind::Removed, "six".to_string()),
                (PackageChangeKind::Upgraded, "lodash".to_string()),
            ]
        );
        assert_eq!(d.hooks.len(), 1);
        assert_eq!(d.hooks[0].change, Change::Added);
        assert!(d.origins.is_empty() && d.sites.is_empty());
        assert!(ScanDiff::new(&now, &now).is_empty());
    }

    #[test]
    fn test_scan_diff_sites_a() {
        let gha = Site::new(
            Ecosystem::Gha,
            SiteKind::Manifest,
            PathShared::from("/repo/.github/workflows/ci.yml"),
        );
        let base = scan(
            &[(pkg(Ecosystem::Gha, "actions/checkout", "v4"), gha.clone())],
            None,
        );
        let now = scan(
            &[
                (pkg(Ecosystem::Gha, "actions/checkout", "v5"), gha.clone()),
                (pkg(Ecosystem::Python, "six", "1.16.0"), py_site()),
            ],
            Some(vec![(py_site(), hook("x.pth"))]),
        );
        let d = ScanDiff::new(&base, &now);
        // refs cannot be ordered
        assert_eq!(d.packages[1].change, PackageChangeKind::Changed);
        assert_eq!(d.sites.len(), 1);
        assert_eq!(d.sites[0].change, Change::Added);
        assert_eq!(d.origins.len(), 1);
        // hooks are compared only when both scans have them
        assert!(d.hooks.is_empty());
    }

    #[test]
    fn test_diff_report_vulns_a() {
        let requests = pkg(Ecosystem::Python, "requests", "2.31.0");
        let idna = pkg(Ecosystem::Python, "idna", "3.6");
        let base = scan(&[(requests.clone(), py_site())], Some(vec![]));
        let now = scan(
            &[(requests.clone(), py_site()), (idna.clone(), py_site())],
            Some(vec![]),
        );
        let base_vulns = vec![PackageVulns {
            package: requests.clone(),
            ids: vec!["GHSA-old".into(), "GHSA-fixed".into()],
        }];
        let audit = Audit {
            vulns: vec![
                PackageVulns {
                    package: idna.clone(),
                    ids: vec!["GHSA-idna".into()],
                },
                PackageVulns {
                    package: requests.clone(),
                    ids: vec!["GHSA-new".into(), "GHSA-old".into()],
                },
            ],
            infos: HashMap::new(),
        };
        let b = BaselineRef {
            scan: &base,
            time: Duration::from_secs(100),
            vulns: Some(&base_vulns),
            violations: None,
        };
        let r =
            DiffReport::new(Some(&b), &now, Duration::from_secs(200), Ok(&audit), None);
        assert_eq!(r.status, Status::Alert);
        let found: Vec<(&str, VulnSource)> = r
            .new_vulns
            .iter()
            .map(|v| (v.id.as_str(), v.source))
            .collect();
        assert_eq!(
            found,
            vec![
                ("GHSA-idna", VulnSource::Introduced),
                ("GHSA-new", VulnSource::Published)
            ]
        );
        assert_eq!(r.resolved_vulns.len(), 1);
        assert_eq!(r.resolved_vulns[0].id, "GHSA-fixed");
        // where each is: the sites holding the package, now for new and at the baseline for resolved
        let site = "/env/lib/python3.12/site-packages".to_string();
        assert!(r.new_vulns.iter().all(|v| v.sites == vec![site.clone()]));
        assert_eq!(r.resolved_vulns[0].sites, vec![site.clone()]);
        let j = serde_json::to_value(&r.new_vulns[0]).unwrap();
        assert_eq!(j["sites"], serde_json::json!([site]));

        // an advisory that still affects an upgraded package is neither new nor resolved
        let upgraded = pkg(Ecosystem::Python, "requests", "2.32.0");
        let now = scan(&[(upgraded.clone(), py_site())], Some(vec![]));
        let audit = Audit {
            vulns: vec![PackageVulns {
                package: upgraded,
                ids: vec!["GHSA-old".into()],
            }],
            infos: HashMap::new(),
        };
        let r2 =
            DiffReport::new(Some(&b), &now, Duration::from_secs(200), Ok(&audit), None);
        assert!(r2.new_vulns.is_empty());
        assert_eq!(r2.resolved_vulns.len(), 1);
        assert_eq!(r2.status, Status::Changed);
        let text = r.to_text();
        assert!(
            text.contains("[introduced] python idna-3.6 GHSA-idna"),
            "{text}"
        );
        assert!(
            text.contains("+ python /env/lib/python3.12/site-packages: idna-3.6"),
            "{text}"
        );

        // a new baseline: all found are existing
        let r = DiffReport::new(None, &now, Duration::from_secs(200), Ok(&audit), None);
        assert!(r.new_vulns.iter().all(|v| v.source == VulnSource::Existing));
        assert!(r.changes.is_empty() && r.resolved_vulns.is_empty());
        assert!(r.to_text().contains("baseline established"));
    }

    #[test]
    fn test_diff_report_vuln_sites_a() {
        let lodash = pkg(Ecosystem::Npm, "lodash", "4.17.15");
        let other = Site::new(
            Ecosystem::Npm,
            SiteKind::Installed,
            PathShared::from("/a/node_modules"),
        );
        // one package at two sites (and listed twice at one): sorted, unique
        let now = scan(
            &[
                (lodash.clone(), npm_site()),
                (lodash.clone(), other.clone()),
                (lodash.clone(), npm_site()),
            ],
            Some(vec![]),
        );
        let audit = Audit {
            vulns: vec![PackageVulns {
                package: lodash.clone(),
                ids: vec!["GHSA-x".into()],
            }],
            infos: HashMap::new(),
        };
        let r = DiffReport::new(None, &now, Duration::from_secs(1), Ok(&audit), None);
        assert_eq!(
            r.new_vulns[0].sites,
            vec![
                "/a/node_modules".to_string(),
                "/proj/node_modules".to_string()
            ]
        );

        // no known sites: the field is omitted, and JSON without it still reads
        let mut v = r.new_vulns[0].clone();
        v.sites.clear();
        let j = serde_json::to_value(&v).unwrap();
        assert!(j.get("sites").is_none());
        let back: VulnFinding = serde_json::from_value(j).unwrap();
        assert!(back.sites.is_empty());
        let resolved: VulnResolved = serde_json::from_value(serde_json::json!({
            "package": serde_json::to_value(&lodash).unwrap(), "id": "GHSA-x"}))
        .unwrap();
        assert!(resolved.sites.is_empty());
    }

    #[test]
    fn test_diff_report_status_a() {
        let six = pkg(Ecosystem::Python, "six", "1.16.0");
        let base = scan(&[(six.clone(), py_site())], Some(vec![]));
        let b = BaselineRef {
            scan: &base,
            time: Duration::from_secs(1),
            vulns: Some(&[]),
            violations: None,
        };
        let audit = Audit::default();
        let r =
            DiffReport::new(Some(&b), &base, Duration::from_secs(2), Ok(&audit), None);
        assert_eq!(r.status, Status::Unchanged);

        let now = scan(
            &[(pkg(Ecosystem::Python, "six", "1.17.0"), py_site())],
            Some(vec![]),
        );
        let r = DiffReport::new(Some(&b), &now, Duration::from_secs(2), Ok(&audit), None);
        assert_eq!(r.status, Status::Changed);

        // an audit failure is reported, not an alert
        let r = DiffReport::new(
            Some(&b),
            &base,
            Duration::from_secs(2),
            Err("offline".into()),
            None,
        );
        assert_eq!(r.status, Status::Changed);
        assert!(r.to_text().contains("unavailable: offline"));

        // a new hook is an alert
        let now = scan(
            &[(six.clone(), py_site())],
            Some(vec![(py_site(), hook("evil.pth"))]),
        );
        let r = DiffReport::new(Some(&b), &now, Duration::from_secs(2), Ok(&audit), None);
        assert_eq!(r.status, Status::Alert);
        assert!(r.to_text().contains("New hooks (1)"));
    }

    #[test]
    fn test_diff_report_json_a() {
        let base = scan(
            &[(pkg(Ecosystem::Python, "six", "1.16.0"), py_site())],
            Some(vec![]),
        );
        let now = scan(
            &[(pkg(Ecosystem::Python, "six", "1.15.0"), py_site())],
            Some(vec![]),
        );
        let b = BaselineRef {
            scan: &base,
            time: Duration::from_secs(1),
            vulns: Some(&[]),
            violations: None,
        };
        let r = DiffReport::new(
            Some(&b),
            &now,
            Duration::from_secs(2),
            Ok(&Audit::default()),
            None,
        );
        assert_eq!(r.status, Status::Alert);
        let j = serde_json::to_string(&r).unwrap();
        assert!(j.contains(r#""change":"downgraded""#), "{j}");
        let back: DiffReport = serde_json::from_str(&j).unwrap();
        assert_eq!(back, r);
    }
}
