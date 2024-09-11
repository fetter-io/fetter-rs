mod dep_manifest;
mod dep_spec;
mod exe_search;
mod package;
mod scan_fs;
mod version_spec;
use crate::scan_fs::ScanFS;
use crate::scan_fs::Anchor;

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
#[derive(clap::Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

// Scan: report information on system
// Validate: test system against bound file
// Derive: produce a requirements file from system condtions
// Purge: remove unvalid packages

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
fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Some(Commands::Validate { bound, subcommands}) => {
            if bound.is_some() {
                println!("got bound");
            }
            match subcommands {
                ValidateSubcommand::Display => {
                    println!("got display");
                }
                ValidateSubcommand::Write { output } => {
                    println!("got write");
                },
            }
        }
        Some(Commands::Scan { subcommands }) => {
            match subcommands {
                ScanSubcommand::Display => {
                    let sfs = ScanFS::from_defaults().unwrap();
                    sfs.display();
                }
                ScanSubcommand::Write { output } => {
                    let sfs = ScanFS::from_defaults().unwrap();
                    sfs.display();
                },
            }
        }
        Some(Commands::Derive { subcommands, anchor }) => {
            match subcommands {
                DeriveSubcommand::Display => {
                    let sfs = ScanFS::from_defaults().unwrap();
                    let dm = sfs.to_dep_manifest((*anchor).into()).unwrap();
                    dm.display();
                }
                DeriveSubcommand::Write { output } => {
                    let sfs = ScanFS::from_defaults().unwrap();
                    let dm = sfs.to_dep_manifest((*anchor).into()).unwrap();
                    // TODO: might have a higher-order func that branches based on extension between txt and json
                    let _ = dm.to_requirements(output);
                },
            }
        }
        None => {}
    }
}
