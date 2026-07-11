use std::io::{IsTerminal, Write, stderr};
use std::thread::{JoinHandle, spawn};
use std::time::{Duration, Instant};

use flume::RecvTimeoutError;

use crate::format::truncate;

/// Messages sent to the display thread. Workers address a worker line by index;
/// feeders address a command counter line by index. The display stops when
/// every sender is dropped (channel disconnection), so there is no explicit
/// termination message.
pub enum DisplayMessage {
    Start(usize, usize), // worker, command index
    Calibrate(usize),
    Idle(usize),
    Counters(Vec<String>),
}

/// What a worker line is showing.
#[derive(Clone, Copy)]
enum WorkerState {
    Idle,
    Running(usize),
    Calibrating,
}

const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏', ' '];
const FRAME: Duration = Duration::from_millis(500);
/// Minimum delay between two redraws: messages arriving faster than this are
/// coalesced into one frame instead of each triggering a full redraw.
const MIN_REDRAW: Duration = Duration::from_millis(50);

/// Fold one message into the display state.
fn apply(msg: DisplayMessage, worker_state: &mut [WorkerState], counter_msgs: &mut Vec<String>) {
    match msg {
        DisplayMessage::Start(w, idx) => worker_state[w] = WorkerState::Running(idx),
        DisplayMessage::Calibrate(w) => worker_state[w] = WorkerState::Calibrating,
        DisplayMessage::Idle(w) => worker_state[w] = WorkerState::Idle,
        DisplayMessage::Counters(lines) => *counter_msgs = lines,
    }
}

/// Query the terminal size via the controlling tty as `(rows, cols)`,
/// defaulting to 24x80.
fn term_size() -> (usize, usize) {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDERR_FILENO, libc::TIOCGWINSZ, &mut ws) == 0
            && ws.ws_col > 0
            && ws.ws_row > 0
        {
            (ws.ws_row as usize, ws.ws_col as usize)
        } else {
            (24, 80)
        }
    }
}

/// Query the terminal width via the controlling tty, defaulting to 80 columns.
pub fn term_width() -> usize {
    term_size().1
}

/// Truncate to at most `cols - 1` visible characters so a line never fills the
/// terminal width (which would wrap and corrupt the cursor math of the
/// in-place redraw). ANSI escape sequences (the bold metric flag) take no
/// columns, so they don't count; a reset is appended in case the truncation
/// dropped the one closing the flag.
fn fit(line: &str, cols: usize) -> String {
    let budget = cols.saturating_sub(1);
    let mut out = String::with_capacity(line.len());
    let mut visible = 0;
    let mut in_escape = false;
    let mut had_escape = false;
    for c in line.chars() {
        if in_escape {
            out.push(c);
            in_escape = c != 'm';
        } else if c == '\x1b' {
            in_escape = true;
            had_escape = true;
            out.push(c);
        } else if visible < budget {
            out.push(c);
            visible += 1;
        } else {
            break;
        }
    }
    if had_escape {
        out.push_str("\x1b[0m");
    }
    out
}

pub fn spawn_display(
    jobs: usize,
    command_labels: Vec<String>,
    rx: flume::Receiver<DisplayMessage>,
    disabled: bool,
) -> JoinHandle<()> {
    spawn(move || {
        // Off a terminal (e.g. piped) we draw nothing and just wait for the
        // end; same when disabled (--output inherit: redrawing over the
        // children's own terminal writes would garble both).
        if disabled || !stderr().is_terminal() {
            while rx.recv().is_ok() {}
            return;
        }

        // What each worker is showing. Running holds the command index; the
        // label is looked up in `command_labels` at draw time, so nothing is
        // allocated per task start and it still reflows on resize.
        let mut worker_state = vec![WorkerState::Idle; jobs];
        let mut counter_msgs: Vec<String> = command_labels
            .iter()
            .map(|label| format!("{label}: pending"))
            .collect();
        let n_lines = worker_state.len() + counter_msgs.len();

        let mut frame = 0usize;
        let mut last_anim = Instant::now();

        // Each frame is one write: erase from the block's top (where the
        // previous frame parked the cursor) to the end of the screen, print
        // every line, and park back at the top. Anchoring on the block's first
        // character survives resizes: when the emulator rewraps the previous
        // frame it moves the parked cursor with the text, whereas counting a
        // fixed number of lines upward from below would land mid-block and
        // leave stale rows behind.
        let draw = |frame: usize, worker_state: &[WorkerState], counter_msgs: &[String]| {
            let (rows, cols) = term_size();
            let glyph = SPINNER[frame % SPINNER.len()];
            let mut lines: Vec<String> = Vec::with_capacity(n_lines);
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
                lines.push(fit(&line, cols));
            }
            for msg in counter_msgs {
                lines.push(fit(&format!("   {msg}"), cols));
            }
            // A frame taller than the screen would scroll on every redraw,
            // spraying stale copies into the scrollback; keep the tail (the
            // counters) and drop worker lines that don't fit.
            let visible = lines.len().min(rows.saturating_sub(1)).max(1);
            let mut buf = String::from("\x1b[?25l\r\x1b[J");
            for line in &lines[lines.len() - visible..] {
                buf.push_str(line);
                buf.push('\n');
            }
            buf.push_str(&format!("\x1b[{visible}A"));
            let mut out = stderr().lock();
            let _ = out.write_all(buf.as_bytes());
            let _ = out.flush();
        };

        let mut last_draw = Instant::now();
        let mut dirty = false;
        draw(frame, &worker_state, &counter_msgs);

        loop {
            // While a state change waits behind the redraw floor, wake when
            // the floor expires instead of a full frame later, so the last
            // update of a burst is never shown late.
            let timeout = if dirty {
                MIN_REDRAW.saturating_sub(last_draw.elapsed())
            } else {
                FRAME
            };
            match rx.recv_timeout(timeout) {
                Ok(msg) => {
                    // Drain everything queued behind the first message: the
                    // frame below reflects the latest state, without one
                    // redraw (and tty write) per message.
                    apply(msg, &mut worker_state, &mut counter_msgs);
                    while let Ok(msg) = rx.try_recv() {
                        apply(msg, &mut worker_state, &mut counter_msgs);
                    }
                    dirty = true;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    // The cursor is parked at the block's top: erase the block
                    // and restore the cursor.
                    let mut out = stderr().lock();
                    let _ = out.write_all(b"\r\x1b[J\x1b[?25h");
                    let _ = out.flush();
                    break;
                }
                Err(RecvTimeoutError::Timeout) => {}
            }

            if last_anim.elapsed() >= FRAME {
                frame = frame.wrapping_add(1);
                last_anim = Instant::now();
            }
            if last_draw.elapsed() >= MIN_REDRAW {
                last_draw = Instant::now();
                dirty = false;
                draw(frame, &worker_state, &counter_msgs);
            }
        }
    })
}
