use std::{
    env, 
    fs::{self, File},
    io::{self, Error, ErrorKind, Read, Write, stdout}
};

use crate::terminal::{h_ptr, w_ptr, WinInfo};

/*
 * To preserve cell formatting:
 * - iterate through entire cell
 * - collect indices of escape characters
 *      - for \x1b, collect a second index,
 *        based on whether it is followed by [:
 *          - if so, 
 *            match against @..~
 *          - if not,
 *            second index immediately follows first
 *      - collect \n index and update cell's height (depth?)
 *      - ignore/remove all others
 *  - while collecting these indices,
 *    record "real" length of string
 *  - when formatting cell, check if offset
 *    includes any part of an escape sequence
 *      - if so, the truncated text needs to be reformatted
*/

#[derive(Debug)]
struct EscSeq {
    seq: String,
    start: usize,
    end: usize,
    len: usize,
}

impl EscSeq {
    fn new() -> Self {
        Self {
            seq: String::new(),
            start: 0usize,
            end: 0usize,
            len: 0usize,
        }
    }

    fn clone(&self) -> Self {
        Self {
            seq: self.seq.clone(),
            start: self.start,
            end: self.end,
            len: self.len,
        }
    }

    fn push_seq(&mut self, c: char) {
        self.seq.push(c);
    }

    fn set_start(&mut self, c: char, i: usize) {
        self.push_seq(c);
        self.start = i;
    }

    fn set_end(&mut self, c: char, i: usize) {
        self.push_seq(c);
        self.end = i;
        self.len = self.end - self.start + 1;
    }
}

#[derive(Debug)]
pub struct Cell {
    content: Vec<String>,
    escape_sequences: Vec<EscSeq>,
    lens: Vec<usize>,
    width: usize, // of the cell, not its content
    height: usize, // of the cell, not its content
    pub text_offset: usize,
    pub height_offset: usize,
}

impl Cell {
    fn new(content: String) -> Self {
        // store content as lines separated by newlines;
        // store escapes and their indices
        let mut lines = Vec::<String>::new();
        let mut esq = Vec::<EscSeq>::new();

        // len keeps track of "real" len
        // (i.e. the characters, not the formatting)
        let mut lens = Vec::<usize>::new();
        let mut len = 0usize;

        let mut line = String::new();
        let mut e = EscSeq::new();
        
        let mut i = 0usize;

        let mut x = false;
        let mut is_csi = false;
        // check for escape sequences;
        for c in content.chars() {
            match c {
                '\x1b' => {
                    x = true;
                    e.set_start(c, i);
                }
                '[' if x => {
                    is_csi = true;
                    e.push_seq(c);
                }
                '@'..='~' if x => {
                    e.set_end(c, i);
                    esq.push(e.clone());
                    e = EscSeq::new();

                    x = false;
                    is_csi = false;
                }
                '\n' if !x => {
                    // push line and len
                    lines.push(line);
                    lens.push(len);
                    line = String::new();
                    len = 0usize;
                    continue;
                }
                _ => {
                    match x {
                        false => len += 1,
                        true => {
                            match is_csi {
                                true => e.push_seq(c),
                                false => {
                                    e.set_end(c, i);
                                    esq.push(e.clone());
                                    e = EscSeq::new();

                                    x = false;
                                }
                            }
                        }
                    }
                }
            }
           
            line.push(c);
            i += 1;
        }

        // push final line and len
        lines.push(line);
        lens.push(len);

        Self { 
            content: lines,
            escape_sequences: esq,
            lens,
            width: 12usize,
            height: 1usize,
            text_offset: 0usize,
            height_offset: 0usize,
        }
    }

