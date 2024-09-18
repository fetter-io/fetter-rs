use std;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    fetter::run_cli(std::env::args_os());
    Ok(())
}



// TODO:
// to_validation_report takes a struct of flags
// test perimit_unspecified
// expose cli args

