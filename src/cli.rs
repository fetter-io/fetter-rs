use std::collections::HashSet;
use std::process;

use crate::inspect_report::InspectReport;
use crate::util::default_python_exe_names;
use crate::util::get_absolute_path_from_exe;
use crate::validation_report::ValidationFlags;
use clap::{Parser, Subcommand, ValueEnum};
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::Duration;

use crate::allow_list::AllowList;
use crate::audit_report::AuditReport;
use crate::dep_manifest::DepManifest;
use crate::dep_spec::DepSpec;
use crate::eco::scanner_for;
use crate::ecosystem::Ecosystem;
use crate::lookup_report::LookupReport;
use crate::monitor::monitor_scan_loop;
use crate::scan_fs::ScanFS;
use crate::spin::print_banner;
use crate::spin::spin;
use crate::table::Tableable;
use crate::unpack_report::UnpackReport;
use crate::ureq_client::UreqClient;
use crate::util::get_writer;
use crate::util::path_normalize;
use crate::util::CacheConfig;
use crate::util::FlagLog;
use crate::util::FlagRetainPassing;
use crate::util::ResultDynError;
use crate::util::ScanConfig;
use crate::util::DURATION_0;
use crate::util::{logger, FlagCacheRefresh};
use crate::util::{path_cache, Anchor};
use crate::EnvMarkerState;

//------------------------------------------------------------------------------
// utility enums

/// The output format of `derive`.
#[derive(Copy, Clone, ValueEnum)]
pub enum DeriveFormat {
    /// A requirements file (Python packages only).
    Requirements,
    /// A fetter.toml allow list (all ecosystems).
    Toml,
    /// A list of purls, one per observed package version (all ecosystems; the anchor does not apply).
    Purls,
    /// A list of purls with VERS ranges, one per observed package (all ecosystems; the anchor applies).
    PurlVers,
}

#[derive(Copy, Clone, ValueEnum)]
pub enum CliAnchor {
    Lower,
    Upper,
    Both,
}
impl From<CliAnchor> for Anchor {
    fn from(cli_anchor: CliAnchor) -> Self {
        match cli_anchor {
            CliAnchor::Lower => Anchor::Lower,
            CliAnchor::Upper => Anchor::Upper,
            CliAnchor::Both => Anchor::Both,
        }
    }
}

#[derive(Debug, Clone)]
pub enum CvssFilter {
    All,            // No filtering, show all results
    MaxOnly,        // Show only vulnerabilities with maximum observed CVSS score
    Threshold(f64), // Show vulnerabilities with CVSS score >= threshold
}

impl CvssFilter {
    pub fn from_arg(arg: Option<Option<f64>>) -> Self {
        match arg {
            None => CvssFilter::All,           // flag not present
            Some(None) => CvssFilter::MaxOnly, // --cvss with no value
            Some(Some(v)) if (0.0..=10.0).contains(&v) => CvssFilter::Threshold(v),
            Some(Some(_)) => {
                eprintln!("Error: CVSS score must be a number between 0.0 and 10.0");
                std::process::exit(1);
            }
        }
    }
}

//------------------------------------------------------------------------------

const ERROR_EXIT_CODE: i32 = 3;
const TITLE: &str =
    "fetter: System-wide package discovery, validation, and vulnerability scanning";

const AFTER_HELP: &str = "\
Examples:
  fetter scan
  fetter -E python scan
  fetter --root ~/src scan
  fetter scan write -o /tmp/pkgscan.txt --delimiter '|'
  fetter search --pattern pip*
  fetter --origin python3 derive -a lower write -o /tmp/bound_requirements.txt
  fetter --origin python3 validate --bound /tmp/bound_requirements.txt
  fetter --origin .venv count
  fetter audit
  fetter --origin python3 --origin /usr/bin/python audit write -o /tmp/audit.txt -d '|'
  fetter --origin python3 unpack-count
  fetter --origin .venv python site-install --bound requirements.txt
";

