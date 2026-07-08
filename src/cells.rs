use crate::{
    csv_io::int_to_base_26,
    cmd_err::CmdErr,
    terminal::WinInfo,
};

#[derive(Clone, Debug)]
pub struct EscSeq {
    pub seq: String,
    pub start: usize,
    pub len: usize,
}

impl EscSeq {
    pub fn new() -> Self {
        Self {
            seq: String::new(),
            start: 0usize,
            len: 0usize,
        }
    }

    pub fn clone(&self) -> Self {
        Self {
            seq: self.seq.clone(),
            start: self.start,
            len: self.len,
        }
    }

    pub fn push_seq(&mut self, c: char) {
        self.seq.push(c);
        self.len = self.seq.len();
    }

    pub fn set_start(&mut self, c: char, i: usize) {
        self.push_seq(c);
        self.start = i;
    }
}

#[derive(Clone, Debug)]
pub struct Cell {
    pub content: String,
    pub escape_sequences: Vec<EscSeq>,
    pub width: usize,
    pub text_offset: usize,
    pub is_focused: bool,
}

impl Cell {
    pub fn new(content: &str) -> Self {
        // store escapes and their indices
        let mut esq = Vec::<EscSeq>::new();

        let mut line = String::new();
        let mut e = EscSeq::new();
        
        let mut x = false;
        let mut is_csi = false;
        let mut i = 0usize;
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
                    e.push_seq(c);
                    esq.push(e.clone());
                    e = EscSeq::new();

                    x = false;
                    is_csi = false;
                    continue;
                }
                '\n' => {
                    let mut nl = EscSeq::new();
                    nl.set_start(c, i);
                    esq.push(nl);
                    continue;
                }
                _ => {
                    match x {
                        false => {
                            line.push(c);
                            i += 1;
                        }
                        true => {
                            e.push_seq(c);
                            match is_csi {
                                true => continue,
                                false => {
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
            width: 12usize,
            text_offset: 0usize,
            is_focused: false,
        }
    }

    fn clone_escapes(&self) -> Vec<EscSeq> {
        let mut copy = Vec::<EscSeq>::new();
        for esc in &self.escape_sequences {
            copy.push(esc.clone());
        }

        copy
    }

    pub fn view(&self) -> &str {
        &self.content[..]
    }

    pub fn raw_content(&self) -> String {
        let mut raw = String::new();
        let mut escapes = self.escape_sequences.clone();
        escapes.sort_by_key(|esc| esc.start);

        let mut escape_idx = 0usize;
        for (content_idx, c) in self.content.chars().enumerate() {
            while escape_idx < escapes.len() && escapes[escape_idx].start == content_idx {
                raw.push_str(&escapes[escape_idx].seq);
                escape_idx += 1;
            }
            raw.push(c);
        }

        while escape_idx < escapes.len() {
            raw.push_str(&escapes[escape_idx].seq);
            escape_idx += 1;
        }

        raw
    }

    pub fn clone(&self) -> Self {
        Self { 
            content: self.content.clone(),
            escape_sequences: self.clone_escapes(),
            width: self.width,
            text_offset: self.text_offset,
            is_focused: self.is_focused,
        } 
    }

    pub fn len(&self) -> usize {
        self.content.chars().count()
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.is_focused = focused;
    }

    // directly set it
    pub fn set_text_offset(&mut self, val: usize) {
        self.text_offset = val.min(self.len().saturating_sub(self.width));
    }

    pub fn set_width(&mut self, w: usize) {
        self.width = w;
    }
}

#[cfg(test)]
mod tests {
    use super::Cell;

    #[test]
    fn cell_new_stores_newline_as_hidden_escape() {
        let cell = Cell::new("a\nb");

        assert_eq!(cell.content, "ab");
        assert_eq!(cell.escape_sequences.len(), 1);
        assert_eq!(cell.escape_sequences[0].seq, "\n");
        assert_eq!(cell.escape_sequences[0].start, 1);
        assert_eq!(cell.escape_sequences[0].len, 1);
    }

    #[test]
    fn raw_content_reinserts_hidden_escapes() {
        let cell = Cell::new("a\nb\x1b[31mc");

        assert_eq!(cell.content, "abc");
        assert_eq!(cell.raw_content(), "a\nb\x1b[31mc");
    }
}

#[derive(Debug)]
pub struct Column {
    pub cells: Vec<Cell>,
    pub start: usize, // terminal col where this begins
    pub width: usize,
    pub indices: Vec<usize>,
    pub padding: usize,
}

impl Column {
    pub fn new() -> Self {
        let mut cells = Vec::<Cell>::with_capacity(256);
        for _ in 0..256 {
            cells.push(Cell::new(""));
        }
        let indices = (0..256).collect::<Vec<usize>>();
        Self {
            cells,
            start: 0usize,
            width: 12usize,
            indices,
            padding: 0usize,
        }
    }

    pub fn clone(&self) -> Self {
        let mut cells = self.cells.clone();
        for cell in &mut cells {
            cell.is_focused = false;
        }
        let indices = self.indices.clone();
        let start = self.start;
        let width = self.width;
        let padding = self.padding;
        Self { cells, start, width, indices, padding }
    }

    pub fn push_cell(&mut self, cell: Cell) {
        self.cells.push(cell);
        self.indices.push(self.indices.len());
    }

    pub fn insert_cell(&mut self, idx: usize, cell: Cell) {
        if idx < self.indices.len() {
            let real_idx = self.indices[idx];
            self.cells.insert(real_idx, cell);
            self.indices.push(0usize);
            for i in ((idx + 1)..self.indices.len()).rev() {
                self.indices[i] = self.indices[i - (i > idx) as usize];
                self.indices[i] += (self.indices[i] >= real_idx) as usize;
            }
            for i in 0..idx {
                self.indices[i] += (self.indices[i] >= real_idx) as usize;
            }
        } else {
            self.cells.push(cell);
            self.indices.insert(idx, self.indices.len());
        }
    }

    pub fn remove_cell(&mut self, idx: usize) -> Cell {
        let real_idx = self.indices[idx];
        let cell = self.cells.remove(real_idx);

        for i in 0..idx {
            if self.indices[i] > real_idx {
                self.indices[i] -= 1;
            }
        }

        for i in idx + 1..self.indices.len() {
            let shifted = self.indices[i];
            self.indices[i - 1] = if shifted > real_idx {
                shifted - 1
            } else {
                shifted
            };
        }

        self.indices.pop();
        cell
    }

    pub fn reindex(&mut self) {
        let mut reindexed = Vec::<Cell>::with_capacity(self.cells.len());
       
        for i in 0..self.len() {
            reindexed.push(self.cells.get(self.indices[i]).unwrap().clone());
        }
        self.cells = reindexed;
        self.indices = (0..self.cells.len()).collect();
        self.padding = 0;
    }

    pub fn drain_cells(&mut self, range: std::ops::RangeInclusive<usize>) -> Vec<Cell> {
        let start = *range.start();
        let end = *range.end();

        self.reindex();
        
        let drained_cells = self.cells.drain(start..=end).collect();
        self.indices = (0..self.cells.len()).collect();

        drained_cells
    }

    pub fn swap_cells(&mut self, a: usize, b: usize) {
        let int = self.indices[a];
        self.indices[a] = self.indices[b];
        self.indices[b] = int;
    }

    pub fn bubble_up(&mut self, start: usize, end: usize) {
        for i in start..end {
            self.swap_cells(i, i + 1);
        }
    }

    pub fn bubble_down(&mut self, start: usize, end: usize) {
        for i in (end + 1..=start).rev() {
            self.swap_cells(i, i - 1);
        }
    }

    pub fn make_unique(&mut self) -> usize {
        let mut seen = Vec::<&str>::new();
        let mut seen_idx = Vec::<usize>::new();
        let mut u_idx = Vec::<usize>::new();
        for i in 0..self.indices.len() {
            let cur = self.indices[i];
            if &self.cells[cur].content == "" { 
                seen_idx.push(cur);
                continue; 
            }
            
            let mut s = 0;
            for _ in 0..seen.len() {
                if &self.cells[cur].content == &seen[s] {
                    break;
                }
                s += 1;
            }
            if s == seen.len() {
                seen.push(&self.cells[cur].content);
                u_idx.push(cur);
            } else {
                seen_idx.push(cur);
            }
        }
        // save num unique to return
        let uq_len = u_idx.len();
        if uq_len != self.len() - self.padding {
            let seen_len = seen_idx.len();
            self.padding += seen_len;
            
            for _ in 0..seen_len {
                u_idx.push(self.cells.len());
                self.cells.push(Cell::new(""));
            }
            u_idx.append(&mut seen_idx);
            
            self.indices = u_idx;
        }

        uq_len
    }

    pub fn revert(&mut self) {
        self.indices = (0..self.len()).collect();
        self.cells.truncate(self.cells.len() - self.padding);
        self.padding = 0;
    }

    pub fn set_start(&mut self, st: usize) {
        self.start = st;
    }

    pub fn set_width(&mut self, w: usize) {
        self.width = w;
        for cell in &mut self.cells {
            cell.width = w;
        }
    }

    pub fn col_width(&self) -> usize {
        return self.width + 3 // + 3 for formatting
    }

    pub fn view_cell(&self, idx: usize) -> &str {
        let real_idx = self.indices[idx];
        self.cells.get(real_idx).unwrap().view()
    }

    pub fn get_cell(&mut self, idx: usize) -> &mut Cell {
        let real_idx = self.indices[idx];
        self.cells.get_mut(real_idx).unwrap()
    }

    pub fn len(&self) -> usize {
        self.cells.len() - self.padding
    }
}

#[derive(Debug)]
pub struct Cells {
    pub filename: String,
    pub delim: char,
    pub header: Vec<Cell>,
    pub col_ids: Vec<Cell>,
    pub columns: Vec<Column>,
    pub w_cell: (usize, usize),
    pub changed: bool,
    pub written: bool,
    pub slices: usize,
}

impl Cells {
    pub fn new(filename: String, delim: char, header: Vec<Cell>, col_ids: Vec<Cell>, num_cols: usize) -> Self {
        let columns = Vec::<Column>::with_capacity(num_cols);
        let w_cell = (0usize, 0usize);
        let changed = false;
        let written = false;
        let slices = 0usize;

        Self { 
            filename,
            delim,
            header, 
            col_ids, 
            columns, 
            w_cell, 
            changed, 
            written,
            slices
        }
    }

    pub fn clone_cell_row(row: &Vec<Cell>) -> Vec<Cell> {
        let mut new_row = Vec::<Cell>::new();
        for cell in row {
            new_row.push(cell.clone());
        }

        new_row
    }

    pub fn set_column_width(&mut self, w: usize) {
        let width = w.max(3);
        let idx = self.w_cell.0; // w_cell is where the focus is
        let column = self.columns.get_mut(idx).unwrap();
        let col_id = self.col_ids.get_mut(idx).unwrap();
        let col_name = self.header.get_mut(idx).unwrap();
        column.set_width(width);
        col_id.set_width(width);
        col_name.set_width(width);
        self.changed = true;
    }

    pub fn set_w_cell(&mut self, col: usize, row: usize) {
        // first unfocus the previous w_cell
        let mut w_cell = self.w_cell();
        w_cell.set_focused(false);
        self.w_cell = (col, row);
        w_cell = self.w_cell();
        w_cell.set_focused(true);
    }

    pub fn push_column(&mut self, col: Column) {
        self.columns.push(col);
    }

    pub fn insert_column(&mut self, idx: usize, col: Column) {
        self.columns.insert(idx, col);
    }

    pub fn insert_col_name(&mut self, idx: usize, col_name: Cell) {
        self.header.insert(idx, col_name);
    }

    pub fn push_to_col(&mut self, col: usize, cell: Cell) {
        self.columns[col].push_cell(cell);
    }

    pub fn get_column(&mut self, idx: usize) -> &mut Column {
        self.columns.get_mut(idx).unwrap()
    }

    pub fn num_cols(&self) -> usize {
        self.columns.len()
    }

    pub fn num_rows(&self) -> usize {
        self.columns[0].len()
    }

    pub fn w_cell(&mut self) -> &mut Cell {
        let col = self.columns.get_mut(self.w_cell.0).unwrap();
        col.get_cell(self.w_cell.1)
    }

    pub fn increment_col_ids(&mut self) {
        let len = self.col_ids.len() as u32; 
        
        self.col_ids.push(Cell::new(&int_to_base_26(len)));
    }

    pub fn get_col_idx<'a>(&self, id: &'a str) -> Result<usize, CmdErr<'a>> {
        let mut id_chars = id.chars();
        let first = id_chars.nth(0).unwrap();
        if first == '\'' || first == '"' {
            let name: String = id_chars
                .take(id.len() - 2)
                .collect();
            for i in 0..self.header.len() {
                if self.header[i].content == name {
                    return Ok(i);
                }
            }
            return Err(CmdErr::NoName(id));
        } else {
            for i in 0..self.col_ids.len() {
                if &self.col_ids[i].content == id {
                    return Ok(i);
                }
            }
            return Err(CmdErr::NoId(id));
        }
    }
}

pub struct Context {
    pub id: usize,
    pub cells: Cells,
    pub w_pointer: usize,
    pub h_pointer: usize,
    pub w_offset: usize,
    pub h_offset: usize,
}

impl Context {
    pub fn new(id: usize, cells: Cells) -> Self {
        Self {
            id,
            cells,
            w_pointer: 0usize,
            h_pointer: 0usize,
            w_offset: 0usize,
            h_offset: 0usize,
        }
    }

