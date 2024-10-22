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

// we duplicate each component so we can update frames faster while keeping the visual changes slow
const FRAME_SPIN: [&str; 20] = [
    "·", "·", "•", "•", "○", "○", "◉", "◉", "◎", "◎", "◉", "◉", "○", "○", "•", "•", "·",
    "·", " ", " ",
];

// vec!["◦", "•", "○", "◉", "◎", "◯", "◎", "◉", "○", "•", "◦", " "]
// vec!["────", "•───", "••──", "•••─", "─•••", "──••", "───•"];
// vec!["▏", "▎", "▍", "▌", "▋", "▊", "▉", "▊", "▋", "▌", "▍", "▎", "▏", " "];
// vec!["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█", "▇", "▆", "▅", "▄", "▃", "▂", "▁", " "];
// vec!["○─•  ", "◉──• ", "◎───•", "◉──• ", "○─•  "];

pub(crate) fn spin(active: Arc<AtomicBool>) {
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
                let fs = FRAME_SPIN[frame_idx % FRAME_SPIN.len()];
                let msg = format!("{} fettering... ", fs);
                write_color(&mut stdout, 120, 120, 120, &msg);
                stdout.flush().unwrap();
                thread::sleep(Duration::from_millis(80));
                frame_idx += 1;
            }
            stdout.execute(cursor::MoveToColumn(0)).unwrap();
            stdout.execute(Clear(ClearType::CurrentLine)).unwrap();
        }
    });
}
