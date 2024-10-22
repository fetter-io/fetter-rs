use crossterm::tty::IsTty;
use crossterm::{
    cursor,
    terminal::{Clear, ClearType},
    ExecutableCommand,
};
use std::io::{stdout, Write};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::Duration;

use crate::table::write_color;

pub(crate) fn spin(active: Arc<AtomicBool>) {
    let frame_spin = vec!["────", "•───", "••──", "•••─", "─•••", "──••", "───•"];
    // let frame_spin = vec![
    //     "▏", "▎", "▍", "▌", "▋", "▊", "▉", "▊", "▋", "▌", "▍", "▎", "▏", " ",
    // ];
    // let frame_spin = vec!["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█", "▇", "▆", "▅", "▄", "▃", "▂", "▁", " "];
    // let frame_spin = vec!["○─•  ", "◉──• ", "◎───•", "◉──• ", "○─•  "];

    let mut stdout = stdout();
    if !stdout.is_tty() {
        return;
    }
    let mut frame_idx = 0;

    thread::spawn(move || {
        // wait 1 sec to avoid starting for fast searches
        let delay_init = Duration::from_secs(1);
        thread::sleep(delay_init);
        if active.load(Ordering::Relaxed) {
            stdout.execute(Clear(ClearType::CurrentLine)).unwrap();
            while active.load(Ordering::Relaxed) {
                stdout.execute(cursor::MoveToColumn(0)).unwrap();
                let fs = frame_spin[frame_idx % frame_spin.len()];
                let msg = format!("{} fettering... ", fs);
                write_color(&mut stdout, 120, 120, 120, &msg);
                stdout.flush().unwrap();
                thread::sleep(Duration::from_millis(100));
                frame_idx += 1;
            }
            stdout.execute(cursor::MoveToColumn(0)).unwrap();
            stdout.execute(Clear(ClearType::CurrentLine)).unwrap();
        }
    });
}
