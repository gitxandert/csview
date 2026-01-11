use std::{
    env, 
    fs::{self, File},
    io::{self, Error, ErrorKind, Read, Write, stdout}
};

use crate::terminal::{h_ptr, w_ptr, WinInfo};

pub struct Cells {
    rows: Vec<Vec<String>>,
    num_cols: usize,
    num_rows: usize,
    widths: Vec<usize>,
    heights: Vec<usize>,
}

impl Cells {
    fn new(num_cols: usize, num_rows: usize) -> Self {
        let rows = Vec::<Vec<String>>::new();
        let widths = vec![12usize; num_cols];
        let heights = vec![1usize; num_rows];

        Self { rows, num_cols, num_rows, widths, heights }
    }

    pub fn xy(&self) -> (usize, usize) {
        (self.num_cols, self.num_rows)
    }

    fn push_row(&mut self, row: Vec<String>) {
        self.rows.push(row);
    }

    fn get_row(&mut self, idx: usize) -> &Vec<String> {
        if idx > self.num_rows - 1 {
            return &self.rows[self.num_rows - 1];
        } else {
            return &self.rows[idx];
        }
    }

    fn num_cols(&self) -> usize {
        self.num_cols
    }

    fn num_rows(&self) -> usize {
        self.num_rows
    }
}

fn parse_by_newline(block: &str) -> Vec<String> {
    let mut parsed = Vec::<String>::new();
    
    let mut line = String::new();
    let mut is_quoted = false;
    let mut saw_newline = false;
    
    for c in block.chars() {
        match c {
            '"' => {
                is_quoted = !is_quoted;
                line.push(c);
            }
            '\n' => {
                // guard against consecutive newlines
                if !saw_newline {
                    saw_newline = true;
                    if !is_quoted {
                        parsed.push(line.clone());
                        line.clear();
                    }
                }
            }
            _ => {
                saw_newline = false;
                line.push(c);
            }
        }
    }

    parsed
}

fn parse_by_comma(line: &str) -> Vec<String> {
    let mut parsed = Vec::<String>::new();
    
    let mut cell = String::new();
    let mut is_quoted = false;
    let mut quote: Option<char> = None;

    for c in line.chars() {
        match c {
            '"' => {
                match quote {
                    None => {
                        quote = Some(c);
                    }
                    Some(q) if q == c => {
                        quote = None;
                    }
                    Some(_) => {
                        cell.push(c);
                    }
                }
            }
            ',' if quote.is_none() => {
                parsed.push(cell.clone());
                cell = String::new();
            }
            _ => cell.push(c),
        }
    }

    parsed.push(cell);
    parsed
}

fn parse_csv_into_cells(csv: String) -> Result<Cells, io::Error> {
    let lines: Vec<String> = parse_by_newline(&csv);

    let col_names: Vec<String> = parse_by_comma(&lines[0]);
    let num_cols = col_names.len();
    let num_rows = lines.len();
    let mut cells = Cells::new(num_cols, num_rows);
    cells.push_row(col_names);

    for i in 1..num_rows {
        let row: Vec<String> = parse_by_comma(&lines[i]);
        cells.push_row(row);
    }

    Ok(cells)
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

    let mut file = fs::read_to_string(filename)?;

    let cells = parse_csv_into_cells(file)?;

    Ok(cells)
}

pub fn show_csv(cells: &mut Cells, w_info: &mut WinInfo) {
    unsafe {
        if w_info.changed() {
            let cur_w = *w_ptr();
            let cur_h = *h_ptr();
            let mut out = stdout();

            write!(out, "\x1b[H\x1b[2J").unwrap();
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

            // consistent row_len
            let row_len = cells.num_cols();
            let num_rows = cells.num_rows();

            let widths = cells.widths.clone();
            for _ in 0..rows.min(cells.num_rows() - h_offset) {
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
                    line = "    |".to_string();
                    v_cols = cells.get_row(0);
                } else {
                    // if reached cells.num_rows(),
                    // print XXXX instead of row number
                    if row < cells.num_rows() {
                        // print in hexadecimal (space-saving)
                        line = format!("{:04X}|", row);
                        v_cols = cells.get_row(row);
                    } else {
                        line = "XXXX| EOF".to_string();
                    }
                }

                // reset index for each row
                idx = orig_idx;
                if row_len > 0 && idx < row_len {
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
                                        cell = format!(" {:<width$}... |", 
                                            contents, width = width - 3);
                                    } else {
                                        contents = col
                                            .chars()
                                            .take(width)
                                            .collect();
                                        cell = format!(" {:<width$} |",
                                            contents, width = width);
                                    }
                                }
                                None => cell = " !!!CSVERR!!! |".to_string(),
                            }

                            // highlight the current cell
                            if w_info.w_pointer == idx &&
                               w_info.h_pointer == row {
                                cell = "\x1b[7m".to_string() + &cell + "\x1b[27m";
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

            let row_num_len = 5;
            
            // move cursor to bottom
            write!(out, "\x1b[{};{}H\x1b[2K", cur_h, 1)
                .unwrap();

            out.flush().unwrap();
        }
    }
}