    fn format_cell(&self) -> (String, usize) {
        let mut cell = String::new();
        let mut content = self.content();
        let mut extra_len = 0usize;
      
        // iterate through escape_sequences
        // while escape_sequence[i] < self.text_offset;
        // any escape sequence before the
        // (real) text offset will be added to the start
        // of the formatted string; and escape sequence
        // after will be added to the end of the string
        let mut fmt_start = String::new();
        let mut fmt_end = String::new();
        let mut skip = self.text_offset;
        for esc in &self.escape_sequences {
            if esc.start > skip {
                fmt_end.push_str(&esc.seq);
                extra_len += esc.len;
            } else {
                fmt_start.push_str(&esc.seq);
                extra_len += esc.len;
                skip += (esc.end.saturating_sub(skip) + 1);
            }
        }

        let mut take = self.width;
        for esc in &self.escape_sequences {
        // should have skipped past all previous EscSeq,
        // so only need to take remaining into account
            if esc.end < skip || esc.start > skip + take {
                continue;
            }
            take += esc.len;
        }

        let post_skip: String = content
            .chars()
            .skip(skip)
            .collect();

        let pslen = post_skip.len();
        let mut add = String::new();
        if pslen > take {
            take = take.saturating_sub(3);
            add = "...".to_string();
        } else if post_skip.len() < take {
            let ws = take.saturating_sub(pslen);
            add = format!("\x1b[4m{:<width$}", " ", width = ws);
        }

        extra_len += add.len();

        let mut taken: String = post_skip
            .chars()
            .take(take)
            .collect();

        taken.push_str(&add);
        fmt_end.push_str("\x1b[4m");

        cell = format!(
            "| {}{}{} ", 
            fmt_start, taken, fmt_end, 
        );

        (cell, extra_len)
    }

    pub fn write(&mut self, input: String, cur_pos: usize) {
        let old_len = self.len();
        let content = self.content();
        let take = self.text_offset + cur_pos;
        let start: String = content
            .chars()
            .take(take)
            .collect();
        let end: String = content
            .chars()
            .skip(take)
            .collect();

        self.set_content(format!("{start}{input}{end}"));
        
        let cur_len = self.len() + 1;
        self.set_len(cur_len);
    /*    
        if cur_len >= self.width {
            if cur_len > old_len {
                self.inc_text_offset(cur_len.saturating_sub(old_len));
            } else {
                self.dec_text_offset(old_len.saturating_sub(cur_len));
            }
        }
    */
       
    }

    pub fn delete(&mut self, cur_pos: usize) {
        // check if at beginning of string;
        // if so, don't delete anything
        let check = cur_pos;
        let subbed = cur_pos.saturating_sub(1);
        if check.saturating_sub(subbed) == 0 && self.text_offset == 0 { return; }

        let take = self.text_offset + subbed;
        let content = self.content();

        let start: String = content
            .chars()
            .take(take)
            .collect();
        let end: String = content
            .chars()
            .skip(take + 1)
            .collect();

        self.set_content(format!("{start}{end}"));
        self.set_len(self.len().saturating_sub(1));
    }
    
    fn set_content(&mut self, new: String) {
        let mut content = self.content.get_mut(self.height_offset).unwrap();
        *content = new;
    }

    fn set_len(&mut self, new: usize) {
        let mut len = self.lens.get_mut(self.height_offset).unwrap();
        *len = new;
    }

    fn set_width(&mut self, w: usize) {
        self.width = w;
    }

    fn set_height(&mut self, h: usize) {
        self.height = h;
    }

    pub fn dec_text_offset(&mut self, val: usize) {
        self.text_offset = self.text_offset.saturating_sub(val);
    }