#[derive(clap::Parser)]
#[command(version, about, long_about = TITLE, after_help = AFTER_HELP)]
struct Cli {
    /// Ecosystems to scan, as a comma-separated list or repeated flag. If not provided, all supported ecosystems are scanned.
    #[arg(
        short = 'E',
        long,
        value_enum,
        value_delimiter = ',',
        value_name = "ECOSYSTEMS"
    )]
    ecosystem: Vec<Ecosystem>,

    /// Zero or more origin paths (e.g. a Python executable or virtual environment). The ecosystem of each path is inferred. If not provided, all discoverable origins will be used.
    #[arg(
        long,
        value_name = "ORIGINS",
        required = false,
        default_value = "*",
        short_alias = 'e',
        alias = "exe"
    )]
    origin: Vec<PathBuf>,

    /// Directories to search (recursively) for origins, replacing the default search of home directories and system locations.
    #[arg(long, value_name = "DIRECTORIES")]
    root: Vec<PathBuf>,

    /// Create or use caches that expire after the provided number of seconds. A duration of zero will disable caching.
    #[arg(long, short, required = false, default_value = "120")]
    cache_duration: u64,

    /// Provide an explicit directory to be used for storing caches.
    #[arg(long, required = false)]
    cache_directory: Option<PathBuf>,

    /// Disable terminal animations.
    #[arg(long, short)]
    quiet: bool,

    /// Enable logging output.
    #[arg(long, short)]
    log: bool,

    /// Force all output to stderr.
    #[arg(long)]
    stderr: bool,

    /// On validation failures, print version information and provided string.
    #[arg(long, short, required = false)]
    banner: Option<String>,

    /// Force inclusion of the user site-packages, even if it is not activated. If not set, user site packages will only be included if the interpreter has been configured to use it.
    #[arg(long, required = false)]
    user_site: bool,

    /// When searching for all discoverable origins, include all user directories. Otherwise, include only the user's home directory.
    #[arg(long, required = false)]
    all_users: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Scan environment to report on installed packages.
    Scan {
        #[command(subcommand)]
        subcommands: Option<ScanSubcommand>,
    },
    /// Inspect all sites for code that runs without being invoked: Python startup files, npm install scripts, and VS Code extensions activated on every editor start.
    Inspect {
        #[command(subcommand)]
        subcommands: Option<InspectSubcommand>,
    },
    /// Search environment to report on installed packages.
    Search {
        /// Provide a glob-like pattern to match packages.
        #[arg(short, long)]
        pattern: String,

        #[arg(long)]
        case: bool,

        #[command(subcommand)]
        subcommands: Option<SearchSubcommand>,
    },
    /// Count discovered origins, sites, and packages, by ecosystem.
    Count {
        #[command(subcommand)]
        subcommands: Option<CountSubcommand>,
    },
    /// Derive an allow list (requirements) from discovered packages.
    Derive {
        // Select the nature of the bound in the derived requirements.
        #[arg(short, long, value_enum)]
        anchor: CliAnchor,

        /// Output format. If not provided, a requirements file is written when all derived packages are Python, and a fetter.toml otherwise. `requirements` includes only Python packages.
        #[arg(long, value_enum)]
        format: Option<DeriveFormat>,

        #[command(subcommand)]
        subcommands: Option<DeriveSubcommand>,
    },
    /// Validate if packages conform to a validation target.
    Validate {
        /// File path or URL from which to read bound requirements.
        #[arg(short, long, value_name = "FILE")]
        bound: PathBuf,

        /// Names of additional optional (extra) dependency groups.
        #[arg(long, value_name = "OPTIONS")]
        bound_options: Option<Vec<String>>,

        /// Names of packages to be excluded from all evaluation.
        #[arg(long, value_name = "OPTIONS", default_value = "pip")]
        ignore: Vec<String>, // because we default, no reason to make Option

        /// If the subset flag is set, the observed packages can be a subset of the bound requirements.
        #[arg(long)]
        subset: bool,

        /// If the superset flag is set, the observed packages can be a superset of the bound requirements.
        #[arg(long)]
        superset: bool,

        /// Validate every scanned ecosystem, treating an ecosystem the bound does not mention as having no allowed packages. By default such ecosystems are not validated.
        #[arg(long)]
        strict: bool,

        #[command(subcommand)]
        subcommands: Option<ValidateSubcommand>,
    },
    /// Search for package security vulnerabilities via the OSV DB.
    Audit {
        /// Provide a glob-like pattern to select packages.
        #[arg(short, long, default_value = "*")]
        pattern: String,

        /// Enable case-sensitive pattern matching.
        #[arg(long)]
        case: bool,

        /// Show a record for all packages, even if the package has no vulnerabilities.
        #[arg(long)]
        retain_passing: bool,

        /// Ignore any OSV caches and re-fetch vulnerability details.
        #[arg(long)]
        cache_refresh: bool,

        /// Filter vulnerabilities to those greater or equal to a provided CVSS score. If no argument is provided, the maximum is reported.
        #[arg(long, num_args = 0..=1, require_equals = true, value_name = "CVSS")]
        cvss: Option<Option<f64>>,

        #[command(subcommand)]
        subcommands: Option<AuditSubcommand>,
    },
    /// Search for security vulnerabilities in a named package (versions from the ecosystem's registry) via the OSV DB.
    LookupName {
        /// A package name with an optional requirement. For Python, a dependency specification (`numpy>=2`); otherwise `name@requirement` in the ecosystem's native syntax (`-E npm lookup-name 'lodash@^4.17'`, `-E cargo lookup-name serde@1`, `-E gha lookup-name actions/checkout@v4`).
        #[arg(value_name = "NAME")]
        name: String,

        /// If the package does not specify a version, determine how many recent versions to audit.
        #[arg(long)]
        limit: Option<usize>,

        /// Show a record for all packages, even if the package has no vulnerabilities.
        #[arg(long)]
        retain_passing: bool,

        /// Ignore any OSV caches and re-fetch vulnerability details.
        #[arg(long)]
        cache_refresh: bool,

        /// Filter vulnerabilities to those greater or equal to a provided CVSS score. If no argument is provided, the maximum is reported.
        #[arg(long, num_args = 0..=1, require_equals = true, value_name = "CVSS")]
        cvss: Option<Option<f64>>,

        #[command(subcommand)]
        subcommands: Option<LookupNameSubcommand>,
    },
    /// Search for security vulnerabilities in packages from a requirements or lock file via the OSV DB.
    LookupBound {
        /// Directory, file path, URL, or git repository from which to read bound requirements. The provided file can be a fetter.toml, requirements.txt, pyproject.toml or a lock file created by `uv`, `poetry`, `pipenv`, or `pip-tools`. If no argument is provided (or a directory is provided), the directory's fetter.toml is used if present; otherwise it is searched for a lock file, requirements.txt, or pyproject.toml.
        #[arg(value_name = "FILE")]
        bound: Option<PathBuf>,

        /// Names of additional optional (extra) dependency groups.
        #[arg(long, value_name = "OPTIONS")]
        bound_options: Option<Vec<String>>,

        /// Show a record for all packages, even if the package has no vulnerabilities.
        #[arg(long)]
        retain_passing: bool,

        /// Ignore any OSV caches and re-fetch vulnerability details.
        #[arg(long)]
        cache_refresh: bool,

        /// Filter vulnerabilities to those greater or equal to a provided CVSS score. If no argument is provided, the maximum is reported.
        #[arg(long, num_args = 0..=1, require_equals = true, value_name = "CVSS")]
        cvss: Option<Option<f64>>,

        #[command(subcommand)]
        subcommands: Option<LookupBoundSubcommand>,
    },
    /// Discover counts of all installed packages artifacts (files and directories); packages known only from lockfiles or manifests have none.
    UnpackCount {
        /// Provide a glob-like pattern to select packages.
        #[arg(short, long, default_value = "*")]
        pattern: String,

        /// Enable case-sensitive pattern matching.
        #[arg(long)]
        case: bool,

        #[command(subcommand)]
        subcommands: Option<UnpackCountSubcommand>,
    },
    /// Discover file names of all installed package artifacts; packages known only from lockfiles or manifests have none.
    UnpackFiles {
        /// Provide a glob-like pattern to select packages.
        #[arg(short, long, default_value = "*")]
        pattern: String,

        /// Enable case-sensitive pattern matching.
        #[arg(long)]
        case: bool,

        #[command(subcommand)]
        subcommands: Option<UnpackFilesSubcommand>,
    },
    /// Purge packages that match a search pattern (Python packages only, for now).
    PurgePattern {
        /// Provide a glob-like pattern to select packages.
        #[arg(short, long, default_value = "*")]
        pattern: Option<String>,

        /// Enable case-sensitive pattern matching.
        #[arg(long)]
        case: bool,
    },
    /// Purge packages that are invalid based on dependency specification (Python packages only, for now).
    PurgeInvalid {
        /// File path or URL from which to read bound requirements.
        #[arg(short, long, value_name = "FILE")]
        bound: PathBuf,

        /// Names of additional optional dependency groups.
        #[arg(long, value_name = "OPTIONS")]
        bound_options: Option<Vec<String>>,

        /// If the subset flag is set, the observed packages can be a subset of the bound requirements.
        #[arg(long)]
        subset: bool,

        /// If the superset flag is set, the observed packages can be a superset of the bound requirements.
        #[arg(long)]
        superset: bool,
    },
    /// Commands that apply only to Python environments.
    Python {
        #[command(subcommand)]
        command: PythonCommands,
    },
    /// Periodically scan system and post JSON output to a URL.
    MonitorScan {
        /// Set the period of scan in seconds.
        #[arg(short, long, default_value = "0")]
        period: u64,

        /// Provide the URL to which to post results.
        #[arg(short, long)]
        url: String,

        /// Provide the tenant key.
        #[arg(short, long)]
        tenant: String,
    },
}

