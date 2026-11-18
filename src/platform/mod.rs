// Everything OS-specific lives here: raw mode, terminal size, waiting for
// input, interrupt handling and the clipboard. The rest of csview only
// speaks VT escape sequences, which both backends hand it as bytes.

use std::sync::atomic::AtomicBool;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

pub enum PollEvent {
    Data(usize),
    Sig,
}

#[derive(Clone, Copy)]
pub enum RawMode {
    // the main view: ctrl + c / ctrl + \ interrupt instead of arriving as bytes
    Tui,
    // line editing after exit: every key, ctrl + c included, arrives as bytes
    Line,
}

// prevent interrupts from closing program when in command mode
pub static IN_COMMAND: AtomicBool = AtomicBool::new(false);

// set by signal handlers on unix and by the input reader on windows
pub static GOT_WINCH: AtomicBool = AtomicBool::new(false);
pub static GOT_INT: AtomicBool = AtomicBool::new(false);
pub static GOT_QUIT: AtomicBool = AtomicBool::new(false);
