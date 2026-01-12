use std::mem;
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use libc::{
    self, c_int, ioctl, winsize, STDOUT_FILENO, TIOCGWINSZ,
    termios, tcgetattr, tcsetattr, cfmakeraw, TCSANOW,
};

use crate::csv::Cells;

pub enum ScrollMode {
    Text,
    Cell,
    Axis,
    Page,
}

pub struct WinInfo {
    width: usize,
    height: usize,
    pub w_offset: usize,
    pub h_offset: usize,
    max_w: usize,
    max_h: usize,
    was_changed: bool,
    mode: ScrollMode,
    pub w_pointer: usize,
    pub h_pointer: usize,
    x_page: usize,
    y_page: usize,
}

impl WinInfo {
    pub fn new(row_len: usize, col_len: usize) -> Self {
        Self {
            width: 0usize,
            height: 0usize,
            w_offset: 0usize,
            h_offset: 0usize,
            max_w: row_len - 1,
            max_h: col_len - 1,
            was_changed: true,
            mode: ScrollMode::Cell,
            w_pointer: 0usize,
            h_pointer: 1usize,
            x_page: 0usize,
            y_page: 0usize,
        }
    }

    pub fn changed(&mut self) -> bool {
        if self.was_changed {
            self.was_changed = false;
            return true;
        }

        if self.width == 0usize && self.height == 0usize {
            unsafe {
                let w = *w_ptr();
                let h = *h_ptr();
                self.set_w_h(w, h);
                return true;
            }
        }

        return false;
    }

    pub fn set_x_page(&mut self, end: usize, beg: usize) {
        self.x_page = end - beg;
    }

    pub fn set_y_page(&mut self, end: usize, beg: usize) {
        self.y_page = end - beg;
    }

    // return index of highlighted row;
    // since the h_pointer is indexing rows, it returns itself
    pub fn h_pointer_cell(&self) -> usize {
        self.h_pointer
    }

    // return approximate location of the cell on screen;
    // this means adding up the widths of the cells up to the pointer,
    // plus the length of the row_number at the start of the row,
    // plus 3 (two spaces and a |)
    pub fn w_pointer_cell(&self, row_num_len: usize, widths: &Vec<usize>) -> usize {
        let mut cursor_col = 0usize;
        let mut idx = self.w_offset;
        while idx != self.w_pointer {
            cursor_col += widths[idx];
            idx += 1;
        }
        row_num_len + cursor_col + 3 
    }

    pub fn set_mode(&mut self, mode: ScrollMode) {
        self.mode = mode;
    }

    pub fn set_w_h(&mut self, cur_w: usize, cur_h: usize) {
        if self.width != cur_w {
            self.width = cur_w;
            self.was_changed = true;
        }
        if self.height == cur_h {
            self.height = cur_h;
            self.was_changed = true;
        }
    }

    pub fn h_offset_up(&mut self) {
        match self.mode {
            ScrollMode::Text => (),
            ScrollMode::Cell => self.dec_h_pointer(),
            ScrollMode::Axis => self.unshift_page_h(),
            ScrollMode::Page => self.dec_page_h(),
        }
    }

    pub fn h_offset_down(&mut self) {
        match self.mode {
            ScrollMode::Text => (),
            ScrollMode::Cell => self.inc_h_pointer(),
            ScrollMode::Axis => self.shift_page_h(),
            ScrollMode::Page => self.inc_page_h(),
        }
    }

    pub fn w_offset_left(&mut self, cells: &mut Cells) {
        match self.mode {
            ScrollMode::Text => {
                cells.set_text_offset(
                    -1, 
                    self.h_pointer,
                    self.w_pointer
                );
            }
            ScrollMode::Cell => self.dec_w_pointer(),
            ScrollMode::Axis => self.unshift_page_w(),
            ScrollMode::Page => self.dec_page_w(),
        }
    }

    pub fn w_offset_right(&mut self, cells: &mut Cells) {
        match self.mode {
            ScrollMode::Text => {
                cells.set_text_offset(
                    1,
                    self.h_pointer,
                    self.w_pointer
                );
            }
            ScrollMode::Cell => self.inc_w_pointer(),
            ScrollMode::Axis => self.shift_page_w(),
            ScrollMode::Page => self.inc_page_w(),
        }
    }

    // private functions
    //
    fn dec_h_pointer(&mut self) {
        if self.h_pointer > 0 {
            self.h_pointer = self.h_pointer.saturating_sub(1);
            if self.h_pointer < self.h_offset {
                self.h_offset = self.h_offset.saturating_sub(1);
            }
            self.was_changed = true;
        }
    }

    fn inc_h_pointer(&mut self) {
        if self.h_pointer < self.max_h {
            self.h_pointer += 1;
            
            if self.h_pointer >= self.h_offset + self.y_page {
                self.h_offset += 1;
            }
            self.was_changed = true;
        }
    }

    fn dec_page_h(&mut self) {
        if self.h_offset > 0 {
            if self.h_offset.saturating_sub(self.y_page) > 0 {
                self.h_offset = self.h_offset.saturating_sub(self.y_page);
            } else {
                self.h_offset = 0;
            }
            if self.h_pointer.saturating_sub(self.y_page) > 0 {
                self.h_pointer = self.h_pointer.saturating_sub(self.y_page);
            } else {
                self.h_pointer = 0;
            }
            self.was_changed = true;
        }
    }

