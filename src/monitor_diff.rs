//! `monitor-diff`: rescan one system on a schedule, compare each scan to a baseline, and report what changed and whether it matters (see `diff_report`).
//!
//! Output is either text (one report per report period) or a versioned JSON Lines event stream (`Event`), meant to be consumed by separate programs (a live view, a notifier): each line is one JSON object with `schema`, `event`, `time` and `system`; readers should ignore events and fields they do not know.
//!
//! The rolling baseline is stored in the cache directory, per scan configuration, and advanced only after a report is written; a fixed baseline (`--baseline FILE`, written by `monitor-diff save`) is never written.

use std::collections::HashSet;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use serde::Deserialize;
use serde::Serialize;

use crate::allow_list::AllowList;
use crate::audit_report::AuditReport;
use crate::cli::CvssFilter;
use crate::diff_report::Audit;
use crate::diff_report::BaselineRef;
use crate::diff_report::DiffReport;
use crate::diff_report::PackageVulns;
use crate::diff_report::ScanDiff;
use crate::diff_report::Status;
use crate::ecosystem::Ecosystem;
use crate::scan_fs::ScanFS;
use crate::system_tag::SystemTag;
use crate::transmit::SCHEMA;
use crate::ureq_client::UreqClient;
use crate::util::hash_paths;
use crate::util::logger;
use crate::util::CacheConfig;
use crate::util::FlagCacheRefresh;
use crate::util::FlagLog;
use crate::util::FlagRetainPassing;
use crate::util::ResultDynError;
use crate::util::ScanConfig;
use crate::validation_report::ValidationDigestRecord;
use crate::validation_report::ValidationFlags;

//------------------------------------------------------------------------------
// state

/// A baseline: a scan with its audit and (optionally) validation.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct DiffState {
    pub(crate) schema: u32,
    #[serde(default)]
    pub(crate) fetter_version: Option<String>,
    /// Time (since the Unix epoch) of the scan.
    pub(crate) time: Duration,
    /// The schema 1 scan object.
    pub(crate) scan: ScanFS,
    /// None if the audit was unavailable.
    #[serde(default)]
    pub(crate) vulns: Option<Vec<PackageVulns>>,
    /// None if not validated against a bound.
    #[serde(default)]
    pub(crate) violations: Option<Vec<ValidationDigestRecord>>,
}

impl DiffState {
    fn as_ref(&self) -> BaselineRef<'_> {
        BaselineRef {
            scan: &self.scan,
            time: self.time,
            vulns: self.vulns.as_deref(),
            violations: self.violations.as_deref(),
        }
    }

    pub(crate) fn read(path: &Path) -> ResultDynError<Self> {
        let json = fs::read_to_string(path)
            .map_err(|e| format!("Cannot read baseline {}: {e}", path.display()))?;
        serde_json::from_str(&json)
            .map_err(|e| format!("Cannot parse baseline {}: {e}", path.display()).into())
    }

    /// Write via a temporary file and rename, so a reader never sees a partial baseline.
    pub(crate) fn write(&self, path: &Path) -> ResultDynError<()> {
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string(self)?)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// The rolling baseline's path for a scan configuration.
pub(crate) fn rolling_path(cache_dir: &Path, origins_hash: &str) -> PathBuf {
    cache_dir.join(format!("monitor_diff_v1_{origins_hash}.json"))
}

