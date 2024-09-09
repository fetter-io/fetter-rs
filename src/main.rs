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
    #[command(about = "Validate packages in an environment.")]
    Validate {
        #[arg(short, long, value_name = "FILE")]
        bound: Option<PathBuf>,

        #[command(subcommand)]
        validate_command: ValidateCommand,

    },
}

#[derive(Subcommand)]
enum ValidateCommand {
    Display,
    Write {
        #[arg(short, long)]
        output: String,
    },

}




//------------------------------------------------------------------------------

// fn main() {
//     let sfs = ScanFS::from_defaults().unwrap();
//     sfs.report();
// }

fn main() {
    let cli = Cli::parse();

    // if let Some(name) = cli.name.as_deref() {
    //     println!("Value for name: {name}");
    // }


    // You can check for the existence of subcommands, and if found use their
    // matches just as you would the top level cmd
    match &cli.command {
        Some(Commands::Validate { bound, ..}) => {
            if bound.is_some() {
                println!("bounds...");
            } else {
                println!("No bounds...");
            }
        }
        None => {}
    }
}
