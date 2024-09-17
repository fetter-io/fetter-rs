use std;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    fetter::run_cli(std::env::args_os());
    Ok(())
}
