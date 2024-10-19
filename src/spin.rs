use std::io::{stdout, Write};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::thread;
use std::time::Duration;
use crossterm::{
    ExecutableCommand,
    cursor,
    terminal::{Clear, ClearType},
};

pub(crate) fn spin(active: Arc<AtomicBool>, delay_init: Duration) {
    let frame_spin = vec!["-", "/", "-", "\\"];
    // let frame_elipsis = vec!["...", "...", "..."];

    let mut stdout = stdout();
    let mut frame_idx = 0;

    thread::spawn(move || {
        thread::sleep(delay_init);
        if active.load(Ordering::Relaxed) {
            stdout.execute(Clear(ClearType::CurrentLine)).unwrap();
            while active.load(Ordering::Relaxed) {
                stdout.execute(cursor::MoveToColumn(0)).unwrap();
                let fs = frame_spin[frame_idx % frame_spin.len()];
                // let fe = frame_elipsis[frame_idx % frame_elipsis.len()];
                print!("{} fettering...", fs);
                stdout.flush().unwrap();
                thread::sleep(Duration::from_millis(50));
                frame_idx += 1;
            }
            stdout.execute(cursor::MoveToColumn(0)).unwrap();
            stdout.execute(Clear(ClearType::CurrentLine)).unwrap();
        }
    });
}
