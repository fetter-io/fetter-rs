mod dep_manifest;
mod dep_spec;
mod exe_search;
mod package;
mod scan_fs;
mod validation;
mod version_spec;

use crate::dep_manifest::DepManifest;
use crate::scan_fs::Anchor;
use crate::scan_fs::ScanFS;

use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

// NEXT:
// takes a requirements bound file and validates
// Implement a colorful display
// Implement a monitoring mode

//------------------------------------------------------------------------------
// utility enums

#[derive(Copy, Clone, ValueEnum)]
pub(crate) enum CliAnchor {
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
  fetter validate --bound requirements.txt display
  fetter --exe python3 validate --bound requirements.txt display

  fetter scan display
  fetter derive write -o /tmp/bond_requirements.txt
  fetter purge
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
    /// Validate packages in an environment
    Validate {
        /// File path from which to read bound requirements.
        #[arg(short, long, value_name = "FILE")]
        bound: Option<PathBuf>,

        #[command(subcommand)]
        subcommands: ValidateSubcommand,
    },
    /// Scan environment to report on installed packages.
    Scan {
        #[command(subcommand)]
        subcommands: ScanSubcommand,
    },
    /// Scan environment to report on installed packages.
    Derive {
        // Select the nature of the bound in the derived requirements.
        #[arg(short, long, value_enum)]
        anchor: CliAnchor,

        #[command(subcommand)]
        subcommands: DeriveSubcommand,
    },
    /// Purge packages that fail validation
    Purge {
        /// File path from which to read bound requirements.
        #[arg(short, long, value_name = "FILE")]
        bound: Option<PathBuf>,
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
    },
}

// might support output to requirements or toml?
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

fn main() {
    let cli = Cli::parse();
    // we always do a scan; we might cache this
    let sfs = get_scan(cli.exe).unwrap(); // handle error

    match &cli.command {
        Some(Commands::Validate { bound, subcommands }) => {
            let dm = get_dep_manifest(bound).unwrap();
            let v = sfs.validate(dm);
            match subcommands {
                ValidateSubcommand::Display => {
                    v.display();
                }
                ValidateSubcommand::Write { output } => {
                    println!("{:?}", v);
                }
            }
        }
        Some(Commands::Scan { subcommands }) => match subcommands {
            ScanSubcommand::Display => {
                sfs.display();
            }
            ScanSubcommand::Write { output } => {
                sfs.display();
            }
        },
        Some(Commands::Derive {
            subcommands,
            anchor,
        }) => {
            match subcommands {
                DeriveSubcommand::Display => {
                    let dm = sfs.to_dep_manifest((*anchor).into()).unwrap();
                    dm.display();
                }
                DeriveSubcommand::Write { output } => {
                    let dm = sfs.to_dep_manifest((*anchor).into()).unwrap();
                    // TODO: might have a higher-order func that branches based on extension between txt and json
                    let _ = dm.to_requirements(output);
                }
            }
        }
        Some(Commands::Purge { bound }) => {
            let dm = get_dep_manifest(bound);
            println!("purge");
        }

        None => {}
    }
}
