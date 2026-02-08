use std::io::{self, Error, ErrorKind, Read, Write, stdout};

use crate::{
    csv_io::int_to_base_26,
    cmd_err::{self, CmdErr},
    terminal::{WinChange, WinInfo}
};

#[derive(Debug)]
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
    }

    pub fn set_start(&mut self, c: char, i: usize) {
        self.push_seq(c);
        self.start = i;
    }
}

#[derive(Debug)]
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

        // len keeps track of "real" len
        // (i.e. the characters, not the formatting)
        let mut real_len = 0usize;

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
                    // skip new lines
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
        self.content.len()
    }

    pub fn set_content(&mut self, input: String) {
        self.content = input;
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.is_focused = focused;
    }

    // directly set it
    pub fn set_text_offset(&mut self, val: usize) {
        self.text_offset = val.min(self.len().saturating_sub(self.width));
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

    pub fn set_width(&mut self, w: usize) {
        self.width = w;
    }
}

#[derive(Debug)]
pub struct Column {
    pub cells: Vec<Cell>,
    pub start: usize, // terminal col where this begins
    pub width: usize,
    pub indices: Vec<usize>,
}

impl Column {
    pub fn new() -> Self {
        Self {
            cells: Vec::<Cell>::new(),
            start: 0usize,
            width: 12usize,
            indices: Vec::<usize>::new(),
        }
    }

    pub fn push_cell(&mut self, cell: Cell) {
        self.cells.push(cell);
        self.indices.push(self.indices.len());
    }

    pub fn insert_cell(&mut self, idx: usize, cell: Cell) {
        let mut real_idx = idx;
        if idx < self.indices.len() {
            real_idx = self.indices[idx];
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

    pub fn remove_cell(&mut self, idx: usize) {
        let real_idx = self.indices[idx];
        self.cells.remove(real_idx);
        for i in 0..idx {
            self.indices[i] -= (self.indices[i] >= real_idx) as usize;
        }
        for i in ((idx + 1)..self.indices.len()).rev() {
            self.indices[i - 1] = self.indices[i];
            self.indices[i - 1] -= (self.indices[i - 1] >= real_idx) as usize;
        }
        self.indices.pop();
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
        self.cells.get(idx).unwrap().view()
    }

    pub fn get_cell(&mut self, idx: usize) -> &mut Cell {
        let real_idx = self.indices[idx];
        self.cells.get_mut(real_idx).unwrap()
    }

    pub fn len(&self) -> usize {
        self.cells.len()
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
}

impl Cells {
    pub fn new(filename: String, delim: char, header: Vec<Cell>, col_ids: Vec<Cell>, num_cols: usize) -> Self {
        let columns = Vec::<Column>::with_capacity(num_cols);
        let w_cell = (0usize, 0usize);
        let changed = false;
        let written = false;

        Self { 
            filename,
            delim,
            header, 
            col_ids, 
            columns, 
            w_cell, 
            changed, 
            written 
        }
    }

    pub fn clone_cell_row(row: &Vec<Cell>) -> Vec<Cell> {
        let mut new_row = Vec::<Cell>::new();
        for cell in row {
            new_row.push(cell.clone());
        }

        new_row
    }

    pub fn changed(&mut self) -> bool {
        let ret = self.changed;
        self.changed ^= ret;

        ret
    }

    pub fn set_column_width(&mut self, w: usize) {
        let width = (3 > w) as usize * 3 + (w > 3) as usize * w;
        let idx = self.w_cell.0; // w_cell is where the focus is
        let mut column = self.columns.get_mut(idx).unwrap();
        let mut col_id = self.col_ids.get_mut(idx).unwrap();
        let mut col_name = self.header.get_mut(idx).unwrap();
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
        let mut col = self.columns.get_mut(self.w_cell.0).unwrap();
        col.get_cell(self.w_cell.1)
    }

    pub fn increment_col_ids(&mut self) {
        let len = self.col_ids.len() as u32; 
        
        self.col_ids.push(Cell::new(&int_to_base_26(len)));
    }

    pub fn get_col_idx<'a>(&self, id: &'a str) -> Result<usize, CmdErr<'a>> {
        if &id[..1] == "'" || &id[..1] == "\"" {
            let name = &id[1..id.len() - 1];
            for i in 0..self.header.len() {
                if &self.header[i].content == name {
                    return Ok(i);
                }
            }
            return Err(CmdErr::NoName(name));
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
    
    pub fn save_context(&mut self, wi: &WinInfo) {
        self.contexts[self.handle].save(wi)
    }

    pub fn get_context(&mut self) -> &mut Context {
        &mut self.contexts[self.handle]
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
    