    fn inc_page_h(&mut self) {
        if self.h_offset + self.y_page < self.max_h {
            if self.h_offset + self.y_page < self.max_h.saturating_sub(self.y_page) {
                self.h_offset += self.y_page;
                self.h_pointer += self.y_page;
            } else {
                let inc = self.max_h - self.h_offset - self.y_page + 1;
                self.h_offset += inc;
                self.h_pointer += inc;
            }
            self.was_changed = true;
        }
    }

    fn unshift_page_h(&mut self) {
        if self.h_offset > 1 {
            self.h_offset = self.h_offset.saturating_sub(1);
            if self.h_pointer > 1 {
                self.h_pointer = self.h_pointer.saturating_sub(1);
            }
            self.was_changed = true;
        }
    }

    fn shift_page_h(&mut self) {
        // always stay within page
        if self.h_offset <= self.max_h.saturating_sub(self.y_page) {
            self.h_offset += 1;
            if self.h_pointer < self.max_h {
                self.h_pointer += 1;
            }
            self.was_changed = true;
        }
    }

    fn dec_w_pointer(&mut self) {
        if self.w_pointer > 0 {
            self.w_pointer = self.w_pointer.saturating_sub(1);
            if self.w_pointer < self.w_offset {
                self.w_offset = self.w_offset.saturating_sub(1);
            }
            self.was_changed = true;
        }
    }

    fn inc_w_pointer(&mut self) {
        if self.w_pointer < self.max_w {
            self.w_pointer += 1;

            if self.w_pointer >= self.w_offset + self.x_page {
                self.w_offset += 1;
            }
            self.was_changed = true;
        }    
    }

    fn dec_page_w(&mut self) {
        if self.w_offset > 0 {
            if self.w_offset.saturating_sub(self.x_page) > 0 {
                self.w_offset = self.w_offset.saturating_sub(self.x_page);
            } else {
                self.w_offset = 0;
            }
            if self.w_pointer.saturating_sub(self.x_page) > 0 {
                self.w_pointer = self.w_pointer.saturating_sub(self.x_page);
            } else {
                self.w_pointer = 0;
            }
            self.was_changed = true;
        }
    }


    fn inc_page_w(&mut self) {
        if self.w_offset + self.x_page < self.max_w {
            if self.w_offset + self.x_page < self.max_w.saturating_sub(self.x_page) {
                self.w_offset += self.x_page;
                self.w_pointer += self.x_page;
            } else {
                let inc = self.max_w - self.w_offset - self.x_page + 1;
                self.w_offset += inc;
                self.w_pointer += inc;
            }
            self.was_changed = true;
        }
    }

    fn unshift_page_w(&mut self) {
        if self.w_offset > 0 {
            self.w_offset = self.w_offset.saturating_sub(1);
            if self.w_pointer > 0 {
                self.w_pointer = self.w_pointer.saturating_sub(1);
            }
            self.was_changed = true;
        }
    }

    fn shift_page_w(&mut self) {
        if self.w_offset < self.max_w.saturating_sub(self.x_page) {
            self.w_offset += 1;
            if self.w_pointer < self.max_w {
                self.w_pointer += 1;
            }
            self.was_changed = true;
        }
    }
}

// Terminal takeover + signal handling + globals for WIDTH and HEIGHT

static mut ORIG_TERM: Option<termios> = None;
static mut WIDTH: usize = 0;
static mut HEIGHT: usize = 0;

#[inline(always)]
pub unsafe fn w_ptr() -> *mut usize {
    &raw mut WIDTH
}

#[inline(always)]
pub unsafe fn h_ptr() -> *mut usize {
    &raw mut HEIGHT
}

#[inline(always)]
pub fn set_w_h() {
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

pub fn raw_mode(switch: bool) {
    unsafe {
        let fd = libc::STDIN_FILENO;

        let mut term: termios = std::mem::zeroed();
        tcgetattr(fd, &mut term);

        let mut out = std::io::stdout();

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
    
                // switch to Alternate Screen Buffer
                write!(out, "\x1b[?1049h").unwrap();
            }
            false => {
                if let Some(orig) = ORIG_TERM {
                    tcsetattr(fd, TCSANOW, &orig);
                    // switch back to main buffer
                    write!(out, "\x1b[?1049l").unwrap();
                }
            }
        };
        out.flush().unwrap();
    }
}

static GOT_WINCH: AtomicBool = AtomicBool::new(false);
static GOT_INT: AtomicBool = AtomicBool::new(false);

extern "C" fn sig_winch(_sig: c_int) {
    GOT_WINCH.store(true, Ordering::SeqCst);
}

extern "C" fn sig_int(_sig: c_int) {
    GOT_INT.store(true, Ordering::SeqCst);
}

pub fn check_flags(w_info: &mut WinInfo) {
    if GOT_WINCH.swap(false, Ordering::SeqCst) {
        set_w_h();

        unsafe {
            let w = *w_ptr();
            let h = *h_ptr();
            w_info.set_w_h(w, h);
        }
    }

    if GOT_INT.swap(false, Ordering::SeqCst) {
        raw_mode(false);
        std::process::exit(130);
    }
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

