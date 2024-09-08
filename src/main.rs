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
    /// Optional name to operate on
    name: Option<String>,

    /// Sets a custom config file
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Turn debugging information on
    // #[arg(short, long, action = clap::ArgAction::Count)]
    // debug: u8,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// does testing things
    Validate {
        /// lists test values
        #[arg(short, long)]
        list: bool,
    },
}

//------------------------------------------------------------------------------

// fn main() {
//     let sfs = ScanFS::from_defaults().unwrap();
//     sfs.report();
// }

fn main() {
    let cli = Cli::parse();

    if let Some(name) = cli.name.as_deref() {
        println!("Value for name: {name}");
    }

    if let Some(config_path) = cli.config.as_deref() {
        println!("Value for config: {}", config_path.display());
    }

    // You can check for the existence of subcommands, and if found use their
    // matches just as you would the top level cmd
    match &cli.command {
        Some(Commands::Validate { list }) => {
            if *list {
                println!("Printing testing lists...");
            } else {
                println!("Not printing testing lists...");
            }
        }
        None => {}
    }
}
