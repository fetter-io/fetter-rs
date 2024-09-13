use std;

// // NEXT:
// // Packages collect url for @ validations
// // Implement a colorful display
// // Implement a monitoring mode

fn main() -> Result<(), Box<dyn std::error::Error>> {
    fetter::run_cli(std::env::args_os());
    Ok(())
}
