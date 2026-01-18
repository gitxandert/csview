use std::{
    env,
    fs::{self, File},
    time::{SystemTime, UNIX_EPOCH},
    path::{Component, Path, PathBuf},
    io::{self, Error, ErrorKind, Read, Write, stdout}
};

use crate::terminal::{WinChange, WinInfo};

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
    content: String,
    escape_sequences: Vec<EscSeq>,
    t_row: usize,
    t_col: usize,
    pub text_offset: usize,
}

impl Cell {
    fn new(content: String) -> Self {
        // store escapes and their indices
        let mut esq = Vec::<EscSeq>::new();

        // len keeps track of "real" len
        // (i.e. the characters, not the formatting)
        let mut real_len = 0usize;

        let mut line = String::new();
        let mut e = EscSeq::new();
        
        let mut x = false;
        let mut is_csi = false;
        // check for escape sequences;
        for c in content.chars() {
            match c {
                '\x1b' => {
                    x = true;
                    e.set_start(c, i);
                    continue;
                }
                '[' if x => {
                    is_csi = true;
                    e.push_seq(c);
                    continue;
                }
                '@'..='~' if x => {
                    e.set_end(c, i);
                    esq.push(e.clone());
                    e = EscSeq::new();

                    x = false;
                    is_csi = false;
                    continue;
                }
                '\n' => {
                    // skip new lines
                    continue;
                }
                _ => {
                    match x {
                        false => line.push(c),
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
        }

        Self { 
            content: line,
            escape_sequences: esq,
            t_row: 0usize,
            t_col: 0usize,
            text_offset: 0usize,
        }
    }

    // ignore escapes for now
    fn format(&self) -> &str {
        let content = self.content();
     
        let skip = self.text_offset;
        let take = self.width;

        let taken = &content[skip..skip + take];

        "| " + taken + " "    
    }

    pub fn len(&self) -> usize {
        self.content.len()
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn set_content(&mut self, input: String) {
        self.content = input;
    }

    // directly set it
    pub fn set_text_offset(&mut self, val: usize) {
        self.text_offset = val;
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
}

pub struct Column {
    cells: Vec<Cell>,
    start: usize, // terminal col where this begins
    width: usize,
}

impl Column {
    fn new() -> Self {
        Self {
            cells: Vec::<Cell>::new(),
            width: 12usize,
        }
    }

    fn push_cell(&mut self, cell: Cell) {
        self.cells.push(cell);
    }

    pub fn set_width(&mut self, w: usize) {
        self.width = w;
    }

    pub fn width(&self) -> usize {
        return self.width
    }

    pub fn get_cell(&self, idx: usize) -> &Cell {
        &self.cells[idx]
    }
}

pub struct Cells {
    header: Vec<Cell>,
    col_idx: Vec<Cell>,
    row_idx: Column,
    columns: Vec<Column>,
    w_cell: (usize, usize),
    changed: bool,
    pub written: bool,
}

impl Cells {
    fn new(header: Vec<Cell>, col_idx: Vec<Cell>, row_idx: Column, num_cols: usize) -> Self {
        let columns = Vec::<Cell>::with_capacity(num_cols);
        let w_cell = (0usize, 0usize);
        let changed = false;
        let written = false;

        Self { row_idx, columns, w_cell, changed, written }
    }

    fn changed(&mut self) -> bool {
        let ret = self.changed;
        self.changed ^= ret;

        ret
    }

    pub fn set_column_width(&mut self, w: usize) {
        let width = (3 > w) as usize * 3 + (w > 3) as usize * w;
        let idx = self.w_cell.0; // w_cell is where the focus is
        let mut column = self.columns.get_mut(col_idx).unwrap();
        column.set_width(width);
        self.changed = true;
    }

    fn set_w_cell(&mut self, col: usize, row: usize) {
        self.w_cell = (col, row);
    }

    fn push_to_col(&mut self, col: usize, cell: Cell) {
        self.columns[col].push(cell);
    }

    fn get_column(&self, idx: usize) -> &Column {
        &self.columns[idx]
    }

    fn num_cols(&self) -> usize {
        self.columns.len()
    }

    fn num_rows(&self) -> usize {
        self.columns[0].len()
    }

    fn get_col_id(&self, idx: usize) -> &str {
        self.col_idx[0].format()
    }

    fn get_col_name(&self, idx: usize) -> &str {
        let content = self.header[idx].format();
        let formatted = "\x1b[30;47m" + content + "\x1b39;49m";
        formatted
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

    if !saw_newline {
        parsed.push(line);
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
            ch if ch == delim && ch != '"' && !is_quoted => {
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

fn make_row_idx(len: usize) -> Column {
    let mut row_idx = Column::new();
    row_idx.set_width(4usize);
    let blank = Cell::new("    ");
    let head = Cell::new("\x1b[1;4;30;47mHEAD\x1b[0m".to_string());
    row_idx.push_cell(blank);
    row_idx.push_cell(head);

    for i in 1..=len {
        let index: String = format!("\x1b[4;30;47m{:04X}\x1b[39;49m", i);
        row_idx.push_cell(Cell::new(index));
    }

    row_idx
}

fn parse_csv_into_cells(csv: String, delim: char) -> Result<Cells, io::Error> {
    // extract lines, but parse into columns
    let lines: Vec<String> = parse_by_newline(&csv);

    let header: Vec<Cell> = parse_by_delim(&lines.remove(0), delim);
    let col_len = header.len();
    let col_idx: Vec<Cell> = make_col_idx(col_len);
    
    let row_idx: Column = make_row_idx(lines.len());
    let mut cells = Cells::new(header, col_idx, row_idx, col_len);

    for line in lines {
        let row: Vec<Cell> = parse_by_delim(&line, delim);
        for i in 0..col_len {
            cells.push_to_col(i, row[i]);
        }
    }

    Ok(cells)
}

pub fn load_csv(filename: String, delim: char) -> Result<Cells, io::Error> {
    let mut file = fs::read_to_string(filename.clone())?;
    // don't parse carriage returns
    file = file.replace("\r", " ");

    let cells = parse_csv_into_cells(file, delim)?;

    Ok(cells)
}

pub fn show_csv(cells: &mut Cells, w_info: &mut WinInfo) {
    match w_info.changed {
        WinChange::Cell => {
            // redraw the focused cell,
            // with changed content,
            // plus the rest of the line
            //
            // 1 line
            let mut w_cell = cells.w_cell();

        }
        WinChange::Focus => {
            // redraw the last focused cell,
            // w/o highlighting,
            // plus the rest of its line,
            // and the new focused cell, 
            // w/ highlighting,
            // plus the rest of its line
            //
            // 2 lines
        }
        WinChange::ColWidth => {
            // redraw the column whose width has changed,
            // plus all columns after
            //
            // all lines
        }
        WinChange::Row => {
            // shift rows and row_idx,
            // but no need to redraw header and col_idx
            //
            // all lines, except for header and col_idx
        }
        WinChange::Columns => {
            // shift columns and col_idx,
            // but no need to redraw row_idx
            //
            // all lines, but not the row_idx column
        }
        WinChange::Screen => {
            // draw to the new terminal screen dimensions;
            // if they shrink, 
            // only maybe need to redraw focused content
            //
            let mut flush = false;
            if w_info.old_height > w_info.height {
                // focused content always drawn at height, width
                w_info.draw_focused_content();
                flush = true;
            }
            if w_info.width > w_info.old_width {
                let mut gap_start = w_info.old_width;
                let orig = w_info.w_offset + w_info.w_page;
                let mut idx = orig;
                let mut reset_lines = true;
                loop {
                    idx += 1;
                    let col = cells.get_column(idx);
                    if col.start + col.width < w_info.width {
                        if reset_lines {
                            // if this is the first pass through, 
                            // reset each line starting at the column
                            for row in 0..w_info.height {
                                let reset = "\x1b[" + 
                                            row + ":" + 
                                            col.start "H\x1b[K";
                                w_info.push_to_frame(reset);
                            }
                        }
                        let col_id = cells.get_col_id(idx);
                        let col_name = cells.get_col_name(idx);
                        w_info.draw_column(col_id, col_name, col);
                    } else {
                        break;
                    }
                }
                if idx != orig {
                    w_info.set_w_page(
                        w_info.w_offset, idx
                    );
                    flush = true;
                }
            }

            if flush {
                w_info.flush();
            }
            
            w_info.changed = WinChange::Non;
        }
        WinChange::Non => {
            // do nothing
        }
    }
}

pub fn save_backup(file: String) -> Result<(), io::Error> {
    let content = fs::read_to_string(&file)?;
    // find home dir or current dir; propagate error if neither
    let home_dir = match env::home_dir() {
        Some(path) => path,
        None => match env::current_dir() {
            Ok(path) => path,
            Err(e) => return Err(e),
        }
    };

    let file = Path::new(&file);
    let abs = if file.is_absolute() {
        file.to_path_buf()
    } else {
        env::current_dir()?.join(file)
    };

    let abs = fs::canonicalize(&abs)?;

    let parent = abs.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound, "Could not find parent dir"
        )
    })?;

    let stem = abs.file_stem().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound, "Could not find file stem"
        )
    })?;

    let rel_parent: PathBuf = parent
                .components()
                .filter_map(|c| match c {
                    Component::Normal(p) => Some(p),
                    _ => None,
                })
                .collect();

    eprintln!("{:?}", rel_parent);

    // either open or create backup directory
    let dir_path = Path::new(&home_dir)
        .join(".csview")
        .join("backups")
        .join(rel_parent)
        .join(stem);

    fs::create_dir_all(&dir_path)?;

    let backup_dir = fs::read_dir(&dir_path)?;
    let mut backup_files: Vec<_> = backup_dir
        .filter_map(|res| res.ok())
        .map(|e| e.path())
        .collect();

    // limit files in backup dir to 10
    let mut dir_len = backup_files.len();
    let mut stage_for_removal = Vec::<_>::new();
    for bf in &backup_files {
        if dir_len >= 10 {
            stage_for_removal.push(bf.clone());
            dir_len = dir_len.saturating_sub(1);
        } else {
            break;
        }
    }

    // set default to the name of the youngest file,
    // with timestamp incremented by 1
    let default = match backup_files.pop() {
        Some(backup) => {
            let stem = backup.file_stem().unwrap_or_default();
            let ext = backup.extension().unwrap_or_default();

            let stem_str = stem.to_string_lossy();
            let ext_str = ext.to_string_lossy();

            let mut parts: Vec<&str> = stem_str.split('_').collect();
            let prefix = parts[0];
            let timestamp_str = match parts.pop() {
                Some(ts) => ts,
                None => "0",
            };
            let timestamp: u64 = match timestamp_str.parse() {
                Ok(val) => val,
                Err(_) => 0u64,
            };
            let faux_new = timestamp + 1;

            let path = format!("{}_{}.{}", prefix, faux_new, ext_str);

            dir_path.join(path)
        }
        // if no previous file, just make it
        // {filename}_0(.ext)
        None => {
            let stem = file.file_stem().unwrap_or_default();
            let ext = file.extension().unwrap_or_default();

            let stem_str = stem.to_string_lossy();
            let ext_str = ext.to_string_lossy();

            let mut parts: Vec<&str> = stem_str.split('.').collect();
            let prefix = parts[0];
            let path = match parts.get(0) {
                Some(ext) => format!("{}_0.{}", prefix, ext),
                None => format!("{}_0", prefix),
            };

            dir_path.join(path)
        }
    };

    // try to get timestamp to affix to file stem
    let time = SystemTime::now().duration_since(UNIX_EPOCH);
    let backup = match time {
        Ok(t) => {
            let stem = file.file_stem().unwrap_or_default();
            let ext = file.extension().unwrap_or_default();

            let prefix = stem.to_string_lossy();
            let ext = ext.to_string_lossy();

            let timestamp = t.as_secs();

            let path = format!("{}_{}.{}", prefix, timestamp, ext);

            dir_path.join(path)
        }
        Err(_) => default,
    };

    // remove old backups only if write succeeds
    match fs::write(&backup, content) {
        Ok(()) => {
            println!("Wrote backup to {:?}", backup);
            for path in stage_for_removal {
                eprintln!("removing");
                match fs::remove_file(path) {
                    Ok(()) => (),
                    Err(e) => eprintln!("{e}"),
                }
            }
        }
        Err(e) => return Err(e),
    }

    Ok(())
}


// should set up a versioning system, 
// writing to /home/user/.csview/{filename}/{filename}{timestamp}
// upon load of the file, and pruning this directory 
// when its size exceeds 10 (say)
// (should also consider a .csview/config file)
pub fn write_to_file(mut cells: Cells, filename: String, delim: char) {
    let mut sheet = String::new();
    for i in 1..cells.num_rows {
        let mut row = String::new();
        let cur = cells.get_row(i);
        for j in 0..cur.len() {
            let mut content = String::new();
            let cell = cur.get(j).unwrap();
            for line in &cell.content {
                if line.contains(delim) {
                    content.push_str(&format!("\"{}\"", line));
                } else {
                    content.push_str(&line);
                }
            }
            row.push_str(&content);
            if j != cur.len() - 1 {
                row.push(delim);
            }
        }

        if i != cells.num_rows - 1 {
            row.push('\n');
        }

        sheet.push_str(&row);
    }
    
    match fs::write(&filename, sheet) {
        Ok(()) => println!("Wrote {} to file", filename),
        Err(e) => eprintln!("{e}"),
    }
}