    pub fn inc_text_offset(&mut self, val: usize) {
        if self.len().saturating_sub(self.text_offset) > self.width {
            self.text_offset += val;
        } else {
            self.text_offset = self.len().saturating_sub(self.width);
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        *self.lens.get(self.height_offset).unwrap()
    }

    #[inline]
    pub fn content(&self) -> String {
        self.content.get(self.height_offset).unwrap().clone()
    }
}

pub struct Cells {
    rows: Vec<Vec<Cell>>,
    num_cols: usize,
    num_rows: usize,
    w_cell: (usize, usize),
    were_changed: bool,
}

impl Cells {
    fn new(num_cols: usize, num_rows: usize) -> Self {
        let rows = Vec::<Vec<Cell>>::new();
        let text_offsets = vec![0usize; num_cols];
        let w_cell = (0usize, 0usize);
        let were_changed = false;

        Self { rows, num_cols, num_rows, w_cell, were_changed }
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

    fn set_w_cell(&mut self, row: usize, idx: usize) {
        self.w_cell = (row, idx);
    }

    fn push_row(&mut self, row: Vec<Cell>) {
        self.rows.push(row);
    }

    fn get_row(&mut self, idx: usize) -> &Vec<Cell> {
        if idx > self.num_rows - 1 {
            return self.rows.get(self.num_rows - 1).unwrap();
        } else {
            return self.rows.get(idx).unwrap();
        }
    }

    fn num_cols(&self) -> usize {
        self.num_cols
    }

    fn num_rows(&self) -> usize {
        self.num_rows
    }

    pub fn write_to_cell(&mut self, input: String, cur_pos: usize) {
        let mut cell = self.w_cell();
        cell.write(input, cur_pos);
    }

    pub fn delete_from_cell(&mut self, cur_pos: usize) {
        let mut cell = self.w_cell();
        cell.delete(cur_pos);
    }

    pub fn w_cell(&mut self) -> &mut Cell {
        let mut row = self.rows.get_mut(self.w_cell.0).unwrap();
        row.get_mut(self.w_cell.1).unwrap()
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

fn parse_by_delim(line: &str, delim: char) -> Vec<Cell> {
    let mut parsed = Vec::<Cell>::new();
    
    let mut cell_str = String::new();
    let mut is_quoted = false;
   
    let delim = match delim {
        't' => '\t',
        _ => delim
    };

    for c in line.chars() {
        match c {
            '"' => {
                is_quoted = !is_quoted;
            }
            ch if ch == delim && !is_quoted => {
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

fn make_col_idx(num_cols: usize) -> Vec<Cell> {
    let mut row = Vec::<Cell>::new();
    let mut idx = "A".to_string();
    for _ in 0..num_cols {
        let con_width = idx.len();
        let content = {
            if con_width % 2 == 0{
                let ws = 6_usize.saturating_sub(con_width / 2);
                format!("{:<lw$}{}{:<rw$}",
                     " ", idx.clone(), " ", lw = ws, rw = ws)
            } else {
                let lw = 5_usize.saturating_sub(con_width / 2);
                let rw = 6_usize.saturating_sub(con_width / 2);
                format!("{:<lw$}{}{:<rw$}",
                     " ", idx.clone(), " ", lw = lw, rw = rw)

            }
        };
                    
        let cell = Cell::new(content);
        row.push(cell);
       
        let mut i = 1;
        let chars: String = idx.chars().rev().collect();
        let mut add_a = false;
        let mut inc_next = false;
        let new_idx: String = chars
            .chars()
            .map(|c|
                if c == 'Z' {
                    if i == idx.len() {
                        add_a = true;
                    } else {
                        inc_next = true;
                    }
                    i += 1;
                     'A'
                } else {
                    let mut cc = c;
                    if i == 1 || inc_next {
                        let num = c as u32 + 1;
                        cc = char::from_u32(num).unwrap();
                        inc_next = false;
                    }
                    i += 1;
                    cc
                }
            ).collect();

        let new_idx: String = new_idx.chars().rev().collect();
        if add_a {
            idx = format!("{}A", new_idx);
        } else {
            idx = new_idx;
        }
    }

    row
}

fn parse_csv_into_cells(csv: String, delim: char) -> Result<Cells, io::Error> {
    let lines: Vec<String> = parse_by_newline(&csv);

    let col_names: Vec<Cell> = parse_by_delim(&lines[0], delim);
    
    let num_cols = col_names.len();
    let num_rows = lines.len() + 1;
    
    let mut cells = Cells::new(num_cols.clone(), num_rows);
    let col_idx: Vec<Cell> = make_col_idx(num_cols);
    cells.push_row(col_idx);
    cells.push_row(col_names);

    for i in 1..num_rows - 1 {
        let row: Vec<Cell> = parse_by_delim(&lines[i], delim);
        cells.push_row(row);
    }

    Ok(cells)
}

pub fn load_csv(filename: String, delim: char) -> Result<Cells, io::Error> {
    let mut file = fs::read_to_string(filename)?;
    // don't parse carriage returns
    file = file.replace("\r", " ");

    let cells = parse_csv_into_cells(file, delim)?;

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

            // indices for cells in rows
            let orig_idx = w_offset;
            let mut idx = orig_idx;

            // save which cell can be written to
            let mut w_row = 0usize;
            let mut w_idx = 0usize;

            // consistent row_len
            let row_len = cells.num_cols();
            let num_rows = cells.num_rows();

            // length of rendered line
            // (starts with row index)
            let mut line_w = 5usize;

            let mut focus = String::new();

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
                let mut v_cols = &Vec::<Cell>::new();
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

                // reset index and len for each row
                idx = orig_idx;
                line_w = 5usize;
                if row_len > 0 && idx < row_len {
                    loop {
                        if idx < row_len {
                            let row_cell = match v_cols.get(idx) {
                                Some(cell) => cell,
                                // always have a cell; fill with dummy value for now
                                None => &Cell::new("!!!CSVERR!!!".to_string()),
                            };
                            
                            if line_w > cur_w {
                                break;
                            }
                            
                            let (mut cell, extra_len) = row_cell.format_cell();

                            if w_info.w_pointer == idx &&
                               w_info.h_pointer == row {
                                // save info for showing cursor
                                w_info.set_cursor(t_row + 1, line_w + 1, row_cell.width);
                                // highlight cell
                                cell = format!(
                                    "\x1b[7;36;47m{}\x1b[39;49;27m", 
                                    cell
                                );
                                // take escape sequence into account for width calculation above
                                focus = row_cell.content();
                                if focus.len() > cur_w {
                                    focus = (&focus[0..cur_w]).to_string();
                                }
                                w_row = row;
                                w_idx = idx;
                            }
                            
                            cell = format!("\x1b[4m{}", cell);
                            line_w += (row_cell.width + 3);
                            line += &cell;
                            idx += 1;
                        } else {
                            break;
                        }
                    }

                    // write line
                    frame.push_str(&format!("{line}|\x1b[24m"));

                    row += 1;
                    t_row += 1;
                }
            }
            // set cell for writing
            cells.set_w_cell(w_row, w_idx);
            w_info.set_x_page(idx, orig_idx);
            w_info.set_y_page(row, h_offset);

            let row_num_len = 5;
            
            // show focus at bottom
            frame.push_str(&format!("\x1b[{};{}H\x1b[2K{}", cur_h, 1, focus));

            // end update
            let mut out = stdout().lock();
            write!(out, "{}", frame).unwrap();

            if w_info.cursor_shown() {
                let (l, c) = w_info.get_cursor();
                write!(out, "\x1b[{};{}H", l, c + 1);
            }

            out.flush().unwrap();
        }
    }
}

pub fn write_to_file(mut cells: Cells, filename: String) {
    let mut sheet = String::new();
    for i in 1..cells.num_rows {
        let mut row = String::new();
        let cur = cells.get_row(i);
        for j in 0..cur.len() {
            let mut content = String::new();
            let cell = cur.get(j).unwrap();
            for line in &cell.content {
                content.push_str(&line);
            }
            row.push_str(&content);
            if j != cur.len() - 1 {
                row.push(',');
            }
        }

        if i != cells.num_rows - 1 {
            row.push('\n');
        }

        sheet.push_str(&row);
    }
    
    match fs::write(filename, sheet) {
        Ok(()) => (),
        Err(e) => eprintln!("{e}"),
    }
}
