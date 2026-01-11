use std::{
    env, 
    fs::{self, File},
    io::{self, Error, ErrorKind, Read, Write, stdout}
};

use crate::terminal::{h_ptr, w_ptr, WinInfo};

pub struct Cells {
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

    pub fn xy(&self) -> (usize, usize) {
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

pub fn load_csv() -> Result<Cells, io::Error> {
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

    contents = contents.replace("\r\n", "\n");

    let lines: Vec<&str> = contents.lines().collect();

    let x = lines[0].split(',').count();
    let y = lines.len();
    let mut cells = Cells::new(x, y);
    for line in lines {
        let row: Vec<String> = line
            .trim_end_matches('\r')
            .split(',')
            .map(|s| s.to_string())
            .collect();
        eprintln!("len = {}", row.len());
        cells.push_row(row);
    }

    Ok(cells)
}

pub fn show_csv(cells: &mut Cells, w_info: &mut WinInfo) {
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
            for _ in 0..rows.min(cells.len() - h_offset) {
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
                // reference to vec of cols
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

                idx = orig_idx;
                if row_len > 0 && idx < row_len {
                    // reset index for each row
                    loop {
                        if idx < row_len {
                            let width = widths[idx];
                            if line.len() + width > cur_w {
                                break;
                            }
                            let mut cell: String = "".to_string();
                            match v_cols.get(idx) {
                                Some(col) => {
                                    let mut contents: String = "".to_string();
                                    if col.chars().count() > width {
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
                                }
                                None => eprintln!("No cell"),
                            }

                            // highlight the current cell
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
            w_info.set_x_page(idx, orig_idx);
            w_info.set_y_page(row, h_offset);
            write!(out, "\x1b[{};1H\x1b[2K", t_row+1).unwrap();
            out.flush().unwrap();
        }
    }
}
