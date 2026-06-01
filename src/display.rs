use std::io::{IsTerminal, Write, stderr};
use std::thread::{JoinHandle, spawn};
use std::time::{Duration, Instant};

use flume::RecvTimeoutError;

use crate::format::truncate;

/// Messages sent to the display thread. Workers address a worker line by index;
/// feeders address a command counter line by index.
pub enum DisplayMessage {
    Start(usize, usize), // worker, command index
    Calibrate(usize),
    Idle(usize),
    Counters(Vec<String>),
    Done,
}

/// What a worker line is showing.
enum WorkerState {
    Idle,
    Running(usize),
    Calibrating,
}

const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏', ' '];
const FRAME: Duration = Duration::from_millis(500);

/// Query the terminal width via the controlling tty, defaulting to 80 columns.
pub fn term_width() -> usize {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDERR_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 {
            ws.ws_col as usize
        } else {
            80
        }
    }
}

/// Truncate to at most `cols - 1` characters so a line never fills the terminal
/// width (which would wrap and corrupt the cursor math of the in-place redraw).
fn fit(line: &str, cols: usize) -> String {
    line.chars().take(cols.saturating_sub(1)).collect()
}

pub fn spawn_display(
    jobs: usize,
    command_labels: Vec<String>,
    rx: flume::Receiver<DisplayMessage>,
) -> JoinHandle<()> {
    spawn(move || {
        // Off a terminal (e.g. piped) we draw nothing and just wait for the end.
        if !stderr().is_terminal() {
            while let Ok(msg) = rx.recv() {
                if matches!(msg, DisplayMessage::Done) {
                    break;
                }
            }
            return;
        }

        // What each worker is showing. Running holds the command index; the
        // label is looked up in `command_labels` at draw time, so nothing is
        // allocated per task start and it still reflows on resize.
        let mut worker_state: Vec<WorkerState> = (0..jobs).map(|_| WorkerState::Idle).collect();
        let mut counter_msgs: Vec<String> = command_labels
            .iter()
            .map(|label| format!("{label}: pending"))
            .collect();
        let n_lines = worker_state.len() + counter_msgs.len();

        let mut frame = 0usize;
        let mut first = true;
        let mut last_anim = Instant::now();

        let draw =
            |frame: usize, worker_state: &[WorkerState], counter_msgs: &[String], first: bool| {
                let cols = term_width();
                let glyph = SPINNER[frame % SPINNER.len()];
                let mut out = stderr().lock();
                if !first {
                    let _ = write!(out, "\x1b[{n_lines}A");
                }
                for (w, state) in worker_state.iter().enumerate() {
                    let prefix = format!(" {glyph} worker {w}: ");
                    let line = match state {
                        WorkerState::Running(idx) => {
                            // Reserve the prefix and the column fit() trims.
                            let budget = cols.saturating_sub(1 + prefix.chars().count());
                            format!("{prefix}{}", truncate(&command_labels[*idx], budget))
                        }
                        WorkerState::Calibrating => format!("{prefix}<calibration>"),
                        WorkerState::Idle => format!("{prefix}idle"),
                    };
                    let _ = write!(out, "\r\x1b[2K{}\n", fit(&line, cols));
                }
                for msg in counter_msgs {
                    let _ = write!(out, "\r\x1b[2K{}\n", fit(&format!("   {msg}"), cols));
                }
                let _ = out.flush();
            };

        draw(frame, &worker_state, &counter_msgs, first);
        first = false;

        loop {
            match rx.recv_timeout(FRAME) {
                Ok(DisplayMessage::Start(w, idx)) => worker_state[w] = WorkerState::Running(idx),
                Ok(DisplayMessage::Calibrate(w)) => worker_state[w] = WorkerState::Calibrating,
                Ok(DisplayMessage::Idle(w)) => worker_state[w] = WorkerState::Idle,
                Ok(DisplayMessage::Counters(lines)) => counter_msgs = lines,
                Ok(DisplayMessage::Done) | Err(RecvTimeoutError::Disconnected) => {
                    // Move to the top of our block and erase everything below it.
                    let mut out = stderr().lock();
                    let _ = write!(out, "\x1b[{n_lines}A\x1b[J");
                    let _ = out.flush();
                    break;
                }
                Err(RecvTimeoutError::Timeout) => {}
            }

            if last_anim.elapsed() >= FRAME {
                frame = frame.wrapping_add(1);
                last_anim = Instant::now();
            }
            draw(frame, &worker_state, &counter_msgs, first);
        }
    })
}