//------------------------------------------------------------------------------
// events

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum EventBody {
    /// The configuration, once at start.
    Start {
        /// `rolling`, or the path of a fixed baseline.
        baseline: String,
        /// Time of the baseline, if there is one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        baseline_time: Option<Duration>,
        period: u64,
        report_period: u64,
        ecosystems: Vec<Ecosystem>,
    },
    /// After each scan: counts, and the changes since the previous scan (not the baseline).
    Scan {
        origins: usize,
        sites: usize,
        packages: usize,
        changes: ScanDiff,
    },
    /// The evaluated changes since the baseline.
    Report { report: DiffReport },
    /// A scan or report failed; monitoring continues.
    Error { message: String },
    /// An event of a newer fetter.
    #[serde(other)]
    Unknown,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Event {
    pub schema: u32,
    pub time: Duration,
    pub system: SystemTag,
    #[serde(flatten)]
    pub body: EventBody,
}

//------------------------------------------------------------------------------
// options and output

#[derive(Debug, Clone)]
pub(crate) enum DiffOutput {
    /// JSON Lines, to stdout or appended to a file.
    Json(Option<PathBuf>),
    Text,
}

/// An allow list to validate against, with the `validate` options.
pub(crate) struct DiffBound {
    pub(crate) allow_list: AllowList,
    pub(crate) flags: ValidationFlags,
    pub(crate) ignore: HashSet<String>,
    pub(crate) strict: bool,
}

pub(crate) struct DiffOptions {
    pub(crate) period: u64,
    pub(crate) report_period: u64,
    pub(crate) baseline: Option<PathBuf>,
    pub(crate) report_always: bool,
    pub(crate) bound: Option<DiffBound>,
    pub(crate) cvss: CvssFilter,
    pub(crate) output: DiffOutput,
}

enum Sink {
    Stdout,
    File(File),
    Text,
}

impl Sink {
    fn new(output: &DiffOutput) -> io::Result<Self> {
        Ok(match output {
            DiffOutput::Json(None) => Sink::Stdout,
            DiffOutput::Json(Some(p)) => {
                Sink::File(OpenOptions::new().create(true).append(true).open(p)?)
            }
            DiffOutput::Text => Sink::Text,
        })
    }

    fn emit(&mut self, event: &Event) -> io::Result<()> {
        let line = || -> io::Result<String> {
            serde_json::to_string(event).map_err(io::Error::other)
        };
        match self {
            Sink::Stdout => {
                let mut out = io::stdout().lock();
                writeln!(out, "{}", line()?)?;
                out.flush()
            }
            Sink::File(f) => {
                writeln!(f, "{}", line()?)?;
                f.flush()
            }
            Sink::Text => match &event.body {
                EventBody::Report { report } => {
                    let mut out = io::stdout().lock();
                    writeln!(out, "{}", report.to_text())?;
                    out.flush()
                }
                EventBody::Error { message } => {
                    eprintln!("fetter monitor-diff: {message}");
                    Ok(())
                }
                _ => Ok(()),
            },
        }
    }
}

fn now() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

//------------------------------------------------------------------------------

struct Monitor<'a> {
    origin_paths: &'a Vec<PathBuf>,
    client: Arc<dyn UreqClient>,
    config: &'a ScanConfig,
    cache_config: &'a CacheConfig,
    opts: &'a DiffOptions,
    log: FlagLog,
}

impl Monitor<'_> {
    fn scan(&self) -> ResultDynError<ScanFS> {
        let mut sfs =
            ScanFS::from_origin_paths(self.origin_paths, self.config, self.log)?;
        sfs.load_hooks();
        Ok(sfs)
    }

    /// Audit every package; the OSV caches keep repeated audits cheap.
    fn audit(&self, sfs: &ScanFS) -> Result<Audit, String> {
        AuditReport::from_packages(
            self.client.clone(),
            &sfs.get_packages(),
            FlagCacheRefresh(false),
            self.cache_config,
            self.log,
            self.opts.cvss.clone(),
            FlagRetainPassing(false),
        )
        .map(Audit::from_report)
        .map_err(|e| e.to_string())
    }

    fn violations(&self, sfs: &mut ScanFS) -> Option<Vec<ValidationDigestRecord>> {
        self.opts.bound.as_ref().map(|b| {
            sfs.to_allow_list_validation(
                &b.allow_list,
                b.flags.clone(),
                Some(&b.ignore),
                b.strict,
                self.log,
            )
            .to_validation_digest()
        })
    }

    /// A baseline of a scan.
    fn state(
        &self,
        mut sfs: ScanFS,
        prev_vulns: Option<Vec<PackageVulns>>,
    ) -> (DiffState, Result<Audit, String>) {
        let audit = self.audit(&sfs);
        let violations = self.violations(&mut sfs);
        let vulns = match &audit {
            Ok(a) => Some(a.vulns.clone()),
            Err(_) => prev_vulns,
        };
        (
            DiffState {
                schema: SCHEMA,
                fetter_version: Some(env!("CARGO_PKG_VERSION").to_string()),
                time: now(),
                scan: sfs,
                vulns,
                violations,
            },
            audit,
        )
    }
}

/// Write a baseline of the current system to `path`.
pub(crate) fn monitor_diff_save(
    origin_paths: &Vec<PathBuf>,
    client: Arc<dyn UreqClient>,
    config: &ScanConfig,
    cache_config: &CacheConfig,
    opts: &DiffOptions,
    path: &Path,
    log: FlagLog,
) -> ResultDynError<()> {
    let m = Monitor {
        origin_paths,
        client,
        config,
        cache_config,
        opts,
        log,
    };
    let (state, audit) = m.state(m.scan()?, None);
    if let Err(e) = audit {
        eprintln!("Vulnerability audit unavailable, saving baseline without it: {e}");
    }
    state.write(path)
}