/// Commands that apply only to Python environments.
#[derive(Subcommand)]
enum PythonCommands {
    /// Install in site-packages automatic validation checks on every Python run. This command will only run with one Python environment.
    SiteInstall {
        /// File path or URL from which to read bound requirements.
        #[arg(short, long, value_name = "FILE")]
        bound: PathBuf,

        /// Names of additional optional (extra) dependency groups.
        #[arg(long, value_name = "OPTIONS")]
        bound_options: Option<Vec<String>>,

        /// Names of packages to be excluded from all evaluation.
        #[arg(long, value_name = "OPTIONS", default_value = "pip")]
        ignore: Vec<String>,

        /// If the subset flag is set, the observed packages can be a subset of the bound requirements.
        #[arg(long)]
        subset: bool,

        /// If the superset flag is set, the observed packages can be a superset of the bound requirements.
        #[arg(long)]
        superset: bool,

        #[command(subcommand)]
        subcommands: Option<SiteInstallSubcommand>,
    },
    /// Uninstall from site-packages automatic validation checks. This command will only run with one Python environment.
    SiteUninstall,
}

impl fmt::Display for Commands {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let op_str = match self {
            Commands::Scan { .. } => "scan",
            Commands::Inspect { .. } => "inspect",
            Commands::Search { .. } => "search",
            Commands::Count { .. } => "count",
            Commands::Derive { .. } => "derive",
            Commands::Validate { .. } => "validate",
            Commands::Audit { .. } => "audit",
            Commands::LookupName { .. } => "lookup-name",
            Commands::LookupBound { .. } => "lookup-bound",
            Commands::UnpackCount { .. } => "unpack-count",
            Commands::UnpackFiles { .. } => "unpack-files",
            Commands::PurgePattern { .. } => "purge-pattern",
            Commands::PurgeInvalid { .. } => "purge-invalid",
            Commands::Python { .. } => "python",
            Commands::MonitorScan { .. } => "monitor-scan",
        };
        write!(f, "{op_str}")
    }
}

//------------------------------------------------------------------------------

#[derive(Subcommand)]
enum ScanSubcommand {
    /// Display scan in the terminal.
    Display,
    /// Write a scan report to a file.
    Write {
        #[arg(short, long, value_name = "FILE")]
        output: PathBuf,
        #[arg(short, long, default_value = ",")]
        delimiter: char,
    },
}

#[derive(Subcommand)]
enum InspectSubcommand {
    /// Display inspect in the terminal.
    Display,
    /// Write an inspect report to a file.
    Write {
        #[arg(short, long, value_name = "FILE")]
        output: PathBuf,
        #[arg(short, long, default_value = ",")]
        delimiter: char,
    },
}

#[derive(Subcommand)]
enum SearchSubcommand {
    /// Display search int the terminal.
    Display,
    /// Write a search report to a file.
    Write {
        #[arg(short, long, value_name = "FILE")]
        output: PathBuf,
        #[arg(short, long, default_value = ",")]
        delimiter: char,
    },
}

#[derive(Subcommand)]
enum CountSubcommand {
    /// Display scan in the terminal.
    Display,
    /// Write a report to a delimited file.
    Write {
        #[arg(short, long, value_name = "FILE")]
        output: PathBuf,
        #[arg(short, long, default_value = ",")]
        delimiter: char,
    },
}

#[derive(Subcommand)]
enum DeriveSubcommand {
    /// Display derive in the terminal.
    Display,
    /// Write a derive report to a file.
    Write {
        #[arg(short, long, value_name = "FILE")]
        output: PathBuf,
    },
}

#[derive(Subcommand)]
enum ValidateSubcommand {
    /// Display validation in the terminal.
    Display {
        /// If set, the process will exit the provided code.
        #[arg(short, long)]
        code: Option<i32>,
    },
    /// Print a Json representation of validation results.
    Json,
    /// Write a validation report to a file.
    Write {
        #[arg(short, long, value_name = "FILE")]
        output: PathBuf,
        #[arg(short, long, default_value = ",")]
        delimiter: char,
    },
    /// Return an exit code, 0 on success, 3 (by default) on error.
    Exit {
        #[arg(short, long, default_value = "3")]
        code: i32,
    },
}

#[derive(Subcommand)]
enum SiteInstallSubcommand {
    /// Configure site-install to print warnings on validation errors.
    Warn,
    /// Configure site-install to return an exit code on validation errors.
    Exit {
        #[arg(short, long, default_value = "3")]
        code: i32,
    },
}

#[derive(Subcommand)]
enum AuditSubcommand {
    /// Display audit results in the terminal.
    Display,
    /// Write audit results to a delimited file.
    Write {
        #[arg(short, long, value_name = "FILE")]
        output: PathBuf,
        #[arg(short, long, default_value = ",")]
        delimiter: char,
    },
    /// Print a Json representation of audit report results.
    Json,
    /// Return an exit code, 0 on success, 3 (by default) on error.
    Exit {
        #[arg(short, long, default_value = "3")]
        code: i32,
    },
}

#[derive(Subcommand)]
enum LookupNameSubcommand {
    /// Display lookup results in the terminal.
    Display,
    /// Write lookup results to a delimited file.
    Write {
        #[arg(short, long, value_name = "FILE")]
        output: PathBuf,
        #[arg(short, long, default_value = ",")]
        delimiter: char,
    },
    /// Print a Json representation of lookup report results.
    Json,
    /// Return an exit code, 0 on success, 3 (by default) on error.
    Exit {
        #[arg(short, long, default_value = "3")]
        code: i32,
    },
}

