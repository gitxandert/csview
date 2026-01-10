use std::{
    env, mem, ptr,
    fs::{self, File},
    io::{self, ErrorKind, Read, Write, stdout},
};
use libc::{
    self, c_int, ioctl, winsize, STDOUT_FILENO, TIOCGWINSZ,
    termios, tcgetattr, tcsetattr, cfmakeraw, TCSANOW,
    input_event
};

mod keycodes;
use crate::keycodes::*;

fn main() {
    let mut cells: Cells = match load_csv() {
        Ok(file) => file,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };

    install_sig_handlers();
    raw_mode(true);
    set_w_h();

    let mut kbd = match File::open("/dev/input/event3") {
        Ok(dev) => dev,
        Err(e) => {
            raw_mode(false);
            eprintln!("{e}");
            return;
        }
    };

    let (max_w, max_h) = cells.xy();
    let mut w_info = WinInfo::new(max_w, max_h);
    loop {
        show_csv(&mut cells, &mut w_info);
        process_input(&mut w_info, &mut kbd);
    }
}

fn load_csv() -> Result<Cells, io::Error> {
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

    let x = lines[0].split(',').count();
    let y = lines.len();
    let mut cells = Cells::new(x, y);
    for line in lines {
        let row: Vec<String> = line
            .split(',')
            .map(|s| s.to_string())
            .collect();
        cells.push_row(row);
    }

    Ok(cells)
}

fn process_input(w_info: &mut WinInfo, kbd: &mut File) {
    let ev = match read_char(kbd) {
        Some(event) => event,
        None => return,
    };
    if ev.type_ == EV_KEY {
        if ev.value == KEY_DEPRESSED {
            match ev.code {
                KEY_LEFTCTRL | KEY_RIGHTCTRL => w_info.page(),
                KEY_LEFTSHIFT | KEY_RIGHTSHFIT => w_info.shift(),
                KEY_LEFT => w_info.w_offset_left(),
                KEY_RIGHT => w_info.w_offset_right(),
                KEY_UP => w_info.h_offset_up(),
                KEY_DOWN => w_info.h_offset_down(),
                _ => (),
            }
        } else if ev.value == KEY_REPEAT {
            match ev.code {
                KEY_LEFT => w_info.w_offset_left(),
                KEY_RIGHT => w_info.w_offset_right(),
                KEY_UP => w_info.h_offset_up(),
                KEY_DOWN => w_info.h_offset_down(),
                _ => (),
            }
        } else if ev.value == KEY_RELEASED {
            match ev.code {
                KEY_LEFTCTRL | KEY_RIGHTCTRL => w_info.unpage(),
                KEY_LEFTSHIFT | KEY_RIGHTSHFIT => w_info.unshift(),
                _ => (),
            }
        }
    }
}

