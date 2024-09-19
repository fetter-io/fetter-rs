mod count_report;
mod dep_manifest;
mod dep_spec;
mod exe_search;
mod package;
mod package_durl;
mod scan_fs;
mod scan_report;
mod validation_report;
mod version_spec;
mod util;

use std::process;

use clap::{Parser, Subcommand, ValueEnum};
use std::ffi::OsString;
use std::path::PathBuf;
use validation_report::ValidationFlags;

use crate::dep_manifest::DepManifest;
use crate::scan_fs::Anchor;
use crate::scan_fs::ScanFS;

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

        /// If the subset flag is set, observed packages must be a strict subset of the bound requirements.
        #[arg(short, long)]
        subset: bool,

        #[command(subcommand)]
        subcommands: ValidateSubcommand,
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
//------------------------------------------------------------------------------

// Get a ScanFS, optionally using exe_paths if provided
fn get_scan(exe_paths: Option<Vec<PathBuf>>) -> Result<ScanFS, String> {
    if let Some(exe_paths) = exe_paths {
        ScanFS::from_exes(exe_paths)
    } else {
        ScanFS::from_exe_scan()
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
        println!("For more information, try '--help'.");
        return;
    }
    // we always do a scan; we might cache this
    let sfs = get_scan(cli.exe).unwrap(); // handle error

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
            subcommands,
        }) => {
            let dm = get_dep_manifest(bound).unwrap(); // TODO: handle error
            let report_sites = false;
            let permit_unspecified = !subset;
            let vr = sfs.to_validation_report(
                dm,
                ValidationFlags {
                    permit_unspecified,
                    report_sites,
                },
            );
            match subcommands {
                ValidateSubcommand::Display => {
                    vr.to_stdout();
                }
                ValidateSubcommand::Write { output, delimiter } => {
                    let _ = vr.to_file(output, *delimiter);
                }
                ValidateSubcommand::Exit { code } => {
                    process::exit(if vr.len() > 0 { *code } else { 0 });
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

//------------------------------------------------------------------------------
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
