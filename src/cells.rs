use std::io::{self, Error, ErrorKind, Read, Write, stdout};

use crate::{
    csv_io::int_to_base_26,
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
        for i in idx..self.indices.len() - 1 {
            self.indices[i] = self.indices[i + 1];
            self.indices[i] -= (self.indices[i] >= real_idx) as usize;
        }
        self.indices.pop();
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

    pub fn get_col_idx(&self, name: &str) -> Option<usize> {
        for i in 0..self.header.len() {
            if &self.header[i].content == name {
                return Some(i);
            }
        }
        None
    }
}