/// Run monitoring; with a period of 0, scan and report once. Returns true if the last report was an alert.
pub(crate) fn monitor_diff_loop(
    origin_paths: &Vec<PathBuf>,
    client: Arc<dyn UreqClient>,
    config: &ScanConfig,
    cache_config: &CacheConfig,
    opts: &DiffOptions,
    log: FlagLog,
) -> ResultDynError<bool> {
    let system = SystemTag::from_system()
        .map_err(|e| format!("Cannot read system details: {e}"))?;
    let event = |body: EventBody| Event {
        schema: SCHEMA,
        time: now(),
        system: system.clone(),
        body,
    };
    let rolling = match &opts.baseline {
        Some(_) => None,
        None => Some(rolling_path(
            &cache_config.directory,
            &hash_paths(origin_paths, config),
        )),
    };
    let mut baseline: Option<DiffState> = match (&opts.baseline, &rolling) {
        (Some(p), _) => Some(DiffState::read(p)?),
        (None, Some(p)) if p.exists() => match DiffState::read(p) {
            Ok(s) => Some(s),
            Err(e) => {
                logger!(log, module_path!(), "Ignoring unreadable baseline: {}", e);
                None
            }
        },
        _ => None,
    };
    logger!(
        log,
        module_path!(),
        "Baseline: {:?}",
        rolling.as_ref().or(opts.baseline.as_ref())
    );

    let m = Monitor {
        origin_paths,
        client,
        config,
        cache_config,
        opts,
        log,
    };
    let mut sink = Sink::new(&opts.output)?;
    sink.emit(&event(EventBody::Start {
        baseline: opts
            .baseline
            .as_ref()
            .map_or("rolling".to_string(), |p| p.display().to_string()),
        baseline_time: baseline.as_ref().map(|b| b.time),
        period: opts.period,
        report_period: opts.report_period,
        ecosystems: config.ecosystems.clone(),
    }))?;

    let mut prev: Option<ScanFS> = None;
    let mut last_report: Option<Instant> = None;
    let mut alert = false;
    loop {
        let started = Instant::now();
        match m.scan() {
            Err(e) => {
                let message = format!("Scan failed: {e}");
                if opts.period == 0 {
                    return Err(message.into());
                }
                sink.emit(&event(EventBody::Error { message }))?;
            }
            Ok(sfs) => {
                let changes = prev
                    .as_ref()
                    .map(|p| ScanDiff::new(p, &sfs))
                    .unwrap_or_default();
                sink.emit(&event(EventBody::Scan {
                    origins: sfs.origin_to_sites.len(),
                    sites: sfs.site_to_origins.len(),
                    packages: sfs.package_to_sites.len(),
                    changes,
                }))?;
                let due = last_report.is_none_or(|t| {
                    t.elapsed() >= Duration::from_secs(opts.report_period)
                });
                if due {
                    last_report = Some(started);
                    let prev_vulns = baseline.as_ref().and_then(|b| b.vulns.clone());
                    let (state, audit) = m.state(sfs.clone(), prev_vulns);
                    let report = DiffReport::new(
                        baseline.as_ref().map(|b| b.as_ref()).as_ref(),
                        &state.scan,
                        state.time,
                        audit.as_ref().map_err(|e| e.clone()),
                        state.violations.as_deref(),
                    );
                    alert = report.is_alert();
                    let established = baseline.is_none();
                    let changed = report.status != Status::Unchanged || established;
                    if changed || opts.report_always {
                        // a failed write ends monitoring without advancing the baseline
                        sink.emit(&event(EventBody::Report { report }))?;
                    }
                    if let Some(path) = rolling.as_ref().filter(|_| changed) {
                        state.write(path)?;
                        baseline = Some(state);
                    }
                }
                prev = Some(sfs);
            }
        }
        if opts.period == 0 {
            return Ok(alert);
        }
        let period = Duration::from_secs(opts.period);
        thread::sleep(period.saturating_sub(started.elapsed()));
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_report::DiffReport;
    use crate::path_shared::PathShared;

    fn system() -> SystemTag {
        serde_json::from_str(
            r#"{"username":"u","hostname":"h","os_name":"linux","os_version":"1","architecture":"x86_64","logical_cores":4}"#,
        )
        .unwrap()
    }

    #[test]
    fn test_event_round_trip_a() {
        let report = DiffReport::new(
            None,
            &ScanFS::from_parts(
                Default::default(),
                Default::default(),
                ScanConfig::new(false, false),
                "h".into(),
            ),
            Duration::from_secs(5),
            Err("offline".into()),
            None,
        );
        for body in [
            EventBody::Start {
                baseline: "rolling".into(),
                baseline_time: None,
                period: 60,
                report_period: 3600,
                ecosystems: vec![Ecosystem::Python, Ecosystem::Npm],
            },
            EventBody::Scan {
                origins: 1,
                sites: 2,
                packages: 3,
                changes: ScanDiff::default(),
            },
            EventBody::Report { report },
            EventBody::Error {
                message: "x".into(),
            },
        ] {
            let e = Event {
                schema: SCHEMA,
                time: Duration::from_secs(9),
                system: system(),
                body,
            };
            let j = serde_json::to_string(&e).unwrap();
            assert!(j.contains(r#""event":""#), "{j}");
            let back: Event = serde_json::from_str(&j).unwrap();
            assert_eq!(back, e);
        }
    }

    #[test]
    fn test_event_unknown_a() {
        let j = r#"{"schema":3,"time":{"secs":1,"nanos":0},"system":{"username":"u","hostname":"h","os_name":"linux","os_version":"1","architecture":"x86_64","logical_cores":4},"event":"summary","extra":[1]}"#;
        let e: Event = serde_json::from_str(j).unwrap();
        assert_eq!(e.body, EventBody::Unknown);
        assert_eq!(e.schema, 3);
    }

    #[test]
    fn test_state_round_trip_a() {
        use crate::origin_site::Origin;
        use crate::origin_site::Site;
        use crate::package::Package;
        use crate::version_spec::VersionSpec;
        let site = Site::python(PathShared::from("/env/site-packages"));
        let origin = Origin::new(Ecosystem::Python, PathShared::from("/env/bin/python"));
        let six =
            Package::new(Ecosystem::Python, "six", VersionSpec::new("1.16.0"), None);
        let mut scan = ScanFS::from_parts(
            [(origin, vec![site.clone()])].into_iter().collect(),
            [(six.clone(), vec![site])].into_iter().collect(),
            ScanConfig::new(false, false),
            "h".into(),
        );
        scan.site_to_hooks = Some(Default::default());
        let state = DiffState {
            schema: SCHEMA,
            fetter_version: Some("5.0.0".into()),
            time: Duration::from_secs(10),
            scan,
            vulns: Some(vec![PackageVulns {
                package: six,
                ids: vec!["GHSA-1".into()],
            }]),
            violations: None,
        };
        let dir = tempfile::tempdir().unwrap();
        let path = rolling_path(dir.path(), "abc");
        state.write(&path).unwrap();
        let back = DiffState::read(&path).unwrap();
        assert_eq!(back.scan, state.scan);
        assert_eq!(back.vulns, state.vulns);
        assert_eq!(back.time, state.time);
        assert!(!path.with_extension("json.tmp").exists());
        assert!(DiffState::read(&dir.path().join("missing.json")).is_err());
    }

    fn cargo_lock(dir: &Path, version: &str) {
        fs::write(
            dir.join("Cargo.lock"),
            format!(
                "version = 3\n\n[[package]]\nname = \"smallvec\"\nversion = \"{version}\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n"
            ),
        )
        .unwrap();
    }

    fn client(vulns: &str) -> Arc<dyn UreqClient> {
        use crate::ureq_client::UreqClientMock;
        let mut post = std::collections::HashMap::new();
        post.insert(
            "https://api.osv.dev".to_string(),
            format!(r#"{{"results":[{vulns}]}}"#),
        );
        Arc::new(UreqClientMock {
            mock_post: Some(post),
            mock_get: None,
        })
    }

    fn events(path: &Path) -> Vec<Event> {
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn reports(events: &[Event]) -> Vec<DiffReport> {
        events
            .iter()
            .filter_map(|e| match &e.body {
                EventBody::Report { report } => Some(report.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn test_monitor_diff_once_a() {
        use crate::diff_report::PackageChangeKind;
        use crate::diff_report::VulnSource;
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("proj");
        fs::create_dir(&proj).unwrap();
        let cache = dir.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let out = dir.path().join("events.jsonl");
        cargo_lock(&proj, "1.6.0");

        let origins = vec![proj.clone()];
        let config = ScanConfig::new(false, false).with_ecosystems(&[Ecosystem::Cargo]);
        // durations of 0 disable the OSV batch cache, so each run sees the mock
        let cache_config = CacheConfig::new(Duration::ZERO, cache.clone());
        let opts = |baseline: Option<PathBuf>| DiffOptions {
            period: 0,
            report_period: 0,
            baseline,
            report_always: false,
            bound: None,
            cvss: CvssFilter::All,
            output: DiffOutput::Json(Some(out.clone())),
        };
        let vulnerable = r#"{"vulns":[{"id":"RUSTSEC-2021-0003","modified":"x"}]}"#;
        let run = |c: &str, baseline: Option<PathBuf>| {
            monitor_diff_loop(
                &origins,
                client(c),
                &config,
                &cache_config,
                &opts(baseline),
                FlagLog(false),
            )
            .unwrap()
        };

        // a new baseline: the existing vulnerability is an alert
        assert!(run(vulnerable, None));
        let state_path = rolling_path(&cache, &hash_paths(&origins, &config));
        assert!(state_path.exists());
        let e = events(&out);
        assert!(matches!(
            e[0].body,
            EventBody::Start {
                baseline_time: None,
                ..
            }
        ));
        let r = reports(&e);
        assert_eq!(r.len(), 1);
        assert!(r[0].baseline_time.is_none());
        assert_eq!(r[0].new_vulns[0].source, VulnSource::Existing);

        // unchanged: no report
        assert!(!run(vulnerable, None));
        assert_eq!(reports(&events(&out)).len(), 1);

        // a newly published advisory against an unchanged crate
        let two = r#"{"vulns":[{"id":"RUSTSEC-2021-0003","modified":"x"},{"id":"RUSTSEC-2099-0001","modified":"x"}]}"#;
        assert!(run(two, None));
        let r = reports(&events(&out));
        assert_eq!(r.len(), 2);
        assert_eq!(r[1].new_vulns.len(), 1);
        assert_eq!(r[1].new_vulns[0].source, VulnSource::Published);

        // a fixed baseline is never written
        let fixed = dir.path().join("fixed.json");
        monitor_diff_save(
            &origins,
            client("{}"),
            &config,
            &cache_config,
            &opts(None),
            &fixed,
            FlagLog(false),
        )
        .unwrap();
        let saved = fs::read_to_string(&fixed).unwrap();
        cargo_lock(&proj, "1.6.1");
        assert!(!run("{}", Some(fixed.clone())));
        assert_eq!(fs::read_to_string(&fixed).unwrap(), saved);
        let r = reports(&events(&out));
        assert_eq!(r[2].changes.packages[0].change, PackageChangeKind::Upgraded);
        assert!(r[2].new_vulns.is_empty() && r[2].resolved_vulns.is_empty());

        // against the rolling baseline: upgraded, and both advisories resolved
        assert!(!run("{}", None));
        let r = reports(&events(&out));
        assert_eq!(r[3].resolved_vulns.len(), 2);

        // an audit failure reports changes and keeps the baseline's vulnerabilities
        cargo_lock(&proj, "1.6.0");
        let failing: Arc<dyn UreqClient> = Arc::new(crate::ureq_client::UreqClientMock {
            mock_post: None,
            mock_get: None,
        });
        assert!(monitor_diff_loop(
            &origins,
            failing,
            &config,
            &cache_config,
            &opts(None),
            FlagLog(false)
        )
        .unwrap());
        let r = reports(&events(&out));
        assert!(r[4].audit_error.is_some());
        assert_eq!(
            r[4].changes.packages[0].change,
            PackageChangeKind::Downgraded
        );
        let state = DiffState::read(&state_path).unwrap();
        assert_eq!(state.vulns, Some(vec![]));
    }

    #[test]
    fn test_monitor_diff_write_failure_a() {
        // a failed write of the report ends monitoring without advancing the baseline
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("proj");
        fs::create_dir(&proj).unwrap();
        cargo_lock(&proj, "1.6.0");
        let origins = vec![proj];
        let config = ScanConfig::new(false, false).with_ecosystems(&[Ecosystem::Cargo]);
        let cache_config = CacheConfig::new(Duration::ZERO, dir.path().to_path_buf());
        let opts = DiffOptions {
            period: 0,
            report_period: 0,
            baseline: None,
            report_always: false,
            bound: None,
            cvss: CvssFilter::All,
            // a directory cannot be opened for appending
            output: DiffOutput::Json(Some(dir.path().to_path_buf())),
        };
        assert!(monitor_diff_loop(
            &origins,
            client("{}"),
            &config,
            &cache_config,
            &opts,
            FlagLog(false)
        )
        .is_err());
        assert!(!rolling_path(dir.path(), &hash_paths(&origins, &config)).exists());
    }
}
