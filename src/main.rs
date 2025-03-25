use std::io::stderr;
use std::sync::Arc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Arc::new(fetter::UreqClientLive);

    if let Err(e) = fetter::run_cli(std::env::args_os(), client) {
        let mut stderr = stderr();
        fetter::write_color(&mut stderr, "#666666", "fetter ");
        fetter::write_color(&mut stderr, "#cc0000", "Error: ");
        eprintln!("{}", e);
        std::process::exit(1);
    }
    Ok(())
}
