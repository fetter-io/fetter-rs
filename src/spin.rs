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

use std::time::{SystemTime, UNIX_EPOCH};

// Seconds into the current time today.
fn sec_now() -> usize {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("?");
    let sec_now = now.as_secs();
    let sec_per_day = 24 * 60 * 60;
    (sec_now % sec_per_day) as usize
}

// Logistic Map generator
struct LogiMapGen {
    r: f64, // control
    x: f64, // state
}

impl LogiMapGen {
    fn from_steps(r: f64, x0: f64, steps: usize) -> Self {
        let mut gen = LogiMapGen { r, x: x0 };
        gen.advance(steps);
        gen
    }

    fn from_now(r: f64, x0: f64) -> Self {
        LogiMapGen::from_steps(r, x0, sec_now())
    }

    fn advance(&mut self, steps: usize) {
        for _ in 0..steps {
            self.x = self.r * self.x * (1.0 - self.x);
        }
    }
    // fn next(&mut self) -> char {
    //     self.advance(1);
    //     let norm = (self.x * 255.0).clamp(0.0, 255.0);
    //     let point = 0x2800 + norm as u32;
    //     char::from_u32(point).unwrap_or('?')
    // }

    fn next(&mut self) -> char {
        self.advance(1);
        // let norm = (self.x * 63.0).clamp(0.0, 63.0);
        let norm = (self.x * 255.0).clamp(0.0, 255.0);
        // 6-dot Braille Unicode character (U+2800 to U+283F)
        let point = 0x2800 + norm as u32;
        char::from_u32(point).unwrap_or('?')
    }
}

pub(crate) fn spin(active: Arc<AtomicBool>, delay_init: Duration) {
    // let frame_spin = vec!["⣿", "⣶", "⠿",  "⠶", "⠛", "⠉"];
    let mut gen = LogiMapGen::from_now(3.600001, 0.71111);
    let mut stdout = stdout();
    // let mut frame_idx = 0;

    thread::spawn(move || {
        thread::sleep(delay_init);
        if active.load(Ordering::Relaxed) {
            stdout.execute(Clear(ClearType::CurrentLine)).unwrap();
            while active.load(Ordering::Relaxed) {
                stdout.execute(cursor::MoveToColumn(0)).unwrap();
                // let fs = frame_spin[frame_idx % frame_spin.len()];
                let fs = gen.next();
                print!("{} fettering...", fs);
                stdout.flush().unwrap();
                thread::sleep(Duration::from_millis(100));
                // frame_idx += 1;
            }
            stdout.execute(cursor::MoveToColumn(0)).unwrap();
            stdout.execute(Clear(ClearType::CurrentLine)).unwrap();
        }
    });
}
