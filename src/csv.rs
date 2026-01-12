use std::{
    env, 
    fs::{self, File},
    io::{self, Error, ErrorKind, Read, Write, stdout}
};

use crate::terminal::{h_ptr, w_ptr, WinInfo};


#[derive(Debug)]
struct Cell {
    pub content: String,
    pub width: usize,
    pub height: usize,
    pub text_offset: usize,
}

impl Cell {
    fn new(content: String) -> Self {
        Self {
            content,
            width: 12usize,
            height: 1usize,
            text_offset: 0usize,
        }
    }

    fn set_width(&mut self, w: usize) {
        self.width = w;
    }

    fn set_height(&mut self, h: usize) {
        self.height = h;
    }

    fn dec_text_offset(&mut self, val: usize) {
        self.text_offset = self.text_offset.saturating_sub(val);
    }

    fn inc_text_offset(&mut self, val: usize) {
        if self.len().saturating_sub(self.text_offset) > self.width {
            self.text_offset += val;
        } else {
            self.text_offset = self.len().saturating_sub(self.width);
        }
    }

    #[inline]
    fn len(&self) -> usize {
        self.content.len()
    }

    fn format_cell(&self) -> String {
        let mut cell = String::new();
        if self.len() - self.text_offset > self.width {
            let content: String = self.content
                .chars()
                .skip(self.text_offset)
                .take(self.width - 3)
                .collect();
            cell = format!("| {:<width$}... ", 
                content, width = self.width - 3);
        } else {
            let content: String = self.content
                .chars()
                .skip(self.text_offset)
                .take(self.width)
                .collect();
            cell = format!("| {:<width$} ",
                content, width = self.width);
        }
        
        cell
    }
}

pub struct Cells {
    rows: Vec<Vec<Cell>>,
    num_cols: usize,
    num_rows: usize,
    were_changed: bool,
}

impl Cells {
    fn new(num_cols: usize, num_rows: usize) -> Self {
        let rows = Vec::<Vec<Cell>>::new();
        let text_offsets = vec![0usize; num_cols];
        let were_changed = false;

        Self { rows, num_cols, num_rows, were_changed }
    }

    fn changed(&mut self) -> bool {
        if self.were_changed {
            self.were_changed = false;
            return true;
        }

        return false;
    }

    pub fn xy(&self) -> (usize, usize) {
        (self.num_cols, self.num_rows)
    }

    pub fn set_text_offset(&mut self, val: i32, row: usize, col: usize) {
        let mut cell_row = &mut self.rows[row];
        let mut cell = &mut cell_row[col];
        if val > 0 {
            cell.inc_text_offset(val as usize);
        } else {
            let val = val.abs();
            cell.dec_text_offset(val as usize);
        }

        self.were_changed = true;
    }

    fn push_row(&mut self, row: Vec<Cell>) {
        self.rows.push(row);
    }

    fn get_row(&mut self, idx: usize) -> &Vec<Cell> {
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

fn parse_by_comma(line: &str) -> Vec<Cell> {
    let mut parsed = Vec::<Cell>::new();
    
    let mut cell_str = String::new();
    let mut is_quoted = false;

    for c in line.chars() {
        match c {
            '"' => {
                is_quoted = !is_quoted;
            }
            ',' if !is_quoted => {
                let cell = Cell::new(cell_str.clone());
                parsed.push(cell);
                cell_str = String::new();
            }
            _ => cell_str.push(c),
        }
    }

    let cell = Cell::new(cell_str);
    parsed.push(cell);
    parsed
}

fn parse_csv_into_cells(csv: String) -> Result<Cells, io::Error> {
    let lines: Vec<String> = parse_by_newline(&csv);

    let col_names: Vec<Cell> = parse_by_comma(&lines[0]);
    for c in &col_names {
        eprintln!("{}", c.content); 
    }
    let num_cols = col_names.len();
    let num_rows = lines.len();
    let mut cells = Cells::new(num_cols, num_rows);
    cells.push_row(col_names);

    for i in 1..num_rows {
        let row: Vec<Cell> = parse_by_comma(&lines[i]);
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
    file = file.replace("\r", " ");

    let cells = parse_csv_into_cells(file)?;

    Ok(cells)
}

pub fn show_csv(cells: &mut Cells, w_info: &mut WinInfo) {
    unsafe {
        if w_info.changed() || cells.changed() {
            let cur_w = *w_ptr();
            let cur_h = *h_ptr();

            // use double-buffering for one smooth write
            let mut frame = String::with_capacity(8192);

            // move cursor to top_left
            frame.push_str("\x1b[H");
            
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

            for _ in 0..rows.min(cells.num_rows() - h_offset) {
                /*
                * Each line needs to be formatted like so:
                * | content(...) | content(...) | content(...) |
                * so the total width of the screen needs to be portioned:
                * 5 + n x (1 + 1 + cell_width + 1)
                * if the cell at the end goes over, it is dropped
                */

                // move cursor to (row+1, col=1)
                frame.push_str(&format!("\x1b[{};1H\x1b[2K", t_row+1));

                let mut line: String = "".to_string();
                // reference to vec of cols
                let mut v_cols = &Vec::new();
                // always print col names
                if t_row == 0 {
                    line = "    ".to_string();
                    v_cols = cells.get_row(0);
                } else {
                    // if reached cells.num_rows(),
                    // print XXXX instead of row number
                    if row < cells.num_rows() {
                        // print in hexadecimal (space-saving)
                        line = format!("{:04X}", row);
                        v_cols = cells.get_row(row);
                    } else {
                        line = "XXXX| EOF".to_string();
                    }
                }


                // reset index for each row
                idx = orig_idx;
                let mut sub = 0;
                if row_len > 0 && idx < row_len {
                    loop {
                        if idx < row_len {
                            let row_cell = match v_cols.get(idx) {
                                Some(cell) => cell,
                                // always have a cell; fill with dummy value for now
                                None => &Cell::new("!!!CSVERR!!!".to_string()),
                            };
                            
                            let width = row_cell.width;
                            if line.len().saturating_sub(sub) + width > cur_w {
                                break;
                            }
                            
                            let mut cell: String = row_cell.format_cell();

                            // highlight the current cell
                            if w_info.w_pointer == idx &&
                               w_info.h_pointer == row {
                                cell = format!("\x1b[7m{}\x1b[27m", cell);
                                // take escape sequence into account for width calculation above
                                sub = 11;
                            }
                           
                            line += &cell;
                            idx += 1;
                        } else {
                            break;
                        }
                    }

                    // write line
                    frame.push_str(&format!("\x1b[4m{line}|\x1b[24m"));
                    row += 1;
                    t_row += 1;
                }
            }
            w_info.set_x_page(idx, orig_idx);
            w_info.set_y_page(row, h_offset);

            let row_num_len = 5;
            
            // move cursor to bottom
            frame.push_str(&format!("\x1b[{};{}H\x1b[2K", cur_h, 1));

            // end update
            let mut out = stdout().lock();
            write!(out, "{}", frame).unwrap();

            out.flush().unwrap();
        }
    }
}