fn show_csv(cells: &mut Cells, w_info: &mut WinInfo) {
    unsafe {
        let cur_w = *w_ptr();
        let cur_h = *h_ptr();

        if w_info.changed(cur_w.clone(), cur_h.clone()) {
            let mut out = stdout();

            write!(out, "\x1b[2J\x1b[H").unwrap();
            // clear scrollback
            write!(out, "\x1b[3J").unwrap();
            
            let h_offset = w_info.h_offset;
            let w_offset = w_info.w_offset;
            let rows = cur_h.saturating_sub(1);
            let cols = cur_w.saturating_sub(1);

            // row indexes the csv row according to the height offset
            // t_row corresponds to the terminal's row
            let mut row = h_offset;
            let mut t_row = 0usize;

            let orig_idx = w_offset;
            let mut idx = orig_idx;

            let widths = cells.widths.clone();
            for _ in 0..rows {
                /*
                * Each line needs to be formatted like so:
                * | content(...) | content(...) | content(...) |
                * so the total width of the screen needs to be portioned:
                * 5 + n x (1 + 1 + cell_width + 1)
                * if the cell at the end goes over, it is dropped
                */
                // move cursor to (row+1, col=1)
                write!(out, "\x1b[{};1H\x1b[2K", t_row+1).unwrap();

                let mut line: String = "".to_string();
                // vec of cols
                let mut v_cols = &Vec::new();
                // always print col names
                if t_row == 0 {
                    line = "    ".to_string();
                    v_cols = cells.get_row(0);
                    
                } else {
                    // if reached cells.len(),
                    // print XXXX instead of row number
                    if row < cells.len() {
                        // print in hexadecimal (space-saving)
                        line = format!("{:04X}", row);
                        v_cols = cells.get_row(row);
                    } else {
                        line = "XXXX| EOF".to_string();
                    }
                }
                let row_len = v_cols.len();

                if row_len > 0 && idx < row_len {
                    idx = orig_idx;
                    loop {
                        if idx < row_len - 1 {
                            let width = widths[idx];
                            if line.len() + width > cur_w {
                                break;
                            }
                            let col = &v_cols[idx];
                            let mut contents: String = "".to_string();
                            let mut cell: String = "".to_string();
                            if col.len() > width {
                                contents = col
                                    .chars()
                                    .take(width - 3)
                                    .collect();
                                cell = format!("| {:<width$}... ", 
                                    contents, width = width - 3);
                            } else {
                                contents = col
                                    .chars()
                                    .take(width)
                                    .collect();
                                cell = format!("| {:<width$} ",
                                    contents, width = width);
                            }

                            if w_info.w_pointer == idx &&
                               w_info.h_pointer == row {
                                let mut sub = "\x1b[7m".to_string() + &cell + "\x1b[0;4m";
                                cell = sub;
                            }
                           
                            line += &cell;
                            idx += 1;
                        } else {
                            break;
                        }
                    }

                    write!(out, "\x1b[4m{line}\x1b[0m").unwrap();
                    row += 1;
                    t_row += 1;
                }
            }
            w_info.set_x_page(idx - orig_idx);
            w_info.set_y_page(row - h_offset);
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

struct Cells {
    rows: Vec<Vec<String>>,
    x: usize,
    y: usize,
    widths: Vec<usize>,
    heights: Vec<usize>,
}

impl Cells {
    fn new(x: usize, y: usize) -> Self {
        let rows = Vec::<Vec<String>>::new();
        let widths = vec![12usize; x];
        let heights = vec![1usize; y];

        Self { rows, x, y, widths, heights }
    }

    fn xy(&self) -> (usize, usize) {
        (self.x, self.y)
    }

    fn push_row(&mut self, row: Vec<String>) {
        self.rows.push(row);
    }

    fn get_row(&mut self, idx: usize) -> &Vec<String> {
        if idx > self.y {
            return &self.rows[self.y];
        } else {
            return &self.rows[idx];
        }
    }

    fn len(&self) -> usize {
        self.y
    }
}

enum ScrollMode {
    Cell,
    Col,
    Page,
}

struct WinInfo {
    width: usize,
    height: usize,
    w_offset: usize,
    h_offset: usize,
    max_w: usize,
    max_h: usize,
    was_changed: bool,
    mode: ScrollMode,
    w_pointer: usize,
    h_pointer: usize,
    x_page: usize,
    y_page: usize,
}

impl WinInfo {
    fn new(max_w: usize, max_h: usize) -> Self {
        Self {
            width: 0usize, 
            height: 0usize,
            w_offset: 0usize,
            h_offset: 0usize,
            max_w: max_w,
            max_h: max_h,
            was_changed: false,
            mode: ScrollMode::Cell,
            w_pointer: 0usize,
            h_pointer: 1usize,
            x_page: 0usize,
            y_page: 0usize,
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

    fn set_x_page(&mut self, page: usize) {
        self.x_page = page;
    }

    fn set_y_page(&mut self, page: usize) {
        self.y_page = page;
    }

    fn page(&mut self) {
        self.mode = ScrollMode::Page;
    }

    fn unpage(&mut self) {
        self.mode = ScrollMode::Cell;
    }

    fn shift(&mut self) {
        self.mode = ScrollMode::Col;
    }

    fn unshift(&mut self) {
        self.mode = ScrollMode::Cell;
    }

    fn update_w_h(&mut self, cur_w: usize, cur_h: usize) {
        self.width = cur_w;
        self.height = cur_h;
    }

    fn restore_unchanged(&mut self) {
        self.was_changed = false;
    }

    fn h_offset_up(&mut self) {
        match self.mode {
            ScrollMode::Cell => self.dec_h_pointer(),
            ScrollMode::Col => self.unshift_page_h(),
            ScrollMode::Page => self.dec_page_h(),
        }
        self.was_changed = true;
    }

    fn h_offset_down(&mut self) {
        match self.mode {
            ScrollMode::Cell => self.inc_h_pointer(),
            ScrollMode::Col => self.shift_page_h(),
            ScrollMode::Page => self.inc_page_h(),
        }
        self.was_changed = true;
    }

    fn w_offset_left(&mut self) {
        match self.mode {
            ScrollMode::Cell => self.dec_w_pointer(),
            ScrollMode::Col => self.unshift_page_w(),
            ScrollMode::Page => self.dec_page_w(),
        }
        self.was_changed = true;
    }

    fn w_offset_right(&mut self) {
        match self.mode {
            ScrollMode::Cell => self.inc_w_pointer(),
            ScrollMode::Col => self.shift_page_w(),
            ScrollMode::Page => self.inc_page_w(),
        }
        self.was_changed = true;
    }

    fn dec_h_pointer(&mut self) {
        if self.h_pointer > 1 {
            if self.h_pointer - 1 % self.y_page == 0 {
                self.h_offset = self.h_offset.saturating_sub(1);
            }
            self.h_pointer = self.h_pointer.saturating_sub(1);
        }
    }

    fn inc_h_pointer(&mut self) {
        if self.h_pointer < self.max_h - 1 {
            self.h_pointer += 1;
        }
        if self.h_pointer - 1 % self.y_page == 0 {
            self.h_offset += 1;
        }
    }

    fn dec_page_h(&mut self) {
        if self.h_offset.saturating_sub(self.y_page) > 1 {
            self.h_offset = self.h_offset.saturating_sub(self.y_page);
        } else {
            self.h_offset = 0;
        }
        if self.h_pointer.saturating_sub(self.y_page) > 1 {
            self.h_pointer = self.h_pointer.saturating_sub(self.y_page);
        } else {
            self.h_pointer = 1;
        }
    }

    fn inc_page_h(&mut self) {
        if self.h_offset + self.y_page < self.max_h {
            self.h_offset += self.y_page;
        } else {
            self.h_offset = self.max_h - 1;
        }
        if self.h_pointer + self.y_page < self.max_h {
            self.h_pointer += self.y_page;
        } else {
            self.h_pointer = self.max_h - 1;
        }
    }

    fn unshift_page_h(&mut self) {
        if self.h_offset > 1 {
            self.h_offset = self.h_offset.saturating_sub(1);
        }
        if self.h_pointer > 1 {
            self.h_pointer = self.h_pointer.saturating_sub(1);
        }
    }

    fn shift_page_h(&mut self) {
        if self.h_offset < self.max_h - 1 {
            self.h_offset += 1;
        }
        if self.h_pointer < self.max_h - 1 {
            self.h_pointer += 1;
        }
    } 

    fn dec_w_pointer(&mut self) {
        if self.w_pointer > 0 {
            if self.w_pointer % self.x_page == 0 {
                self.w_offset = self.w_offset.saturating_sub(1);
            }
            self.w_pointer = self.w_pointer.saturating_sub(1);
        }
    }

    fn inc_w_pointer(&mut self) {
        if self.w_pointer < self.max_w - 1 {
            self.w_pointer += 1;
        }
        if self.w_pointer % self.x_page == 0 {
            self.w_offset += 1;
        }
    }

    fn dec_page_w(&mut self) {
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
    }

    fn inc_page_w(&mut self) {
        if self.w_offset + self.x_page < self.max_w {
            self.w_offset += self.x_page;
        } else {
            self.w_offset = self.max_w - 1;
        }
        if self.w_pointer + self.x_page < self.max_w {
            self.w_pointer += self.x_page;
        } else {
            self.w_pointer = self.max_w - 1;
        }
    }

    fn unshift_page_w(&mut self) {
        if self.w_offset > 0 {
            self.w_offset = self.w_offset.saturating_sub(1);
        }
        if self.w_pointer > 0 {
            self.w_pointer = self.w_pointer.saturating_sub(1);
        }
    }

    fn shift_page_w(&mut self) {
        if self.w_offset < self.max_w - 1 {
            self.w_offset += 1;
        }
        if self.w_pointer < self.max_h - 1 {
            self.h_pointer += 1;
        }
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
fn read_char(kbd: &mut File) -> Option<input_event> {
    let mut buf = [0u8; mem::size_of::<input_event>()];
    match kbd.read_exact(&mut buf) {
        Err(_) => None,
        Ok(_) => {
            let ev = unsafe { 
                std::ptr::read_unaligned(buf.as_ptr() as *const input_event)
            };
            Some(ev)
        }
    }
}
