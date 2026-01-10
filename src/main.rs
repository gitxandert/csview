use std::{
    env, mem, ptr,
    fs::{self, File},
    io::{self, ErrorKind, Read, Write, stdout},
};
use libc::{
    self, c_int, ioctl, winsize, STDOUT_FILENO, TIOCGWINSZ,
    termios, tcgetattr, tcsetattr, cfmakeraw, TCSANOW,
};

fn main() {
    let mut csv: Vec<String> = match load_csv() {
        Ok(file) => file,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };

    install_sig_handlers();
    raw_mode(true);
    set_w_h();

    let mut c_info = CellsInfo::new();
    loop {
        process_input(&mut c_info);
        show_csv(&mut csv, &mut c_info);
    }
}

fn load_csv() -> Result<Vec<String>, io::Error> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        return Err(io::Error::new(ErrorKind::InvalidInput,
                "csview expects one filename argument (e.g. file.csv)"
                ));
    }

    let filename = &args[1];
    let ext: &str = match filename.rsplit_once(|b: char| b == '.') {
        Some((before, after)) if !before.is_empty() && !after.is_empty() => after,
        _ => {
            return Err(io::Error::new(ErrorKind::InvalidFilename,
                    "Filename must have a .csv extension"
                    ));
        }
    };

    if ext != "csv" {
        return Err(io::Error::new(ErrorKind::InvalidFilename,
                "Filename must have a .csv extension"
                ));
    }

    let mut f = File::open(filename)?;
    let mut contents = String::new();
    f.read_to_string(&mut contents)?;

    let lines: Vec<String> = contents
        .split('\n')
        .map(|s| s.to_string())
        .collect();

    Ok(lines)
}

fn process_input(c_info: &mut CellsInfo) {
    if let Some(c) = read_char() {
        match c {
            27 => {
                if let Some(c2) = read_char() {
                    if c2 == b'[' {
                        if let Some(c3) = read_char() {
                            match c3 {
                                b'D' => { // left
                                    c_info.w_offset_left(1);
                                }
                                b'C' => { // right
                                    c_info.w_offset_right(1);
                                }
                                b'A' => { // up
                                    c_info.h_offset_up(1);
                                }
                                b'B' => { // down
                                    c_info.h_offset_down(1);
                                }
                                _ => (),
                            }
                        } else {}
                    }
                } else {}
            }
            _ => (), // ignore
        }
    } else {}
}


fn show_csv(csv: &mut Vec<String>, c_info: &mut CellsInfo) {
    unsafe {
        let cur_w = *w_ptr();
        let cur_h = *h_ptr();

        if c_info.changed(cur_w.clone(), cur_h.clone()) {
            let mut out = stdout();

            write!(out, "\x1b[3J\x1b[H").unwrap();
            
            let h_offset = c_info.h_offset;
            let w_offset = c_info.w_offset;
            let rows = cur_h.saturating_sub(1);
            let cols = cur_w.saturating_sub(1);

            // row indexes the csv row according to the height offset
            // t_row corresponds to the terminal's row
            let mut row = h_offset;
            let mut t_row = 0usize;
            for _ in 0..rows {
                // move cursor to (row+1, col=1)
                write!(out, "\x1b[{};1H\x1b[2K", t_row+1).unwrap();

                let mut line: String = "".to_string();
                if row < csv.len() {
                    line = csv[row]
                        .chars()
                        .skip(w_offset)
                        .take(cols)
                        .collect();
                }

                write!(out, "{line}").unwrap();
                row += 1;
                t_row += 1;
            }

            write!(out, "\x1b[{};1H\x1b[2K", t_row+1).unwrap();
            out.flush().unwrap();
        }
    }
}

static mut ORIG_TERM: Option<termios> = None;
static mut WIDTH: usize = 0;
static mut HEIGHT: usize = 0;

#[inline(always)]
unsafe fn w_ptr() -> *mut usize {
    &raw mut WIDTH
}