    pub fn save(&mut self, wi: &WinInfo) {
        self.w_pointer = wi.w_pointer;
        self.h_pointer = wi.h_pointer;
        self.w_offset = wi.w_offset;
        self.h_offset = wi.h_offset;
    }

    pub fn cells(&mut self) -> &mut Cells {
        &mut self.cells
    }
}

pub struct Csvs {
    pub contexts: Vec<Context>,
    pub handle: usize,
}

impl Csvs {
    pub fn new(contexts: Vec<Context>) -> Self {
        Self {
            contexts,
            handle: 0usize,
        }
    }

    pub fn push_context(&mut self, con: Context) {
        self.contexts.push(con);
    }
    
    pub fn save_context(&mut self, wi: &WinInfo) {
        self.contexts[self.handle].save(wi)
    }

    pub fn get_context(&mut self) -> &mut Context {
        &mut self.contexts[self.handle]
    }

    pub fn remove_context(&mut self, id: usize) -> Context {
        for c in &mut self.contexts {
            c.id -= (c.id > id) as usize;
        }
        self.handle -= (self.handle > id) as usize; 

        self.contexts.remove(id)
    }

    pub fn get_cells(&mut self) -> &mut Cells {
        &mut self.contexts[self.handle].cells
    }

    pub fn num_contexts(&self) -> usize {
        self.contexts.len()
    }

    pub fn set_handle(&mut self, id: usize) {
        self.handle = id;
    }
}
    