#[derive(Subcommand)]
enum LookupBoundSubcommand {
    /// Display lookup results in the terminal.
    Display,
    /// Write lookup results to a delimited file.
    Write {
        #[arg(short, long, value_name = "FILE")]
        output: PathBuf,
        #[arg(short, long, default_value = ",")]
        delimiter: char,
    },
    /// Print a Json representation of lookup report results.
    Json,
    /// Return an exit code, 0 on success, 3 (by default) on error.
    Exit {
        #[arg(short, long, default_value = "3")]
        code: i32,
    },
}

#[derive(Subcommand)]
enum UnpackCountSubcommand {
    /// Display installed artifacts in the terminal.
    Display,
    /// Write installed artifacts to a delimited file.
    Write {
        #[arg(short, long, value_name = "FILE")]
        output: PathBuf,
        #[arg(short, long, default_value = ",")]
        delimiter: char,
    },
}

#[derive(Subcommand)]
enum UnpackFilesSubcommand {
    /// Display installed artifacts in the terminal.
    Display,
    /// Write installed artifacts to a delimited file.
    Write {
        #[arg(short, long, value_name = "FILE")]
        output: PathBuf,
        #[arg(short, long, default_value = ",")]
        delimiter: char,
    },
}

//------------------------------------------------------------------------------
// Utility constructors specialized for CLI contexts

