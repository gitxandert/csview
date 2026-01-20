use std::io::{self, Error, ErrorKind, Read, Write, stdout};

use crate::terminal::{WinChange, WinInfo};

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
}

#[derive(Debug)]
pub struct Column {
    pub id: Cell,
    pub header: Cell,
    pub cells: Vec<Cell>,
    pub start: usize, // terminal col where this begins
    pub width: usize,
}

impl Column {
    pub fn new() -> Self {
        Self {
            id: Cell::new(""),
            header: Cell::new(""),
            cells: Vec::<Cell>::new(),
            start: 0usize,
            width: 12usize,
        }
    }

    pub fn push_cell(&mut self, cell: Cell) {
        self.cells.push(cell);
    }

    pub fn set_col_id(&mut self, id: &str) {
        self.id = Cell::new(id);
    }

    pub fn set_header(&mut self, header: &str) {
        self.header = Cell::new(header);
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
                              // (not for use with row_idx)
    }

    pub fn get_cell(&mut self, idx: usize) -> &mut Cell {
        self.cells.get_mut(idx).unwrap()
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }
}

pub struct Cells {
    pub header: Vec<Cell>,
    pub col_idx: Vec<Cell>,
    pub row_idx: Column,
    pub columns: Vec<Column>,
    pub w_cell: (usize, usize),
    pub changed: bool,
    pub written: bool,
}

impl Cells {
    pub fn new(header: &Vec<Cell>, col_idx: &Vec<Cell>, row_idx: Column, num_cols: usize) -> Self {
        let columns = Vec::<Column>::with_capacity(num_cols);
        let w_cell = (0usize, 0usize);
        let changed = false;
        let written = false;

        Self { 
            header: Self::clone_cell_row(header), 
            col_idx: Self::clone_cell_row(col_idx), 
            row_idx, 
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
        column.set_width(width);
        self.changed = true;
    }

    pub fn set_w_cell(&mut self, col: usize, row: usize) {
        // first unfocus the previous w_cell
        if col < self.num_cols() && row < self.num_rows() {
            let mut w_cell = self.w_cell();
            w_cell.set_focused(false);
        
            self.w_cell = (col, row);
            w_cell = self.w_cell();
            w_cell.set_focused(true);
        }
    }

    pub fn push_column(&mut self, col: Column) {
        self.columns.push(col);
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
        self.row_idx.len()
    }

    pub fn w_cell(&mut self) -> &mut Cell {
        let mut col = self.columns.get_mut(self.w_cell.0).unwrap();
        col.get_cell(self.w_cell.1)
    }
}

pub fn show_csv(cells: &mut Cells, w_info: &mut WinInfo) {
    match w_info.changed {
        WinChange::Cell => {
            // redraw the focused cell,
            // with changed content,
            // plus the rest of the line
            //
            // 1 line
            w_info.draw_screen(cells);
            w_info.flush();
        }
        WinChange::Focus => {
            // redraw the last focused cell,
            // w/o highlighting,
            // plus the rest of its line,
            // and the new focused cell, 
            // w/ highlighting,
            // plus the rest of its line
            //
            // 1 or 2 lines
            //
            w_info.draw_focus(cells);
            w_info.flush();
        }
        WinChange::ColWidth => {
            // redraw the column whose width has changed,
            // plus all columns after
            //
            // all lines
        }
        WinChange::Rows => {
            // shift rows and row_idx,
            // but no need to redraw header and col_idx
            //
            // all lines, except for header and col_idx
            w_info.draw_screen(cells);
            w_info.flush();
        }
        WinChange::Columns => {
            // shift columns and col_idx,
            // but no need to redraw row_idx
            //
            // all lines, but not the row_idx column
            w_info.draw_screen(cells);
            w_info.flush();
        }
        WinChange::Screen => { // redraw everything on resize (unavoidable)
            w_info.draw_screen(cells);
            w_info.flush();
        }
        WinChange::Init => { // first draw sets w_cell
            let mut w_cell = cells.get_column(0).get_cell(0);
            w_cell.set_focused(true);

            w_info.draw_screen(cells);
            w_info.flush();
        }
        WinChange::Non => {
            // do nothing
        }
    }
}
