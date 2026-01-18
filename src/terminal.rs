use std::mem;
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use libc::{
    self, c_int, ioctl, winsize, STDOUT_FILENO, TIOCGWINSZ,
    termios, tcgetattr, tcsetattr, cfmakeraw, TCSANOW,
};

use crate::csv::{Cell, Cells};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScrollMode {
    Text, // scroll through text within a cell
    Cell, //// change focus from cell to cell
    Axis, ////// shift all rows/columns
    Page, //////// replace all rows/columns with
          //////// the next screenful of rows/columns
}

#[derive(Debug, PartialEq)]
pub enum WinChange {
    Cell,       // one cell's content has changed
    Focus,      //// the focus has changed
    ColWidth,   ////// a single column's width has changed
    Rows,       //// the view of rows has shifted
    Columns,    // the view of columns has shifted
    Screen,     //// the screen's dimensions have changed
    Non,        ////// no change has occurred
}

pub struct WinInfo {
    width: usize,
    height: usize,
    old_width: usize,
    old_height: usize,
    pub w_offset: usize,
    pub h_offset: usize,
    num_cols: usize,
    num_rows: usize,
    w_page: usize,
    h_page: usize,
    changed: WinChange,
    mode: ScrollMode,
    pub w_pointer: usize,
    pub h_pointer: usize,
    frame: String,
    focused_content: String,
    writing: bool,
    cursor: (usize, usize),
}

impl WinInfo {
    pub fn new(num_cols: usize, num_rows: usize) -> Self {
        Self {
            width: 0usize,
            height: 0usize,
            old_width: 0usize,
            old_height: 0usize,
            w_offset: 0usize,
            h_offset: 0usize,
            num_cols: num_cols,
            num_rows: num_rows,
            changed: WinChange::Screen,
            mode: ScrollMode::Cell,
            w_pointer: 0usize,
            h_pointer: 0usize,
            w_page: 0usize,
            h_page: 0usize,
            writing: false,
            cursor: (0usize, 0usize),
        }
    }

    pub fn changed(&mut self) -> WinChange {
        self.changed
    }

    // set w_page and h_page whenever screen is redrawn
    pub fn set_w_page(&mut self, end: usize, beg: usize) {
        self.w_page = end.saturating_sub(beg);
    }
    pub fn set_h_page(&mut self, end: usize, beg: usize) {
        self.h_page = end.saturating_sub(beg);
    }

    pub fn set_mode(&mut self, mode: ScrollMode) {
        self.mode = mode;
    }

    pub fn writing(&self) -> bool {
        self.writing
    }

    pub fn set_writing(&mut self, w: bool) {
        self.writing = w;
    }

    pub fn mode(&self) -> ScrollMode {
        self.mode
    }

    fn cursor_pos(&self) -> (usize, usize) {
        (self.cursor.0, self.cursor.1)
    }

    pub fn set_w_h() {
        unsafe {
            let mut ws: winsize = mem::zeroed();
            if ioctl(STDOUT_FILENO, TIOCGWINSZ.into(), &mut ws) == 0 {
                self.old_width = self.width;
                self.old_height = self.height;
                self.width = ws.ws_col as usize;
                self.height = ws.ws_row as usize;
            }
            self.changed = WinChange::Screen;
        }
    }
    
    pub fn set_w_pointer(&mut self, w) {
        if w > 0 && w < self.num_cols {
            self.w_pointer = w;
            self.changed = WinChange::Focus;

            // change w_offset if w_pointer has gone out of view
            if self.w_pointer < self.w_offset {
                let diff = self.w_offset.saturating_sub(self.w_pointer);
                self.w_offset= self.w_offset.saturating_sub(diff);
                self.changed = WinChange::Columns;
            } else if self.w_pointer > self.w_offset + self.w_page {
                let diff = self.w_pointer.saturating_sub(self.w_offset) + self.w_page;
                self.w_offset = self.w_offset + diff;
                self.changed = WinChange::Columns;
            }
        }
    }

    pub fn set_w_offset(&mut self, w) {
        if w > 0 && w < self.num_cols {
            let old_w = self.w_offset;
            self.w_offset = w;

            // keep w_pointer at same relative spot it was before
            if old_w > self.w_offset {
                let diff = old_w.saturating_sub(self.w_offset);
                self.w_pointer = self.w_pointer.saturating_sub(diff);
            } else {
                let diff = self.w_offset.saturating_sub(old_w);
                self.w_pointer += diff;
            }
            
            self.changed = WinChange::Columns;
        }
    }
    
    pub fn set_h_pointer(&mut self, h) {
        if h > 0 && h < self.num_rows {
            self.h_pointer = h;
            self.changed = WinChange::Focus;

            // change h_offset if h_pointer has gone out of view
            if self.h_pointer < self.h_offset {
                let diff = self.h_offset.saturating_sub(self.h_pointer);
                self.h_offset= self.h_offset.saturating_sub(diff);
                self.changed = WinChange::Rows;
            } else if self.h_pointer > self.h_offset + self.h_page {
                let diff = self.h_pointer.saturating_sub(self.h_offset) + self.h_page;
                self.h_offset = self.h_offset + diff;
                self.changed = WinChange::Rows;
            }
        }
    }