/// Print to stderr what an audit could not check: packages that could not be queried (e.g. no version, an unresolvable ref) and requirements whose package was not found in its registry (e.g. a private package). These do not change the exit code.
fn report_unchecked(ar: &AuditReport) {
    if !ar.precommit_hooks.is_empty() {
        eprintln!(
            "fetter: {} pre-commit hook repo(s) audited as the packages they install: {}",
            ar.precommit_hooks.len(),
            ar.precommit_hooks
                .iter()
                .map(|m| format!(
                    "{}@{} = {}",
                    m.hook.name,
                    m.hook.version,
                    m.packages
                        .iter()
                        .map(|p| format!("{} {}", p.ecosystem, p))
                        .collect::<Vec<_>>()
                        .join(" + ")
                ))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    for m in &ar.pin_mismatches {
        let short = &m.commit[..m.commit.len().min(12)];
        let found = if m.tag_exists {
            format!("tag {} is on another commit", m.comment)
        } else {
            format!("no tag {} exists", m.comment)
        };
        let audited = match &m.actual {
            Some(v) => format!("audited as {v}, the tag of its commit"),
            None => "not audited: its commit has no version tag".to_string(),
        };
        eprintln!(
            "fetter: warning: {}@{short} has the comment {}, but {found}; {audited}",
            m.name, m.comment
        );
    }
    if !ar.resolved_pins.is_empty() {
        eprintln!(
            "fetter: {} SHA pin(s) without a version comment audited as the tag of their commit: {}",
            ar.resolved_pins.len(),
            ar.resolved_pins
                .iter()
                .map(|r| format!("{}@{} = {}", r.name, &r.commit[..r.commit.len().min(12)], r.version))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !ar.unaudited.is_empty() {
        eprintln!(
            "fetter: {} package(s) could not be audited: {}",
            ar.unaudited.len(),
            ar.unaudited
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !ar.unresolved.is_empty() {
        eprintln!(
            "fetter: {} requirement(s) not found in their registry, so not audited: {}",
            ar.unresolved.len(),
            ar.unresolved
                .iter()
                .map(|u| format!("{} ({})", u.requirement, u.ecosystem))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

/// Note selected packages that have no installed artifacts, rather than omitting them silently.
fn report_without_artifacts(ur: &UnpackReport) {
    if !ur.without_artifacts.is_empty() {
        let total: usize = ur.without_artifacts.iter().map(|(_, n)| n).sum();
        eprintln!(
            "fetter: {} package(s) are known only from lockfiles or manifests and have no installed artifacts: {}",
            total,
            ur.without_artifacts
                .iter()
                .map(|(e, n)| format!("{e} {n}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

/// The Python section of an allow list (empty if there is none), for requirements output.
fn python_requirements(al: &AllowList) -> ResultDynError<DepManifest> {
    match &al.python {
        Some(dm) => Ok(dm.clone()),
        None => DepManifest::try_from_iter(Vec::<String>::new()),
    }
}

/// Determine the ecosystems to scan. Python-only commands require Python; lookup commands currently support only Python. Explicitly requested ecosystems that are not yet implemented are reported and skipped.
fn resolve_ecosystems(
    requested: &[Ecosystem],
    command: Option<&Commands>,
) -> ResultDynError<Vec<Ecosystem>> {
    if let Some(Commands::Python { .. }) = command {
        if requested.iter().any(|e| *e != Ecosystem::Python) {
            return Err("`python` supports only the python ecosystem".into());
        }
        return Ok(vec![Ecosystem::Python]);
    }
    // a name is looked up in exactly one ecosystem; Python if none is given (as before v5)
    if let Some(Commands::LookupName { .. }) = command {
        return match requested {
            [] => Ok(vec![Ecosystem::Python]),
            [e] => Ok(vec![*e]),
            _ => Err("`lookup-name` takes exactly one ecosystem (-E)".into()),
        };
    }
    if requested.is_empty() {
        return Ok(Ecosystem::ALL.to_vec());
    }
    for e in requested {
        if scanner_for(*e).is_none() {
            eprintln!("fetter: ecosystem `{e}` is not yet supported and will be skipped");
        }
    }
    Ok(requested.to_vec())
}

/// The ecosystem and `lookup-name` argument (`name@requirement`, or a Python requirement) for a purl: `pkg:npm/lodash@4.17.15` is npm `lodash@4.17.15`. A `pkg:github` purl names a GitHub Action unless `-E precommit` is given. An explicit `-E` must agree with the purl.
fn purl_lookup(
    arg: &str,
    requested: &[Ecosystem],
) -> ResultDynError<(Ecosystem, String)> {
    let purl = crate::purl::Purl::parse(arg)?;
    let targets = crate::purl::purl_targets(&purl);
    let target = match requested {
        [] => targets.first(),
        [e] => targets.iter().find(|t| t.0 == *e),
        _ => return Err("`lookup-name` takes exactly one ecosystem (-E)".into()),
    };
    let Some((eco, name, version)) = target else {
        return Err(if targets.is_empty() {
            format!("purl type {:?} is not supported", purl.ty).into()
        } else {
            format!("{arg} is not a package of the ecosystem given with -E").into()
        });
    };
    let req = crate::purl::exact_requirement(*eco, name, version.as_deref());
    let arg = match eco {
        Ecosystem::Python => req,
        _ => format!("{name}@{req}"),
    };
    Ok((*eco, arg))
}

// Provided `origin_paths` are not normalized.
fn from_cache_or_scan(
    origin_paths: &Vec<PathBuf>,
    config: &ScanConfig,
    animate: bool,
    cache_config: &CacheConfig,
    log: FlagLog,
    stderr: bool,
) -> ResultDynError<ScanFS> {
    ScanFS::from_cache(origin_paths, config, cache_config, log).or_else(|err| {
        logger!(
            log,
            module_path!(),
            "Could not load ScanFS from cache: {:?}",
            err
        );
        // full load
        let active = Arc::new(AtomicBool::new(true));
        if animate {
            spin(active.clone(), "scanning".to_string(), stderr);
        }
        let sfs = ScanFS::from_origin_paths(origin_paths, config, log)?;

        if cache_config.duration > DURATION_0 {
            sfs.to_cache(cache_config, log)?;
        }

        if animate {
            active.store(false, Ordering::Relaxed);
            thread::sleep(Duration::from_millis(100));
        }
        Ok(sfs)
    })
}

//------------------------------------------------------------------------------
/// Python-only commands that were top-level before v5.
const MOVED_TO_PYTHON: [&str; 2] = ["site-install", "site-uninstall"];

/// Accept the v4 spelling of commands that moved under `fetter python` (`fetter --exe python3 site-install ...` is `fetter --exe python3 python site-install ...`), so existing scripts keep working; this mirrors the hidden `--exe`/`-e` aliases of `--origin`. Only the first such argument is rewritten, and only if `python` does not already precede it.
fn migrate_args(args: Vec<OsString>) -> Vec<OsString> {
    let Some(i) = args
        .iter()
        .skip(1)
        .position(|a| a.to_str().is_some_and(|s| MOVED_TO_PYTHON.contains(&s)))
        .map(|i| i + 1)
    else {
        return args;
    };
    if args[1..i].iter().any(|a| a == "python") {
        return args;
    }
    let mut out = args;
    out.insert(i, OsString::from("python"));
    out
}

pub fn run_cli<I, T>(args: I, client: Arc<dyn UreqClient>) -> ResultDynError<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let cli = Cli::parse_from(migrate_args(args));
    if cli.command.is_none() {
        return Err("No command provided. For more information, try '--help'.".into());
    }
    let log = FlagLog(cli.log);
    let quiet = cli.quiet;
    let stderr = cli.stderr;
    let banner = cli.banner;
    let cache_dur = Duration::from_secs(cli.cache_duration);

    let cache_dir: PathBuf = match cli.cache_directory.as_deref() {
        Some(p) => path_normalize(p, true)?,
        None => path_cache(true)
            .ok_or_else(|| io::Error::other("Cannot get default cache dir"))?,
    };
    logger!(log, module_path!(), "Cache dir: {:?}", cache_dir);

    let ecosystems = resolve_ecosystems(&cli.ecosystem, cli.command.as_ref())?;
    let roots = cli
        .root
        .iter()
        .map(|r| path_normalize(r, true))
        .collect::<ResultDynError<Vec<PathBuf>>>()?;
    let config = ScanConfig::new(cli.user_site, cli.all_users)
        .with_ecosystems(&ecosystems)
        .with_roots(roots);

    // do a fresh scan or load a cached scan
    let get_sfs = || -> ResultDynError<ScanFS> {
        let cache_config = CacheConfig::new(cache_dur, cache_dir.clone());
        from_cache_or_scan(&cli.origin, &config, !quiet, &cache_config, log, stderr)
    };

    match &cli.command {
        Some(Commands::Scan { subcommands }) => match subcommands {
            Some(ScanSubcommand::Write { output, delimiter }) => {
                let sfs = get_sfs()?;
                let sr = sfs.to_scan_report();
                sr.to_file(output, *delimiter)?;
            }
            Some(ScanSubcommand::Display) | None => {
                let sfs = get_sfs()?;
                let sr = sfs.to_scan_report();
                let _ = sr.to_writer(stderr);
            }
        },
        Some(Commands::Inspect { subcommands }) => match subcommands {
            Some(InspectSubcommand::Write { output, delimiter }) => {
                let sfs = get_sfs()?;
                let it = InspectReport::from_site_to_origins(&sfs.site_to_origins)?;
                it.to_file(output, *delimiter)?;
            }
            Some(InspectSubcommand::Display) | None => {
                let sfs = get_sfs()?;
                let it = InspectReport::from_site_to_origins(&sfs.site_to_origins)?;
                let _ = it.to_writer(stderr);
            }
        },
        Some(Commands::Search {
            subcommands,
            pattern,
            case,
        }) => match subcommands {
            Some(SearchSubcommand::Write { output, delimiter }) => {
                let sfs = get_sfs()?;
                let sr = sfs.to_search_report(pattern, !case);
                sr.to_file(output, *delimiter)?;
            }
            Some(SearchSubcommand::Display) | None => {
                // default
                let sfs = get_sfs()?;
                let sr = sfs.to_search_report(pattern, !case);
                let _ = sr.to_writer(stderr);
            }
        },
        Some(Commands::Count { subcommands }) => match subcommands {
            Some(CountSubcommand::Write { output, delimiter }) => {
                let sfs = get_sfs()?;
                let cr = sfs.to_count_report();
                cr.to_file(output, *delimiter)?;
            }
            Some(CountSubcommand::Display) | None => {
                // default
                let sfs = get_sfs()?;
                let cr = sfs.to_count_report();
                let _ = cr.to_writer(stderr);
            }
        },
        Some(Commands::Derive {
            subcommands,
            anchor,
            format,
        }) => {
            let sfs = get_sfs()?;
            if matches!(format, Some(DeriveFormat::Purls | DeriveFormat::PurlVers)) {
                let (purls, without) = match format {
                    Some(DeriveFormat::PurlVers) => sfs.to_purl_ranges((*anchor).into()),
                    _ => sfs.to_purls(),
                };
                if without > 0 {
                    eprintln!("fetter: {without} package(s) have no purl type and are not listed");
                }
                let text = format!("# via fetter\n{}\n", purls.join("\n"));
                match subcommands {
                    Some(DeriveSubcommand::Write { output }) => {
                        std::fs::write(output, text).map_err(|e| {
                            format!("Cannot write {}: {e}", output.display())
                        })?
                    }
                    Some(DeriveSubcommand::Display) | None => {
                        let mut writer = get_writer(stderr);
                        let _ = write!(writer, "{text}");
                    }
                }
                return Ok(());
            }
            let al = sfs.to_allow_list((*anchor).into())?;
            let format = format.unwrap_or(if al.is_python_only() {
                DeriveFormat::Requirements
            } else {
                DeriveFormat::Toml
            });
            match (format, subcommands) {
                (
                    DeriveFormat::Requirements,
                    Some(DeriveSubcommand::Write { output }),
                ) => {
                    python_requirements(&al)?
                        .to_dep_manifest_report()
                        .to_file(output, ' ')?;
                }
                (DeriveFormat::Requirements, Some(DeriveSubcommand::Display) | None) => {
                    let _ = python_requirements(&al)?
                        .to_dep_manifest_report()
                        .to_writer(stderr);
                }
                (DeriveFormat::Toml, Some(DeriveSubcommand::Write { output })) => {
                    std::fs::write(output, al.to_toml())
                        .map_err(|e| format!("Cannot write {}: {e}", output.display()))?;
                }
                (DeriveFormat::Toml, Some(DeriveSubcommand::Display) | None) => {
                    let mut writer = get_writer(stderr);
                    let _ = write!(writer, "{}", al.to_toml());
                }
                (DeriveFormat::Purls | DeriveFormat::PurlVers, _) => {
                    unreachable!("handled above")
                }
            }
        }
        Some(Commands::Validate {
            bound,
            bound_options,
            ignore,
            subset,
            superset,
            strict,
            subcommands,
        }) => {
            let mut sfs = get_sfs()?;
            // a fetter.toml, or any Python bound source
            let al = AllowList::from_bound(bound, bound_options.as_ref(), log)?;
            let permit_superset = *superset;
            let permit_subset = *subset;

            let u_ignore: HashSet<String> = HashSet::from_iter(ignore.iter().cloned());

            let vr = sfs.to_allow_list_validation(
                &al,
                ValidationFlags {
                    permit_superset,
                    permit_subset,
                },
                Some(&u_ignore),
                *strict,
                log,
            );
            // we only print the banner on failure for now
            if !vr.is_empty() && banner.is_some() {
                print_banner(true, banner, stderr);
            }
            match subcommands {
                Some(ValidateSubcommand::Json) => {
                    println!("{}", serde_json::to_string(&vr.to_validation_digest())?);
                }
                Some(ValidateSubcommand::Write { output, delimiter }) => {
                    vr.to_file(output, *delimiter)?;
                }
                Some(ValidateSubcommand::Exit { code }) => {
                    process::exit(if !vr.is_empty() { *code } else { 0 });
                }
                Some(ValidateSubcommand::Display { code }) => {
                    vr.to_writer(stderr)?;
                    if !vr.is_empty() {
                        if let Some(e) = code {
                            process::exit(*e);
                        }
                    }
                }
                None => {
                    vr.to_writer(stderr)?;
                }
            }
        }
        Some(Commands::Audit {
            subcommands,
            pattern,
            case,
            retain_passing,
            cache_refresh,
            cvss,
        }) => {
            let sfs = get_sfs()?;
            // network lookup makes this potentially slow
            let active = Arc::new(AtomicBool::new(true));
            if !quiet {
                spin(
                    active.clone(),
                    "vulnerability searching".to_string(),
                    stderr,
                );
            }
            let cvss_filter = CvssFilter::from_arg(*cvss);
            let cache_config = CacheConfig::new(cache_dur, cache_dir.clone());
            let ar = sfs.to_audit_report(
                pattern,
                client,
                !case,
                FlagCacheRefresh(*cache_refresh),
                &cache_config,
                log,
                cvss_filter,
                FlagRetainPassing(*retain_passing),
            );
            if !quiet {
                active.store(false, Ordering::Relaxed);
                thread::sleep(Duration::from_millis(100));
            }
            // stop the spinner before propagating any error
            let ar = ar?;
            report_unchecked(&ar);
            match subcommands {
                Some(AuditSubcommand::Json) => {
                    println!("{}", serde_json::to_string(&ar)?);
                }
                Some(AuditSubcommand::Write { output, delimiter }) => {
                    ar.to_file(output, *delimiter)?;
                }
                Some(AuditSubcommand::Exit { code }) => {
                    process::exit(if !ar.is_empty() { *code } else { 0 });
                }
                Some(AuditSubcommand::Display) | None => {
                    // default
                    let _ = ar.to_writer(stderr);
                    process::exit(if !ar.is_empty() { ERROR_EXIT_CODE } else { 0 });
                }
            }
        }
        Some(Commands::LookupName {
            subcommands,
            name,
            limit,
            cache_refresh,
            cvss,
            retain_passing,
        }) => {
            // network lookup makes this potentially slow
            let active = Arc::new(AtomicBool::new(true));
            if !quiet {
                spin(
                    active.clone(),
                    "vulnerability searching".to_string(),
                    stderr,
                );
            }
            let cvss_filter = CvssFilter::from_arg(*cvss);
            let cache_config = CacheConfig::new(cache_dur, cache_dir.clone());
            // a purl names its ecosystem
            let (eco, name) = if name.starts_with("pkg:") {
                purl_lookup(name, &cli.ecosystem)?
            } else {
                (ecosystems[0], name.clone())
            };
            let name = &name;
            let lr = match eco {
                Ecosystem::Python => {
                    // NOTE: by conversion to DepSpec, we parse the string and reject any invalid characters
                    DepSpec::from_string(name).and_then(|ds| {
                        LookupReport::from_dep_spec(
                            client,
                            &ds,
                            *limit,
                            &cache_config,
                            FlagCacheRefresh(*cache_refresh),
                            log,
                            cvss_filter,
                            FlagRetainPassing(*retain_passing),
                        )
                    })
                }
                eco => LookupReport::from_name_requirement(
                    client,
                    eco,
                    name,
                    *limit,
                    &cache_config,
                    FlagCacheRefresh(*cache_refresh),
                    log,
                    cvss_filter,
                    FlagRetainPassing(*retain_passing),
                ),
            };
            if !quiet {
                active.store(false, Ordering::Relaxed);
                thread::sleep(Duration::from_millis(100));
            }
            // stop the spinner before propagating any error
            let lr = lr?;
            report_unchecked(&lr);
            match subcommands {
                Some(LookupNameSubcommand::Json) => {
                    println!("{}", serde_json::to_string(&lr)?);
                }
                Some(LookupNameSubcommand::Write { output, delimiter }) => {
                    lr.to_file(output, *delimiter)?;
                }
                Some(LookupNameSubcommand::Exit { code }) => {
                    process::exit(if !lr.is_empty() { *code } else { 0 });
                }
                Some(LookupNameSubcommand::Display) | None => {
                    // default
                    let _ = lr.to_writer(stderr);
                    process::exit(if !lr.is_empty() { ERROR_EXIT_CODE } else { 0 });
                }
            }
        }
        Some(Commands::LookupBound {
            subcommands,
            bound,
            bound_options,
            retain_passing,
            cache_refresh,
            cvss,
        }) => {
            // network lookup makes this potentially slow
            let active = Arc::new(AtomicBool::new(true));
            if !quiet {
                spin(
                    active.clone(),
                    "vulnerability searching".to_string(),
                    stderr,
                );
            }
            let cvss_filter = CvssFilter::from_arg(*cvss);
            let cache_config = CacheConfig::new(cache_dur, cache_dir.clone());

            // a directory (default: the current directory) uses its fetter.toml if present, else the first Python bound found
            let al = match bound {
                Some(b) if !b.is_dir() => {
                    AllowList::from_bound(b, bound_options.as_ref(), log)?
                }
                _ => {
                    let d = match bound {
                        Some(b) => b.clone(),
                        None => env::current_dir()?,
                    };
                    if d.join("fetter.toml").is_file() {
                        AllowList::from_bound(
                            &d.join("fetter.toml"),
                            bound_options.as_ref(),
                            log,
                        )?
                    } else {
                        AllowList::from_python(DepManifest::from_dir(
                            &d,
                            bound_options.as_ref(),
                            log,
                        )?)
                    }
                }
            };
            // NOTE: loading EnvMarkerState from the currently active Python if available, and only if there are Python requirements;
            // interpreter name varies by platform ("python" first on Windows).
            let ems = match &al.python {
                Some(_) => match default_python_exe_names()
                    .iter()
                    .find_map(|name| get_absolute_path_from_exe(name))
                {
                    Some(exe) => Some(EnvMarkerState::from_exe(exe.as_path())?),
                    None => None,
                },
                None => None,
            };

            let lr = LookupReport::from_allow_list(
                client,
                &al,
                &ecosystems,
                ems.as_ref(),
                &cache_config,
                FlagCacheRefresh(*cache_refresh),
                log,
                cvss_filter,
                FlagRetainPassing(*retain_passing),
            );
            if !quiet {
                active.store(false, Ordering::Relaxed);
                thread::sleep(Duration::from_millis(100));
            }
            // stop the spinner before propagating any error
            let lr = lr?;
            report_unchecked(&lr);
            match subcommands {
                Some(LookupBoundSubcommand::Json) => {
                    println!("{}", serde_json::to_string(&lr)?);
                }
                Some(LookupBoundSubcommand::Write { output, delimiter }) => {
                    lr.to_file(output, *delimiter)?;
                }
                Some(LookupBoundSubcommand::Exit { code }) => {
                    process::exit(if !lr.is_empty() { *code } else { 0 });
                }
                Some(LookupBoundSubcommand::Display) | None => {
                    // default
                    let _ = lr.to_writer(stderr);
                    process::exit(if !lr.is_empty() { ERROR_EXIT_CODE } else { 0 });
                }
            }
        }

        Some(Commands::UnpackCount {
            subcommands,
            pattern,
            case,
        }) => {
            let sfs = get_sfs()?;
            let count = true;
            let ir = sfs.to_unpack_report(pattern, !case, count);
            report_without_artifacts(&ir);
            match subcommands {
                Some(UnpackCountSubcommand::Write { output, delimiter }) => {
                    ir.to_file(output, *delimiter)?;
                }
                Some(UnpackCountSubcommand::Display) | None => {
                    // default
                    let _ = ir.to_writer(stderr);
                }
            }
        }
        Some(Commands::UnpackFiles {
            subcommands,
            pattern,
            case,
        }) => {
            let sfs = get_sfs()?;
            let count = false;
            let ir = sfs.to_unpack_report(pattern, !case, count);
            report_without_artifacts(&ir);
            match subcommands {
                Some(UnpackFilesSubcommand::Write { output, delimiter }) => {
                    ir.to_file(output, *delimiter)?;
                }
                Some(UnpackFilesSubcommand::Display) | None => {
                    // default
                    let _ = ir.to_writer(stderr);
                }
            }
        }
        Some(Commands::PurgePattern { pattern, case }) => {
            let sfs = get_sfs()?;
            let _ = sfs.to_purge_pattern(pattern, !case, log);
        }
        Some(Commands::PurgeInvalid {
            bound,
            bound_options,
            subset,
            superset,
        }) => {
            let mut sfs = get_sfs()?;
            let al = AllowList::from_bound(bound, bound_options.as_ref(), log)?;
            let permit_superset = *superset;
            let permit_subset = *subset;
            let _ = sfs.to_purge_invalid(
                &al,
                ValidationFlags {
                    permit_superset,
                    permit_subset,
                },
                log,
            );
        }
        Some(Commands::Python { command }) => match command {
            PythonCommands::SiteInstall {
                bound,
                bound_options,
                ignore,
                subset,
                superset,
                subcommands,
            } => {
                let sfs = get_sfs()?;
                let vf = ValidationFlags {
                    permit_superset: *superset,
                    permit_subset: *subset,
                };
                let exit_else_warn: Option<i32> = match subcommands {
                    Some(SiteInstallSubcommand::Warn) | None => None,
                    Some(SiteInstallSubcommand::Exit { code }) => Some(*code),
                };
                sfs.site_validate_install(
                    bound,
                    bound_options,
                    ignore,
                    &vf,
                    exit_else_warn,
                    log,
                )?;
            }
            PythonCommands::SiteUninstall => {
                let sfs = get_sfs()?;
                sfs.site_validate_uninstall(log)?;
            }
        },
        Some(Commands::MonitorScan {
            period,
            url,
            tenant,
        }) => {
            let _ = monitor_scan_loop(
                &cli.origin,
                client,
                url,
                tenant,
                &config,
                *period,
                log,
            );
        }
        None => {}
    }
    Ok(())
}

//-----------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).unwrap()
    }

    #[test]
    fn test_parse_ecosystem_a() {
        let cli = parse(&["fetter", "-E", "python,npm", "scan"]);
        assert_eq!(cli.ecosystem, vec![Ecosystem::Python, Ecosystem::Npm]);
        let cli = parse(&["fetter", "-E", "vscode", "--ecosystem", "gha", "scan"]);
        assert_eq!(cli.ecosystem, vec![Ecosystem::VsCode, Ecosystem::Gha]);
        assert!(Cli::try_parse_from(["fetter", "-E", "perl", "scan"]).is_err());
        // a purl names its ecosystem; -E must agree; github is an Action unless -E precommit
        assert_eq!(
            purl_lookup("pkg:npm/%40babel/core@7.24.0", &[]).unwrap(),
            (Ecosystem::Npm, "@babel/core@7.24.0".to_string())
        );
        assert_eq!(
            purl_lookup("pkg:pypi/Django_Package@4.2.1", &[]).unwrap(),
            (Ecosystem::Python, "django-package==4.2.1".to_string())
        );
        assert_eq!(
            purl_lookup("pkg:cargo/time@0.1.43", &[]).unwrap(),
            (Ecosystem::Cargo, "time@=0.1.43".to_string())
        );
        assert_eq!(
            purl_lookup("pkg:github/psf/black@24.1.0", &[]).unwrap().0,
            Ecosystem::Gha
        );
        assert_eq!(
            purl_lookup("pkg:github/psf/black@24.1.0", &[Ecosystem::PreCommit]).unwrap(),
            (
                Ecosystem::PreCommit,
                "github.com/psf/black@24.1.0".to_string()
            )
        );
        assert!(purl_lookup("pkg:npm/lodash@4.17.15", &[Ecosystem::Cargo]).is_err());
        assert!(purl_lookup("pkg:maven/a/b@1", &[]).is_err());
        assert!(purl_lookup("pkg:npm", &[]).is_err());
    }

    #[test]
    fn test_parse_origin_a() {
        let cli = parse(&["fetter", "scan"]);
        assert_eq!(cli.origin, vec![PathBuf::from("*")]);
        let cli = parse(&["fetter", "--origin", "python3", "--origin", ".venv", "scan"]);
        assert_eq!(
            cli.origin,
            vec![PathBuf::from("python3"), PathBuf::from(".venv")]
        );
        // `-e` and `--exe` remain as hidden aliases
        let cli = parse(&["fetter", "-e", "python3", "--exe", "python", "scan"]);
        assert_eq!(
            cli.origin,
            vec![PathBuf::from("python3"), PathBuf::from("python")]
        );
    }

    #[test]
    fn test_parse_origin_b() {
        // `-o` is not a short flag for `--origin` (it reads as "output")
        assert!(Cli::try_parse_from(["fetter", "-o", "python3", "scan"]).is_err());
    }

    #[test]
    fn test_parse_python_commands_a() {
        let cli = parse(&["fetter", "python", "site-uninstall"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Python {
                command: PythonCommands::SiteUninstall
            })
        ));
        // site commands are no longer top-level in the parser...
        assert!(Cli::try_parse_from(["fetter", "site-uninstall"]).is_err());
        // ...but the v4 spelling is migrated before parsing
        let m = |a: &[&str]| -> Vec<String> {
            migrate_args(a.iter().map(OsString::from).collect())
                .iter()
                .map(|s| s.to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(
            m(&[
                "fetter",
                "--exe",
                "python3",
                "site-install",
                "--bound",
                "r.txt"
            ]),
            vec![
                "fetter",
                "--exe",
                "python3",
                "python",
                "site-install",
                "--bound",
                "r.txt"
            ]
        );
        assert_eq!(
            m(&["fetter", "site-uninstall"]),
            vec!["fetter", "python", "site-uninstall"]
        );
        // already in v5 form, or not these commands: unchanged
        assert_eq!(
            m(&["fetter", "python", "site-install", "-b", "r.txt"]),
            vec!["fetter", "python", "site-install", "-b", "r.txt"]
        );
        assert_eq!(m(&["fetter", "scan"]), vec!["fetter", "scan"]);
        let cli = Cli::try_parse_from(migrate_args(
            ["fetter", "-e", "python3", "site-uninstall"]
                .iter()
                .map(OsString::from)
                .collect(),
        ))
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Python {
                command: PythonCommands::SiteUninstall
            })
        ));
        // unpack and purge commands are shared, top-level commands
        let cli = parse(&["fetter", "unpack-count", "-p", "pip*"]);
        assert!(matches!(cli.command, Some(Commands::UnpackCount { .. })));
        assert!(Cli::try_parse_from(["fetter", "python", "unpack-count"]).is_err());
    }

    #[test]
    fn test_resolve_ecosystems_a() {
        let scan = parse(&["fetter", "scan"]).command;
        assert_eq!(
            resolve_ecosystems(&[], scan.as_ref()).unwrap(),
            Ecosystem::ALL.to_vec()
        );
        assert_eq!(
            resolve_ecosystems(&[Ecosystem::Npm], scan.as_ref()).unwrap(),
            vec![Ecosystem::Npm]
        );
    }

    #[test]
    fn test_resolve_ecosystems_b() {
        let py = parse(&["fetter", "python", "site-uninstall"]).command;
        assert_eq!(
            resolve_ecosystems(&[], py.as_ref()).unwrap(),
            vec![Ecosystem::Python]
        );
        assert!(resolve_ecosystems(&[Ecosystem::Npm], py.as_ref()).is_err());

        // lookup-name: exactly one ecosystem, Python by default
        let lookup = parse(&["fetter", "lookup-name", "numpy"]).command;
        assert_eq!(
            resolve_ecosystems(&[], lookup.as_ref()).unwrap(),
            vec![Ecosystem::Python]
        );
        assert_eq!(
            resolve_ecosystems(&[Ecosystem::Cargo], lookup.as_ref()).unwrap(),
            vec![Ecosystem::Cargo]
        );
        assert!(
            resolve_ecosystems(&[Ecosystem::Cargo, Ecosystem::Npm], lookup.as_ref())
                .is_err()
        );
        // lookup-bound: like other commands (all by default)
        let bound = parse(&["fetter", "lookup-bound"]).command;
        assert_eq!(
            resolve_ecosystems(&[], bound.as_ref()).unwrap(),
            Ecosystem::ALL.to_vec()
        );
    }
}
