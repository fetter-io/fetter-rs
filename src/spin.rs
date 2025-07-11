use crossterm::tty::IsTty;
use crossterm::{
    cursor,
    terminal::{Clear, ClearType},
    ExecutableCommand,
};
use std::io::Write;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::Duration;

use crate::util::get_writer;
use crate::write_color::write_color;

const FETTER_VERSION: &str = env!("CARGO_PKG_VERSION");

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

fn get_banner(message: Option<String>) -> String {
    let msg = message.map_or(String::new(), |m| format!(": {m}"));
    format!("fetter {FETTER_VERSION}{msg}\n")
}

pub(crate) fn print_banner(is_failure: bool, message: Option<String>, stderr: bool) {
    let mut writer = get_writer(stderr);
    if is_failure {
        write_color(&mut writer, "#cc0000", "Failed: ");
    }
    write_color(&mut writer, "#999999", &get_banner(message));
}

pub(crate) fn spin(active: Arc<AtomicBool>, message: String, stderr: bool) {
    let mut writer = get_writer(stderr);
    if !writer.is_tty() {
        return;
    }
    let mut frame_idx = 0;

    thread::spawn(move || {
        // wait 1 sec to avoid starting for fast searches
        let delay_init = Duration::from_secs(1);
        thread::sleep(delay_init);
        if active.load(Ordering::Relaxed) {
            writer.execute(Clear(ClearType::CurrentLine)).unwrap();
            while active.load(Ordering::Relaxed) {
                writer.execute(cursor::MoveToColumn(0)).unwrap();
                let fs = FRAME_SPIN[frame_idx % FRAME_SPIN.len()];
                let msg = format!("{fs} {message}... ");
                write_color(&mut writer, "#666666", &msg);
                writer.flush().unwrap();
                thread::sleep(Duration::from_millis(80));
                frame_idx += 1;
            }
            writer.execute(cursor::MoveToColumn(0)).unwrap();
            writer.execute(Clear(ClearType::CurrentLine)).unwrap();
        }
    });
}