    pub fn set_h_offset(&mut self, h) {
        if h > 0 && h < self.num_rows {
            let old_h = self.h_offset;
            self.h_offset = h;

            // keep h_pointer at same relative spot it was before
            if old_h > self.h_offset {
                let diff = old_h.saturating_sub(self.h_offset);
                self.h_pointer = self.h_pointer.saturating_sub(diff);
            } else {
                let diff = self.h_offset.saturating_sub(old_h);
                self.h_pointer += diff;
            }
            
            self.changed = WinChange::Rows;
        }
    }

    pub fn draw_focused_content(&mut self) {
        let focused = &self.focused_content;
        let row = self.height;
        let col = self.width;
        let content = "\x1b[" + row + ":" + col "H\x1b[K" + focused;
        self.push_to_frame(content);
    }

    pub fn draw_column(&mut self, id: &str, name: &str, col: Column) {
        let start = col.start;
        
        for i in self.h_offset..self.height {
            let cell = col.get_cell(i);
            let content = cell.format();
            self.push_to_frame(content);
        }
    }

    pub fn push_to_frame(&mut self, content: &str) {
        self.frame.push_str(content);
    }

    pub fn flush(&mut self) {
        let mut out = std::io::stdout();
        if writing {
            // show cursor
            let (l, c) = self.cursor_pos();
            let cursor = "\x1b[" + l ":" + c + "H\x1b[?25h";
            self.push_to_frame(cursor);
        }
        write!(out, "\x1b[?25l{}", frame);
        self.changed = WinChange::Non;
        self.frame = String::new();
    }
}

// Terminal takeover + signal handling + globals for WIDTH and HEIGHT

static mut ORIG_TERM: Option<termios> = None;

pub fn raw_mode(switch: bool) {
    unsafe {
        let fd = libc::STDIN_FILENO;

        let mut term: termios = std::mem::zeroed();
        tcgetattr(fd, &mut term);

        let mut out = std::io::stdout();
        let mut outstring = String::new();

        match switch {
            true => {
                ORIG_TERM = Some(term);
                let mut raw = term;
                cfmakeraw(&mut raw);
                //re-enable SIGINT
                raw.c_lflag |= libc::ISIG;
                // turn on non-blocking to catch sigs
                raw.c_cc[libc::VMIN] = 0;
                raw.c_cc[libc::VTIME] = 0;
                tcsetattr(fd, TCSANOW, &raw);
    
                // switch to Alternate Screen Buffer
                outstring.push_str("\x1b[?1049h");
                // hide cursor
                outstring.push_str("\x1b[?25l");
            }
            false => {
                if let Some(orig) = ORIG_TERM {
                    tcsetattr(fd, TCSANOW, &orig);
                    // switch back to main buffer
                    outstring.push_str("\x1b[?1049l");
                    // show cursor
                    outstring.push_str("\x1b[?25h");
                }
            }
        };
        write!(out, "{}", outstring).unwrap();
        out.flush().unwrap();
    }
}

static GOT_WINCH: AtomicBool = AtomicBool::new(false);
static GOT_INT: AtomicBool = AtomicBool::new(false);
static GOT_QUIT: AtomicBool = AtomicBool::new(false);

extern "C" fn sig_winch(_sig: c_int) {
    GOT_WINCH.store(true, Ordering::SeqCst);
}

extern "C" fn sig_int(_sig: c_int) {
    GOT_INT.store(true, Ordering::SeqCst);
}

extern "C" fn sig_quit(_sig: c_int) {
    GOT_QUIT.store(true, Ordering::SeqCst);
}

pub fn check_flags(w_info: &mut WinInfo) -> bool {
    if GOT_WINCH.swap(false, Ordering::SeqCst) {
        w_info.set_w_h();
    }

    return GOT_INT.swap(false, Ordering::SeqCst) || 
           GOT_QUIT.swap(false, Ordering::SeqCst);
}

pub fn install_sig_handlers() {
    unsafe {
        //SIGWINCH
        let mut sa_winch: libc::sigaction = std::mem::zeroed();
        sa_winch.sa_sigaction = sig_winch as usize;
        libc::sigemptyset(&mut sa_winch.sa_mask);
        sa_winch.sa_flags = 0;
        libc::sigaction(libc::SIGWINCH, &sa_winch, std::ptr::null_mut());

        //SIGINT
        let mut sa_int: libc::sigaction = std::mem::zeroed();
        sa_int.sa_sigaction = sig_int as usize;
        libc::sigemptyset(&mut sa_int.sa_mask);
        sa_int.sa_flags = 0;
        libc::sigaction(libc::SIGINT, &sa_int, std::ptr::null_mut());
        
        //SIGQUIT
        let mut sa_quit: libc::sigaction = std::mem::zeroed();
        sa_quit.sa_sigaction = sig_quit as usize;
        libc::sigemptyset(&mut sa_quit.sa_mask);
        sa_quit.sa_flags = 0;
        libc::sigaction(libc::SIGQUIT, &sa_quit, std::ptr::null_mut());
    }
}

pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        raw_mode(false);
        let panic_info = format!("Panic: {info}");
        if let Ok(mut log) = File::create("/tmp/csview.log") {
            log.write_all(panic_info.as_bytes());
        } else {}
        std::process::exit(130);
    }));
}

