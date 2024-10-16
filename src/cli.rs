use std::process;

use crate::validation_report::ValidationFlags;
use clap::{Parser, Subcommand, ValueEnum};
use std::ffi::OsString;
use std::path::PathBuf;

use crate::dep_manifest::DepManifest;
use crate::scan_fs::Anchor;
use crate::scan_fs::ScanFS;
use crate::table::Tableable;

//------------------------------------------------------------------------------
// utility enums

#[derive(Copy, Clone, ValueEnum)]
enum CliAnchor {
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

//------------------------------------------------------------------------------

const AFTER_HELP: &str = "\
Examples:
  fetter scan display
  fetter scan write -o /tmp/pkgscan.txt --delimiter '|'

  fetter search --pattern pip* display

  fetter count display

  fetter --exe python3 derive -a lower write -o /tmp/bound_requirements.txt

  fetter validate --bound /tmp/bound_requirements.txt display
  fetter --exe python3 validate --bound /tmp/bound_requirements.txt display

  fetter purge --bound /tmp/bound_requirements.txt
";

#[derive(clap::Parser)]
#[command(version, about, long_about = None, after_help = AFTER_HELP)]
struct Cli {
    /// Zero or more executable paths to derive site package locations. If not provided, all discoverable executables will be used.
    #[arg(short, long, value_name = "FILES", required = false)]
    exe: Option<Vec<PathBuf>>,