#[inline(always)]
unsafe fn h_ptr() -> *mut usize {
    &raw mut HEIGHT
}

struct CellsInfo {
    width: usize,
    height: usize,
    w_offset: usize,
    h_offset: usize,
    was_changed: bool,
}

impl CellsInfo {
    fn new() -> Self {
        Self {
            width: 0usize, 
            height: 0usize,
            w_offset: 0usize,
            h_offset: 0usize,
            was_changed: false,
        }
    }

    fn changed(&mut self, cur_w: usize, cur_h: usize) -> bool {
        if self.was_changed {
            self.restore_unchanged();
            return true;
        }

        if self.width != cur_w || self.height != cur_h {
            self.update_w_h(cur_w, cur_h);
            return true;
        }

        return false;
    }

    fn update_w_h(&mut self, cur_w: usize, cur_h: usize) {
        self.width = cur_w;
        self.height = cur_h;
    }

    fn restore_unchanged(&mut self) {
        self.was_changed = false;
    }

    fn h_offset_up(&mut self, val: usize) {
        self.h_offset = self.h_offset.saturating_sub(val);
        self.was_changed = true;
    }

    fn h_offset_down(&mut self, val: usize) {
        self.h_offset += val;
        self.was_changed = true;
    }

    fn w_offset_left(&mut self, val: usize) {
        self.w_offset = self.w_offset.saturating_sub(val);
        self.was_changed = true;
    }

    fn w_offset_right(&mut self, val: usize) {
        self.w_offset += val;
        self.was_changed = true;
    }
}

fn raw_mode(switch: bool) {
    unsafe {
        let fd = libc::STDIN_FILENO;

        let mut term: termios = std::mem::zeroed();
        tcgetattr(fd, &mut term);

        match switch {
            true => {
                ORIG_TERM = Some(term);
                let mut raw = term;
                cfmakeraw(&mut raw);
                //re-enable SIGINT
                raw.c_lflag |= libc::ISIG;
                // make reads non-blocking
                raw.c_cc[libc::VMIN] = 0;
                raw.c_cc[libc::VTIME] = 1;
                tcsetattr(fd, TCSANOW, &raw);
            }
            false => {
                if let Some(orig) = ORIG_TERM {
                    tcsetattr(fd, TCSANOW, &orig);
                }
            }
        };
    }
}

extern "C" fn sig_winch(_sig: c_int) {
    set_w_h();
}

extern "C" fn sig_int(_sig: c_int) {
    raw_mode(false);
    print!("\r\n");
    std::process::exit(130);
}

fn install_sig_handlers() {
    unsafe {
        let mut sa_winch: libc::sigaction = std::mem::zeroed();
        sa_winch.sa_sigaction = sig_winch as usize;
        sa_winch.sa_flags = 0;

        libc::sigemptyset(&mut sa_winch.sa_mask);

        //register
        libc::sigaction(libc::SIGWINCH, &sa_winch, std::ptr::null_mut());
        
        let mut sa_int: libc::sigaction = std::mem::zeroed();
        sa_int.sa_sigaction = sig_int as usize;
        sa_int.sa_flags = 0;

        libc::sigemptyset(&mut sa_int.sa_mask);

        //register
        libc::sigaction(libc::SIGINT, &sa_int, std::ptr::null_mut());
 
    }
}

#[inline(always)]
fn set_w_h() {
    unsafe {
        let mut ws: winsize = mem::zeroed();
        if ioctl(STDOUT_FILENO, TIOCGWINSZ.into(), &mut ws) == 0 {
            let w = w_ptr();
            let h = h_ptr();
            *w = ws.ws_col as usize;
            *h = ws.ws_row as usize;
        }
    }
}

#[inline(always)]
fn read_char() -> Option<u8> {
    let mut buf = [0u8; 1];
    match std::io::stdin().read(&mut buf) {
        Ok(0) => None,
        Ok(_) => Some(buf[0]),
        Err(_) => None,
    }
}

