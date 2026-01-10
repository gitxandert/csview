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

    let (max_w, max_h) = cells.xy();
    let mut w_info = WinInfo::new(max_w, max_h);
    loop {
        process_input(&mut w_info);
        show_csv(&mut cells, &mut w_info);
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

fn process_input(w_info: &mut WinInfo) {
    let c = read_char().unwrap_or(0);
    match c {
        27 => {
            let c2 = read_char().unwrap_or(0);
            if c2 == b'[' {
                let c3 = read_char().unwrap_or(0);
                match c3 {
                    b'D' => { // left
                        w_info.w_offset_left(1);
                    }
                    b'C' => { // right
                        w_info.w_offset_right(1);
                    }
                    b'A' => { // up
                        w_info.h_offset_up(1);
                    }
                    b'B' => { // down
                        w_info.h_offset_down(1);
                    }
                    _ => (),
                }
            }
        }
        _ => (), // ignore
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
                let mut idx = w_offset;
                let row_len = v_cols.len();
                if row_len > 0 && idx < row_len{
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

struct WinInfo {
    width: usize,
    height: usize,
    w_offset: usize,
    h_offset: usize,
    max_w: usize,
    max_h: usize,
    was_changed: bool,
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
        if self.h_offset > 0 {
            self.h_offset = self.h_offset.saturating_sub(val);
            self.was_changed = true;
        }
    }

    fn h_offset_down(&mut self, val: usize) {
        if self.h_offset < self.max_h {
            self.h_offset += val;
            self.was_changed = true;
        }
    }

    fn w_offset_left(&mut self, val: usize) {
        if self.w_offset > 0 {
            self.w_offset = self.w_offset.saturating_sub(val);
            self.was_changed = true;
        }
    }

    fn w_offset_right(&mut self, val: usize) {
        if self.w_offset < self.max_w {
            self.w_offset += val;
            self.was_changed = true;
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
fn read_char() -> Option<u8> {
    let mut buf = [0u8; 1];
    match std::io::stdin().read(&mut buf) {
        Ok(0) => None,
        Ok(_) => Some(buf[0]),
        Err(_) => None,
    }
}