    /// Force inclusion of the user site-packages, even if it is not activated. If not set, user site packages will only be included if the interpreter has been configured to use it.
    #[arg(long, required = false)]
    user_site: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Scan environment to report on installed packages.
    Scan {
        #[command(subcommand)]
        subcommands: ScanSubcommand,
    },
    /// Search environment to report on installed packages.
    Search {
        #[arg(short, long)]
        pattern: String,

        #[arg(short, long)]
        case: bool,

        #[command(subcommand)]
        subcommands: SearchSubcommand,
    },
    /// Count discovered executables and installed packages.
    Count {
        #[command(subcommand)]
        subcommands: CountSubcommand,
    },
    /// Derive new requirements from discovered packages.
    Derive {
        // Select the nature of the bound in the derived requirements.
        #[arg(short, long, value_enum)]
        anchor: CliAnchor,

        #[command(subcommand)]
        subcommands: DeriveSubcommand,
    },
    /// Validate if packages conform to a validation target.
    Validate {
        /// File path from which to read bound requirements.
        #[arg(short, long, value_name = "FILE")]
        bound: Option<PathBuf>,

        /// If the subset flag is set, the observed packages can be a subset of the bound requirements.
        #[arg(long)]
        subset: bool,

        /// If the superset flag is set, the observed packages can be a superset of the bound requirements.
        #[arg(long)]
        superset: bool,

        #[command(subcommand)]
        subcommands: ValidateSubcommand,
    },
    /// Search for vulnerabilities on observed packages.
    Audit {
        #[command(subcommand)]
        subcommands: AuditSubcommand,
    },
    /// Purge packages that fail validation
    Purge {
        /// File path from which to read bound requirements.
        #[arg(short, long, value_name = "FILE")]
        bound: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ScanSubcommand {
    /// Display scan to the terminal.
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
enum SearchSubcommand {
    /// Display search to the terminal.
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
    /// Display scan to the terminal.
    Display,
    /// Write a report to a file.
    Write {
        #[arg(short, long, value_name = "FILE")]
        output: PathBuf,
        #[arg(short, long, default_value = ",")]
        delimiter: char,
    },
}

#[derive(Subcommand)]
enum DeriveSubcommand {
    /// Display derive to the terminal.
    Display,
    /// Write a derive report to a file.
    Write {
        #[arg(short, long, value_name = "FILE")]
        output: PathBuf,
    },
}

#[derive(Subcommand)]
enum ValidateSubcommand {
    /// Display validation to the terminal.
    Display,
    /// Print a JSON representation of validation results.
    JSON,
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
enum AuditSubcommand {
    /// Display validation to the terminal.
    Display,
    /// Print a JSON representation of validation results.
    Write {
        #[arg(short, long, value_name = "FILE")]
        output: PathBuf,
        #[arg(short, long, default_value = ",")]
        delimiter: char,
    },
}
//------------------------------------------------------------------------------

// Get a ScanFS, optionally using exe_paths if provided
fn get_scan(
    exe_paths: Option<Vec<PathBuf>>,
    force_usite: bool,
) -> Result<ScanFS, String> {
    if let Some(exe_paths) = exe_paths {
        ScanFS::from_exes(exe_paths, force_usite)
    } else {
        ScanFS::from_exe_scan(force_usite)
    }
}

fn get_dep_manifest(bound: &Option<PathBuf>) -> Result<DepManifest, String> {
    if let Some(bound) = bound {
        DepManifest::from_requirements(bound)
    } else {
        Err("Invalid bound path".to_string())
    }
}

// TODO: return Result type with errors
pub fn run_cli<I, T>(args: I)
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = Cli::parse_from(args);

    if cli.command.is_none() {
        println!("No command provided. For more information, try '--help'.");
        return;
    }
    // we always do a scan; we might cache this
    let sfs = get_scan(cli.exe, cli.user_site).unwrap(); // handle error

    match &cli.command {
        Some(Commands::Scan { subcommands }) => match subcommands {
            ScanSubcommand::Display => {
                let sr = sfs.to_scan_report();
                let _ = sr.to_stdout();
            }
            ScanSubcommand::Write { output, delimiter } => {
                let sr = sfs.to_scan_report();
                let _ = sr.to_file(output, *delimiter);
            }
        },
        Some(Commands::Search {
            subcommands,
            pattern,
            case,
        }) => match subcommands {
            SearchSubcommand::Display => {
                let sr = sfs.to_search_report(&pattern, !case);
                let _ = sr.to_stdout();
            }
            SearchSubcommand::Write { output, delimiter } => {
                let sr = sfs.to_search_report(&pattern, !case);
                let _ = sr.to_file(output, *delimiter);
            }
        },
        Some(Commands::Count { subcommands }) => match subcommands {
            CountSubcommand::Display => {
                let cr = sfs.to_count_report();
                let _ = cr.to_stdout();
            }
            CountSubcommand::Write { output, delimiter } => {
                let cr = sfs.to_count_report();
                let _ = cr.to_file(output, *delimiter);
            }
        },
        Some(Commands::Derive {
            subcommands,
            anchor,
        }) => {
            match subcommands {
                DeriveSubcommand::Display => {
                    let dm = sfs.to_dep_manifest((*anchor).into()).unwrap();
                    dm.to_stdout();
                }
                DeriveSubcommand::Write { output } => {
                    let dm = sfs.to_dep_manifest((*anchor).into()).unwrap();
                    // TODO: might have a higher-order func that branches based on extension between txt and json
                    let _ = dm.to_requirements(output);
                }
            }
        }
        Some(Commands::Validate {
            bound,
            subset,
            superset,
            subcommands,
        }) => {
            let dm = get_dep_manifest(bound).unwrap(); // TODO: handle error
            let permit_superset = *superset;
            let permit_subset = *subset;
            let vr = sfs.to_validation_report(
                dm,
                ValidationFlags {
                    permit_superset,
                    permit_subset,
                },
            );
            match subcommands {
                ValidateSubcommand::Display => {
                    let _ = vr.to_stdout();
                }
                ValidateSubcommand::JSON => {
                    println!(
                        "{}",
                        serde_json::to_string(&vr.to_validation_digest()).unwrap()
                    );
                }
                ValidateSubcommand::Write { output, delimiter } => {
                    let _ = vr.to_file(output, *delimiter);
                }
                ValidateSubcommand::Exit { code } => {
                    process::exit(if vr.len() > 0 { *code } else { 0 });
                }
            }
        }
        Some(Commands::Audit { subcommands }) => {
            let ar = sfs.to_audit_report();
            match subcommands {
                AuditSubcommand::Display => {
                    let _ = ar.to_stdout();
                }
                AuditSubcommand::Write { output, delimiter } => {
                    let _ = ar.to_file(output, *delimiter);
                }
            }
        }
        Some(Commands::Purge { bound }) => {
            let _dm = get_dep_manifest(bound);
            println!("purge");
        }
        None => {}
    }
}

//-----------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    // use super::*;
    use std::ffi::OsString;

    #[test]
    fn test_run_cli_a() {
        let _args = vec![OsString::from("fetter"), OsString::from("-h")];
        // run_cli(args); // print to stdout
    }
}
