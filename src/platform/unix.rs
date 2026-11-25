use std::{
    io::{self, Read, Write},
    mem,
    process::{Command, Stdio},
    sync::atomic::Ordering,
};
use libc::{c_int, STDIN_FILENO, STDOUT_FILENO};

use super::{PollEvent, RawMode, GOT_INT, GOT_QUIT, GOT_WINCH, IN_COMMAND};

pub type SavedMode = libc::termios;

// (width, height) of the terminal, if stdout is one
pub fn term_size() -> Option<(usize, usize)> {
    unsafe {
        let mut ws: libc::winsize = mem::zeroed();
        if libc::ioctl(STDOUT_FILENO, libc::TIOCGWINSZ.into(), &mut ws) == 0 {
            Some((ws.ws_col as usize, ws.ws_row as usize))
        } else {
            None
        }
    }
}

// switch stdin to raw mode; returns the previous mode, or None if stdin isn't a terminal
pub fn enter_raw(mode: RawMode) -> Option<SavedMode> {
    unsafe {
        let mut orig: libc::termios = mem::zeroed();
        if libc::tcgetattr(STDIN_FILENO, &mut orig) != 0 {
            return None;
        }
        let mut raw = orig;
        libc::cfmakeraw(&mut raw);
        match mode {
            RawMode::Tui => {
                //re-enable signals
                raw.c_lflag |= libc::ISIG;
                // turn on non-blocking to catch sigs
                raw.c_cc[libc::VMIN] = 0;
                raw.c_cc[libc::VTIME] = 0;
            }
            RawMode::Line => {
                raw.c_cc[libc::VMIN] = 1;
                raw.c_cc[libc::VTIME] = 0;
            }
        }
        libc::tcsetattr(STDIN_FILENO, libc::TCSANOW, &raw);
        Some(orig)
    }
}

pub fn restore(saved: &SavedMode) {
    unsafe {
        libc::tcsetattr(STDIN_FILENO, libc::TCSANOW, saved);
    }
}

// block until stdin has data or a signal arrives
pub fn poll_stdin(buf: &mut [u8]) -> Result<PollEvent, io::Error> {
    let mut fds = [libc::pollfd {
        fd: STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    }];

    let ret = unsafe {
        libc::poll(
            fds.as_mut_ptr(),
            fds.len() as libc::nfds_t,
            -1
        )
    };

    if ret < 0 {
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::Interrupted {
            return Ok(PollEvent::Sig);
        }
        return Err(err);
    }

    if fds[0].revents & libc::POLLIN != 0 {
        let n = io::stdin().read(buf)?;
        return Ok(PollEvent::Data(n));
    }

    Ok(PollEvent::Data(0))
}

// point stdin at the controlling terminal, so keys can be read after piped
// input has been used up
pub fn reattach_stdin() -> Result<(), io::Error> {
    let tty = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty")?;
    if unsafe { libc::dup2(std::os::fd::AsRawFd::as_raw_fd(&tty), STDIN_FILENO) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

// plain blocking read, for line editing
pub fn read_stdin(buf: &mut [u8]) -> Result<usize, io::Error> {
    io::stdin().read(buf)
}

extern "C" fn sig_winch(_sig: c_int) {
    GOT_WINCH.store(true, Ordering::SeqCst);
}

extern "C" fn sig_int(_sig: c_int) {
    if !IN_COMMAND.load(Ordering::Relaxed) {
        GOT_INT.store(true, Ordering::SeqCst);
    }
}

extern "C" fn sig_quit(_sig: c_int) {
    if !IN_COMMAND.load(Ordering::Relaxed) {
        GOT_QUIT.store(true, Ordering::SeqCst);
    }
}

pub fn install_sig_handlers() {
    let handlers: [(c_int, extern "C" fn(c_int)); 3] = [
        (libc::SIGWINCH, sig_winch),
        (libc::SIGINT, sig_int),
        (libc::SIGQUIT, sig_quit),
    ];
    for (sig, handler) in handlers {
        unsafe {
            let mut sa: libc::sigaction = mem::zeroed();
            sa.sa_sigaction = handler as *const () as usize;
            libc::sigemptyset(&mut sa.sa_mask);
            sa.sa_flags = 0;
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
}

// Pipe content to the first available clipboard tool; returns false if none worked
pub fn copy_to_clipboard(content: &str) -> bool {
    let tools: [&[&str]; 4] = [
        &["wl-copy"],
        &["xclip", "-selection", "clipboard"],
        &["xsel", "--clipboard", "--input"],
        &["pbcopy"],
    ];
    for tool in tools {
        let Ok(mut child) = Command::new(tool[0])
            .args(&tool[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn() else { continue };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(content.as_bytes());
        }
        if child.wait().is_ok_and(|s| s.success()) {
            return true;
        }
    }
    false
}

// Read content from the first available clipboard tool
pub fn read_from_clipboard() -> Option<String> {
    let tools: [&[&str]; 4] = [
        &["wl-paste", "--no-newline"],
        &["xclip", "-selection", "clipboard", "-o"],
        &["xsel", "--clipboard", "--output"],
        &["pbpaste"],
    ];
    for tool in tools {
        let Ok(output) = Command::new(tool[0])
            .args(&tool[1..])
            .stderr(Stdio::null())
            .output()
        else {
            continue;
        };
        if output.status.success() {
            if let Ok(content) = String::from_utf8(output.stdout) {
                return Some(content);
            }
        }
    }
    None
}
