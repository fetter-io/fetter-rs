mod dep_manifest;
mod dep_spec;
mod exe_search;
mod package;
mod scan_fs;
mod version_spec;
use crate::scan_fs::ScanFS;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

// NEXT:
// write a requurements bound file based on ScanFSs
// takes a requirements bound file and validates
// Implement a colorful display
// Implement a monitoring mode

#[derive(clap::Parser)]
#[command(version, about, long_about = None)]
struct Cli {
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
        validate_subcommand: ValidateSubcommand,
    },
    /// Scan environment to report on installed packages.
    Scan {
        #[command(subcommand)]
        scan_subcommand: ScanSubcommand,
    },
    /// Scan environment to report on installed packages.
    Derive {
        #[command(subcommand)]
        derive_subcommand: DeriveSubcommand,
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
// Scan: report information on system
// Validate: test system against bound file
// Derive: produce a requirements file from system condtions
// Purge: remove unvalid packages

fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Some(Commands::Validate { bound, validate_subcommand}) => {
            if bound.is_some() {
                println!("got bound");
            }
            match validate_subcommand {
                ValidateSubcommand::Display => {
                    println!("got display");
                }
                ValidateSubcommand::Write { output } => {
                    println!("got write");
                },
            }
        }
        Some(Commands::Scan { scan_subcommand }) => {
            match scan_subcommand {
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
        // TODO: need parameter for min, max, eq
        Some(Commands::Derive { derive_subcommand }) => {
            match derive_subcommand {
                DeriveSubcommand::Display => {
                    let sfs = ScanFS::from_defaults().unwrap();
                    let dm = sfs.to_dep_manifest().unwrap();
                    dm.display();
                }
                DeriveSubcommand::Write { output } => {
                    let sfs = ScanFS::from_defaults().unwrap();
                    let dm = sfs.to_dep_manifest().unwrap();
                    // TODO: might have a higher-order func that branches based on extension between txt and json
                    dm.to_requirements(output);
                },
            }
        }
        None => {}
    }
}
