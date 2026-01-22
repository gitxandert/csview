use std::{
    mem, 
    io::Write,
    fs::{File, OpenOptions},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering}
};
use libc::{
    self, c_int, ioctl, winsize, STDOUT_FILENO, TIOCGWINSZ,
    termios, tcgetattr, tcsetattr, cfmakeraw, TCSANOW,
};

use crate::cells::{Cell, Cells, Column};

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
    Init,       ////// first draw; draws everything
    Non,        //// no change has occurred
}

pub struct WinInfo {
    pub width: usize,
    pub height: usize,
    pub old_width: usize,
    pub old_height: usize,
    pub w_offset: usize,
    pub h_offset: usize,
    pub w_pointer: usize,
    pub h_pointer: usize,
    pub w_page: usize,
    pub h_page: usize,
    pub changed: WinChange,
    pub mode: ScrollMode,
    num_cols: usize,
    num_rows: usize,
    frame: String,
    focused_content: String,
    writing: bool,
    cursor: (usize, usize),
}

impl WinInfo {
    pub fn new(num_cols: usize, num_rows: usize) -> Self {
        let (width, height) = {
            unsafe {
                let mut w = 0usize;
                let mut h = 0usize;
                let mut ws: winsize = mem::zeroed();
                if ioctl(STDOUT_FILENO, TIOCGWINSZ.into(), &mut ws) == 0 {
                    w = ws.ws_col as usize;
                    h = ws.ws_row as usize;
                }
                (w, h)
            }
        };

        Self {
            width: width,
            height: height,
            old_width: 0usize,
            old_height: 0usize,
            w_offset: 0usize,
            h_offset: 0usize,
            num_cols: num_cols,
            num_rows: num_rows,
            focused_content: String::new(),
            frame: String::new(),
            changed: WinChange::Init,
            mode: ScrollMode::Cell,
            w_pointer: 0usize,
            h_pointer: 0usize,
            w_page: 0usize,
            h_page: 0usize,
            writing: false,
            cursor: (0usize, 0usize),
        }
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

    pub fn set_w_h(&mut self) {
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
    
    pub fn set_w_pointer(&mut self, w: usize) {
        if w >= 0 && w < self.num_cols {
            self.w_pointer = w;
            self.changed = WinChange::Focus;

            // change w_offset if w_pointer has gone out of view
            if self.w_pointer < self.w_offset {
                self.w_offset = self.w_offset.saturating_sub(self.w_page);
                self.changed = WinChange::Columns;
            } else if self.w_pointer >= self.w_offset + self.w_page {
                let diff = self.w_pointer.saturating_sub(self.w_offset + self.w_page);
                self.w_offset = self.w_pointer.saturating_sub(diff).min(
                    self.num_cols.saturating_sub(self.w_page)
                );
                self.changed = WinChange::Columns;
            }
        } else if w >= self.num_cols {
            if self.w_pointer != self.num_cols.saturating_sub(1) {
                self.w_pointer = self.num_cols.saturating_sub(1);
                self.w_offset = self.num_cols.saturating_sub(self.w_page);
                self.changed = WinChange::Focus;
            }
        }
    }

    pub fn set_w_offset(&mut self, w: usize) {
        if w >= 0 && w < self.num_cols {
            let old_w = self.w_offset;
            self.w_offset = w;

            // keep w_pointer at same relative spot it was before
            if old_w > self.w_offset {
                let diff = old_w.saturating_sub(self.w_offset);
                self.w_pointer = self.w_pointer.saturating_sub(diff);
                self.changed = WinChange::Columns;
            } else if old_w < self.w_offset {
                let diff = self.w_offset.saturating_sub(old_w);
                self.w_pointer += diff;
                self.changed = WinChange::Columns;
            }
        }
    }
    
    pub fn set_h_pointer(&mut self, h: usize) {
        if h >= 0 && h < self.num_rows {
            let old_h = self.h_pointer;
            self.h_pointer = h;
            self.changed = WinChange::Focus;
            // change h_offset if h_pointer has gone out of view
            if self.h_pointer < self.h_offset {
                self.h_offset = self.h_offset.saturating_sub(self.h_page);
                self.changed = WinChange::Rows;
            } else if self.h_pointer >= self.h_offset + self.h_page {
                let diff = self.h_pointer.saturating_sub(self.h_offset + self.h_page);
                self.h_offset = self.h_pointer.saturating_sub(diff).min(
                    self.num_rows.saturating_sub(self.h_page + 1)
                );
                self.changed = WinChange::Rows;
            }
        } else {
            if self.h_pointer != self.num_rows.saturating_sub(1) {
                self.h_pointer = self.num_rows.saturating_sub(1);
                self.h_offset = self.num_rows.saturating_sub(self.h_page + 1);
                self.changed = WinChange::Focus;
            }
        }
    }

    pub fn set_h_offset(&mut self, h: usize) {
        if h >= 0 && h <= self.num_rows.saturating_sub(self.h_page) {
            let old_h = self.h_offset;
            self.h_offset = h;

            // keep h_pointer at same relative spot it was before
            if old_h > self.h_offset {
                let diff = old_h.saturating_sub(self.h_offset);
                self.h_pointer = self.h_pointer.saturating_sub(diff);
                self.changed = WinChange::Rows;
            } else if self.h_offset > old_h {
                let diff = self.h_offset.saturating_sub(old_h);
                self.h_pointer += diff;
                self.changed = WinChange::Rows;
            }
        }
    }

    pub fn set_focused(&mut self, focused: &str) {
        self.focused_content.clear();
        let take = focused.len().min(self.width);
        self.focused_content.push_str(&focused[..take]);
    }

    pub fn draw_focused_content(&mut self) {
        let focused = &self.focused_content;
        let row = self.height;
        let content = format!("\x1b[{row};1H\x1b[2K\x1b[0m{focused}");
        self.push_to_frame(&content);
    }

    pub fn draw_screen(&mut self, cells: &mut Cells) {
        // reset focused cell
        cells.set_w_cell(self.w_pointer, self.h_pointer);

        let mut id = self.w_offset;
        let mut w = 0usize;
        let mut h = 0usize;
        for i in 1..=self.height {
            let beg = format!("\x1b[{i};1H\x1b[2K");
            self.push_to_frame(&beg);

            if i == 1 {
                self.push_to_frame("\x1b[4m    |");
                let mut start = 6usize;

                let col_ids = &cells.col_idx;
                let mut col_id = &col_ids[id];
                let mut col_width = col_id.width + 3; // + 3 for formatting
                while start + col_width < self.width {
                    let content = &col_id.content;
                    let ws = (col_id.width / 2).saturating_sub(1);
                    let with_ws = format!(
                        "{:<ws$}{}{:<ws$}", 
                        " ", content, " ", ws = ws
                    );
                    let positioned = format!(
                        " {:<width$} |", with_ws, width = col_id.width
                    );
                    self.push_to_frame(&positioned);
                    start += col_width;
                    id += 1;
                    if id < self.num_cols {
                        col_id = &col_ids[id];
                        col_width = col_id.width + 3;
                    } else {
                        break;
                    }
                }
            } else if i == 2 {
                self.push_to_frame("\x1b[1;30;47mHEAD|\x1b[22;39;49m");
                let mut start = 6usize;
                id = self.w_offset;

                let header = &cells.header;
                let mut col_name = &header[id];
                let mut col_width = col_name.width + 3; // + 3 for formatting
                while start + col_width < self.width {
                    let take = col_name.text_offset + col_name.width.min(col_name.len());
                    let content = &col_name.content[col_name.text_offset..take];
                    let positioned = format!(
                        "\x1b[30;47m {:<width$} |\x1b[39;49m", 
                        content, width = col_name.width
                    );
                    self.push_to_frame(&positioned);

                    start += col_width;
                    id += 1;
                    if id < self.num_cols {
                        col_name = &header[id];
                        col_width = col_name.width + 3;
                    } else {
                        break;
                    }
                }
            } else {
                let row_id = (i - 3) + self.h_offset;
                if row_id >= self.num_rows {
                    break;
                }

                let row_idx = &cells.row_idx.get_cell(row_id).content;
                let row_num = format!(
                    "\x1b[30;47m{row_idx} \x1b[39;49m"
                );
                self.push_to_frame(&row_num);

                let mut start = 6usize;
                id = self.w_offset;
                let columns = &mut cells.columns;
                let mut col = &mut columns[id];
                let mut col_width = col.col_width();

                while start + col_width < self.width {
                    let mut cell = col.get_cell(row_id);
                    let take = cell.text_offset + cell.width.min(cell.len());
                    let content = &cell.content;
                    let visible = &content[cell.text_offset..take];
                    let positioned = {
                        if cell.is_focused {
                            w = id;
                            h = row_id;
                            self.set_focused(content);
                            format!(
                                "\x1b[7;36;47m {:<width$} \x1b[27;39;49m|", 
                                visible, width = col_width - 3
                            )
                        } else {
                            format!(
                                " {:<width$} |",
                                visible, width = col_width - 3
                            )
                        }
                    };
                    self.push_to_frame(&positioned);

                    col.set_start(start);
                    start += col_width;
                    id += 1;
                    if id < self.num_cols {
                        col = &mut columns[id];
                        col_width = col.col_width();
                    } else {
                        break;
                    }
                }
            }
        }

        self.set_w_page(
            id, self.w_offset
        );
        self.set_h_page(
            self.height.saturating_sub(4) + self.h_offset,
            self.h_offset
        );
    }

    fn print_row(&mut self, cells: &mut Cells, mut i: usize, row: usize) {
        let mut col = &mut cells.columns[i];
        let mut start = col.start;
        let mut width = col.col_width();
        while start + width < self.width {
            let mut cell = col.get_cell(row);
            let take = cell.text_offset + cell.width.min(cell.len());
            let content = &cell.content;
            let visible = &content[cell.text_offset..take];
            let formatted = {
                if cell.is_focused {
                    self.set_focused(&content);
                    format!("\x1b[7;36;47m {:<width$} \x1b[27;39;49m|",
                        visible, width = width - 3
                    )
                } else {
                    format!(" {:<width$} |", visible, width = width - 3)
                }
            };
            self.push_to_frame(&formatted);

            start += width;
            i += 1;
            if i < self.num_cols {
                col = &mut cells.columns[i];
                width = col.col_width();
            } else {
                break;
            }
        }
    }

    pub fn draw_focus(&mut self, cells: &mut Cells) {
        // erase the previously-focused cell
        // and whatever follows it
        let (wc_c, wc_l) = cells.w_cell;
        cells.set_w_cell(self.w_pointer, self.h_pointer);

        let prev_start = cells.columns[wc_c].start;
        let cur_start = cells.columns[self.w_pointer].start;
        let mut start = prev_start.min(cur_start);
        let prev_l = 3 + wc_l.saturating_sub(self.h_offset); 
        let beg = format!("\x1b[{};{}H\x1b[K\x1b[4m", prev_l, start);
        self.push_to_frame(&beg);
        

        // redraw previous row and the new row
        // if the new row is different from the previous
        let i = wc_c.min(self.w_pointer);
        self.print_row(cells, i, wc_l); 
        
        if wc_l != self.h_pointer {
            let cur_l = 3 + self.h_pointer.saturating_sub(self.h_offset); 
            let beg = format!(
                "\x1b[{};{}H\x1b[K\x1b[4m", cur_l, start
            );
            self.push_to_frame(&beg);

            self.print_row(cells, self.w_pointer, self.h_pointer);
        }
    }

    pub fn push_to_frame(&mut self, content: &str) {
        self.frame.push_str(content);
    }

    pub fn flush(&mut self) {
        self.draw_focused_content();

        let mut out = std::io::stdout();
        if self.writing {
            // show cursor
            let (l, c) = self.cursor_pos();
            let cursor = format!("\x1b[{l};{c}H\x1b[?25h");
            self.push_to_frame(&cursor);
        }
        write!(out, "\x1b[?25l{}\x1b", self.frame);
        out.flush().unwrap();

        self.frame = String::new();
        self.changed = WinChange::Non;
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
        if let Ok(mut log) = OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/csview.log") 
        {
            log.write_all(panic_info.as_bytes());
        } else { eprintln!("couldn't open /tmp/csview.log"); }
        std::process::exit(130);
    }));
}

