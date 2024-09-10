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
// Need to implment to_file() on DepManifest
// Implement command line entry points
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
    /// Analyze environment to report on installed packages.
    Analyze {
        #[command(subcommand)]
        analyze_subcommand: AnalyzeSubcommand,
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
enum AnalyzeSubcommand {
    /// Display analysis to the terminal.
    Display,
    /// Write a analysis report to a file.
    Write {
        #[arg(short, long, value_name = "FILE")]
        output: PathBuf,
    },
}



//------------------------------------------------------------------------------
// Analyze: report information on system
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
        Some(Commands::Analyze { analyze_subcommand }) => {
            match analyze_subcommand {
                AnalyzeSubcommand::Display => {
                    let sfs = ScanFS::from_defaults().unwrap();
                    sfs.report_scan();
                }
                AnalyzeSubcommand::Write { output } => {
                    let sfs = ScanFS::from_defaults().unwrap();
                    let dm = sfs.to_dep_manifest();
                    println!("{:?}", dm);
                },
            }
        }
        None => {}
    }
}
