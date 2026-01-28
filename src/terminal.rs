use std::{
    mem, 
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering}
};
use libc::{
    self, c_int, ioctl, winsize, STDOUT_FILENO, TIOCGWINSZ,
    termios, tcgetattr, tcsetattr, cfmakeraw, TCSANOW,
};

use crate::{
    cmd_err::{self, CmdErr},
    cells::{Cell, Cells, Column},
};

#[derive(Debug, PartialEq)]
pub enum WinChange {
    Cell,       // one cell's content has changed
    Focus,      //// the focus has changed
    ColWidth,   ////// a single column's width has changed
    Rows,       //// the view of rows has shifted
    Columns,    // the view of columns has shifted
    Screen,     //// the screen's dimensions have changed
    Init,       ////// first draw; draws everything
    Write,      //// write contents of WriteBuf + row
    Command,    // write currently-typed command at bottom
    Non,        //// no change has occurred
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScrollMode {
    Text, // scroll through text within a cell
    Cell, //// change focus from cell to cell
    Axis, ////// shift all rows/columns
    Page, //////// replace all rows/columns with
          //////// the next screenful of rows/columns
}

#[derive(Debug, PartialEq)]
pub enum InputMode {
    Scroll,     // input translates to scrolling
    Write,      // input affects write buffer
    Command,    // input is processed as commands
}

pub struct Cursor {
    line: usize,
    col: usize,
    offset: usize,
    limit: usize,
}

impl Cursor {
    pub fn new() -> Self {
        Self {
            line: 0usize,
            col: 0usize,
            offset: 0usize,
            limit: 0usize,
        }
    }
}

#[derive(Debug)]
struct WriteBuf {
    data: Vec<char>,
    capacity: usize,
    gap_start: usize,
    gap_len: usize,
    offset: usize,
    content_len: usize,
    window: usize,
}

impl WriteBuf {
    pub fn new(capacity: usize) -> Self {
        Self {
            data: vec![' '; capacity],
            capacity: capacity,
            gap_start: 0,
            gap_len: capacity,
            offset: 0usize,
            content_len: 0usize,
            window: 12usize,
        }
    }

    pub fn reset(&mut self) {
        for i in 0..self.gap_start {
            self.data[i] = ' ';
        }
        let post_gap = self.gap_start + self.gap_len;
        for i in post_gap..self.data.len() {
            self.data[i] = ' ';
        }
        self.gap_len = self.capacity;
        self.gap_start = 0usize;
        self.offset = 0usize;
        self.content_len = 0usize;
        self.window = 12usize;
    }

    // moves with cursor
    pub fn move_gap(&mut self, pos: usize) {
        while pos < self.gap_start {
            self.gap_start -= 1;
            self.data[self.gap_start + self.gap_len] = self.data[self.gap_start];
        }
        while pos > self.gap_start {
            self.data[self.gap_start] = self.data[self.gap_start + self.gap_len];
            self.gap_start += 1;
        }
    }

    pub fn insert(&mut self, c: char) {
        if self.gap_len == 0 { self.grow(); }

        self.data[self.gap_start] = c;
        self.gap_start += 1;
        self.content_len += 1;
        self.gap_len = self.gap_len.saturating_sub(1);
    }

    pub fn delete(&mut self) {
        if self.gap_start == 0 { return; }

        self.gap_start = self.gap_start.saturating_sub(1);
        self.content_len = self.content_len.saturating_sub(1);
        self.gap_len += 1;
    }

    pub fn as_string(&self) -> String {
        let mut contents = String::new();
        for i in 0..self.gap_start {
            contents.push(self.data[i]);
        }
        let post_gap = self.gap_start + self.gap_len;
        for i in post_gap..self.data.len() {
            contents.push(self.data[i]);
        }

        contents
    }

    fn grow(&mut self) {
        let old_cap = self.data.len();
        let new_cap = old_cap * 2;

        let mut new_data = vec![' '; new_cap];

        for i in 0..self.gap_start {
            new_data[i] = self.data[i];
        }

        let new_gap_len = old_cap.saturating_sub(
            self.gap_start + self.gap_len
        );
        let new_gap_start = new_cap.saturating_sub(
            new_gap_len
        );

        for i in 0..new_gap_len {
            new_data[new_gap_start + i] = self.data[self.gap_start + self.gap_len + i];
        }

        self.data = new_data;
        self.capacity = new_cap;
        self.gap_len = new_cap.saturating_sub(
            self.gap_start.saturating_sub(new_gap_len)
        );
    }
}

pub struct WinInfo {
    pub width: usize,
    pub height: usize,
    pub old_width: usize,
    pub old_height: usize,
    pub w_offset: usize,
    pub h_offset: usize,
    pub w_pointer: usize,
    pub h_pointer: usize,
    pub w_page: usize,
    pub h_page: usize,
    pub changed: WinChange,
    pub scroll_mode: ScrollMode,
    pub input_mode: InputMode,
    num_cols: usize,
    num_rows: usize,
    frame: String,
    focused_content: String,
    cursor: Cursor,
    write_buffer: WriteBuf,
}

impl WinInfo {
    pub fn new(num_cols: usize, num_rows: usize) -> Self {
        let (width, height) = {
            unsafe {
                let mut w = 0usize;
                let mut h = 0usize;
                let mut ws: winsize = mem::zeroed();
                if ioctl(STDOUT_FILENO, TIOCGWINSZ.into(), &mut ws) == 0 {
                    w = ws.ws_col as usize;
                    h = ws.ws_row as usize;
                }
                (w, h)
            }
        };

        Self {
            width: width,
            height: height,
            old_width: 0usize,
            old_height: 0usize,
            w_offset: 0usize,
            h_offset: 0usize,
            num_cols: num_cols,
            num_rows: num_rows,
            focused_content: String::new(),
            frame: String::new(),
            changed: WinChange::Init,
            scroll_mode: ScrollMode::Cell,
            input_mode: InputMode::Scroll,
            w_pointer: 0usize,
            h_pointer: 0usize,
            w_page: 0usize,
            h_page: 0usize,
            cursor: Cursor::new(),
            write_buffer: WriteBuf::new(1024),
        }
    }

    // set w_page and h_page whenever screen is redrawn
    pub fn set_w_page(&mut self, end: usize, beg: usize) {
        self.w_page = end.saturating_sub(beg);
    }
    pub fn set_h_page(&mut self, end: usize, beg: usize) {
        self.h_page = end.saturating_sub(beg);
    }

    pub fn set_write_mode(&mut self, w: bool) {
        let mut out = std::io::stdout();
        match w {
            true => {
                self.input_mode = InputMode::Write;
                write!(out, "\x1b[?25h");
                self.cursor.offset = 0usize;
            }
            false => {
                self.input_mode = InputMode::Scroll;
                write!(out, "\x1b[?25l");
            }
        }
        out.flush().unwrap();
    }

    pub fn set_command_mode(&mut self, b: bool) {
        match b {
            true => {
                unsafe {
                    IN_COMMAND.store(true, Ordering::SeqCst);
                }
                self.input_mode = InputMode::Command;
                // set cursor to a space after a colon at the bottom of the screen
                self.set_cursor(self.height, 2, self.width.saturating_sub(1));
                self.cursor.offset = 0usize;
                self.write_buffer.reset();
                self.push_str_to_frame(
                    &format!(
                        "\x1b[{};1H\x1b[2K:\x1b[{};{}H\x1b[?25h",
                        self.height, self.cursor.line, self.cursor.col
                    )
                );
            }
            false => {
                unsafe {
                    IN_COMMAND.store(false, Ordering::SeqCst);
                }
                self.input_mode = InputMode::Scroll;
                self.push_str_to_frame("\x1b[?25l");
                self.draw_focused_content();
            }
        }
        self.flush();
    }

    pub fn set_write_buffer_w_cell(&mut self, cell: &Cell) {
        let buf = &mut self.write_buffer;
        buf.reset();
    
        for c in cell.content.chars() {
            buf.insert(c);
        }
       
        buf.offset = cell.text_offset;
        buf.move_gap(buf.offset);
        buf.window = cell.width;
    }

    pub fn write_to_cell(&mut self, cell: &mut Cell) {
        let buf = &mut self.write_buffer;
        cell.content = String::new();
        for i in 0..buf.gap_start {
            cell.content.push(buf.data[i]);
        }
        for i in (buf.gap_start + buf.gap_len)..buf.data.len() {
            cell.content.push(buf.data[i]);
        }
        cell.text_offset = buf.offset;
        self.changed = WinChange::Cell;
    }

    pub fn set_cursor(&mut self, line: usize, col: usize, limit: usize) {
        self.cursor.line = line;
        self.cursor.col = col;
        self.cursor.limit = limit;
    }

    pub fn move_cursor_right(&mut self) {
        let cursor = &mut self.cursor;
        let buf = &mut self.write_buffer;

        let new_cur_off = cursor.offset + 1;
        if new_cur_off > cursor.limit {
            let new_buf_off = buf.offset + 1;
            let diff = buf.content_len.saturating_sub(new_buf_off);
            if diff > buf.window {
                buf.offset = new_buf_off;
                buf.move_gap(buf.gap_start + 1);
            }
        } else {
            if buf.gap_start < buf.content_len {
                cursor.offset = new_cur_off;
                buf.move_gap(buf.gap_start + 1);
            }
        }
    }

    pub fn move_cursor_left(&mut self) {
        let cursor = &mut self.cursor;
        let buf = &mut self.write_buffer;

        if cursor.offset == 0 {
            if buf.offset > 0 {
                buf.offset = buf.offset - 1;
                buf.move_gap(buf.gap_start.saturating_sub(1));
            }
        } else {
            cursor.offset = cursor.offset - 1;
            buf.move_gap(buf.gap_start.saturating_sub(1));
        }
    }

    fn cursor_pos(&self) -> (usize, usize) {
        (self.cursor.line, self.cursor.col + self.cursor.offset)
    }

    pub fn set_w_h(&mut self) {
        unsafe {
            let mut ws: winsize = mem::zeroed();
            if ioctl(STDOUT_FILENO, TIOCGWINSZ.into(), &mut ws) == 0 {
                self.old_width = self.width;
                self.old_height = self.height;
                self.width = ws.ws_col as usize;
                self.height = ws.ws_row as usize;
            }
            self.changed = WinChange::Screen;
        }
    }
    
    pub fn set_w_pointer(&mut self, w: usize) {
        if w >= 0 && w < self.num_cols {
            let old_w = self.w_pointer;
            self.w_pointer = w;
            self.changed = WinChange::Focus;

            // change w_offset if w_pointer has gone out of view
            if self.w_pointer < self.w_offset {
                self.w_offset = self.w_offset.saturating_sub(
                    old_w.saturating_sub(self.w_pointer)
                );
                self.changed = WinChange::Columns;
            } else if self.w_pointer >= self.w_offset + self.w_page {
                let diff = self.w_pointer.saturating_sub(old_w);
                self.w_offset = (self.w_offset + diff).min(
                    self.num_cols.saturating_sub(self.w_page)
                );
                self.changed = WinChange::Columns;
            }
        } else if w >= self.num_cols {
            if self.w_pointer != self.num_cols.saturating_sub(1) {
                self.w_pointer = self.num_cols.saturating_sub(1);
                self.w_offset = self.num_cols.saturating_sub(self.w_page);
                self.changed = WinChange::Focus;
            }
        }
    }

    pub fn set_w_offset(&mut self, w: usize) {
        if w >= 0 && w < self.num_cols {
            let old_w = self.w_offset;
            self.w_offset = w.min(self.num_cols.saturating_sub(self.w_page));

            // keep w_pointer at same relative spot it was before
            if old_w > self.w_offset {
                let diff = old_w.saturating_sub(self.w_offset);
                self.w_pointer = self.w_pointer.saturating_sub(diff);
                self.changed = WinChange::Columns;
            } else if old_w < self.w_offset {
                let diff = self.w_offset.saturating_sub(old_w);
                self.w_pointer += diff;
                self.changed = WinChange::Columns;
            }
        }
    }
    
    pub fn set_h_pointer(&mut self, h: usize) {
        if h >= 0 && h < self.num_rows {
            let old_h = self.h_pointer;
            self.h_pointer = h;
            self.changed = WinChange::Focus;
            // change h_offset if h_pointer has gone out of view
            if self.h_pointer < self.h_offset {
                self.h_offset = self.h_offset.saturating_sub(
                    old_h.saturating_sub(self.h_pointer)
                );
                self.changed = WinChange::Rows;
            } else if self.h_pointer >= self.h_offset + self.h_page {
                let diff = self.h_pointer.saturating_sub(old_h);
                self.h_offset = (self.h_offset + diff).min(
                    self.num_rows.saturating_sub(self.h_page)
                );
                self.changed = WinChange::Rows;
            }
        } else {
            if self.h_pointer != self.num_rows.saturating_sub(1) {
                self.h_pointer = self.num_rows.saturating_sub(1);
                self.h_offset = self.num_rows.saturating_sub(self.h_page);
                self.changed = WinChange::Rows;
            }
        }
    }

    pub fn set_h_offset(&mut self, h: usize) {
        if h >= 0 && h <= self.num_rows.saturating_sub(self.h_page + 1) {
            let old_h = self.h_offset;
            self.h_offset = h;

            // keep h_pointer at same relative spot it was before
            if old_h > self.h_offset {
                let diff = old_h.saturating_sub(self.h_offset);
                self.h_pointer = self.h_pointer.saturating_sub(diff);
                self.changed = WinChange::Rows;
            } else if self.h_offset > old_h {
                let diff = self.h_offset.saturating_sub(old_h);
                self.h_pointer += diff;
                self.changed = WinChange::Rows;
            }
        }
    }

    pub fn set_focused(&mut self, focused: &str) {
        self.focused_content.clear();
        let take = focused.len().min(self.width);
        self.focused_content.push_str(&focused[..take]);
    }

    pub fn add_to_write_buffer(&mut self, content: &str) {
        for c in content.chars() {
            self.write_buffer.insert(c);
            
            let cursor = &mut self.cursor;
            let buf = &mut self.write_buffer;
          
            let new_cur_off = cursor.offset + 1;
            if new_cur_off > cursor.limit {
                let new_buf_off = buf.offset + 1;
                let diff = buf.content_len.saturating_sub(new_buf_off);
                if diff >= buf.window {
                    buf.offset = new_buf_off;
                }
            } else {
                if buf.gap_start <= buf.content_len {
                    cursor.offset = new_cur_off;
                }
            }
        }
        self.changed = WinChange::Write;
    }

    pub fn delete_from_write_buffer(&mut self) {
        self.write_buffer.delete();

        let cursor = &mut self.cursor;
        let buf = &mut self.write_buffer;

        if buf.offset > 0 {
            buf.offset -= 1;
        } else {
            if cursor.offset > 0 {
                cursor.offset -= 1;
            }
        }
    }

    pub fn draw_focused_content(&mut self) {
        self.push_str_to_frame(
            &format!(
                "\x1b[{};1H\x1b[2K\x1b[0m{}",
                self.height, self.focused_content
            )
        );
    }
    
    fn print_col_ids(&mut self, cells: &mut Cells, mut i: usize, mut start: usize) {
        let col_ids = &cells.col_ids;
        let mut col_id = &col_ids[i];
        let mut col_width = col_id.width + 3; // + 3 for formatting
        while start + col_width < self.width {
            let content = &col_id.content;
            let ws = (col_id.width / 2).saturating_sub(1);
            let with_ws = format!(
                "{:<ws$}{}{:<ws$}",
                " ", content, " ", ws = ws
            );
            let positioned = format!(
                " {:<width$} |", with_ws, width = col_id.width
            );
            self.push_str_to_frame(&positioned);
            start += col_width;
            i += 1;
            if i < self.num_cols {
                col_id = &col_ids[i];
                col_width = col_id.width + 3;
            } else {
                break;
            }
        }
    }

    fn print_header(&mut self, cells: &mut Cells, mut i: usize, mut start: usize) {
        let header = &cells.header;
        let mut col_name = &header[i];
        let mut col_width = col_name.width + 3; // + 3 for formatting
        while start + col_width < self.width {
            let take = col_name.text_offset + col_name.width.min(col_name.len());
            let content = &col_name.content[col_name.text_offset..take];
            let positioned = format!(
                "\x1b[30;47m {:<width$} |\x1b[39;49m", 
                content, width = col_name.width
            );
            self.push_str_to_frame(&positioned);

            start += col_width;
            i += 1;
            if i < self.num_cols {
                col_name = &header[i];
                col_width = col_name.width + 3;
            } else {
                break;
            }
        }
    }

    fn print_row(&mut self, cells: &mut Cells, i: &mut usize, row: usize, mut start: usize) {
        let mut col = &mut cells.columns[*i];
        let mut width = col.col_width();

        while start + width < self.width {
            let mut cell = col.get_cell(row);
            let take = cell.text_offset + cell.width.min(cell.len() - cell.text_offset);
            let content = &cell.content;
            let visible = &content[cell.text_offset..take];
            let formatted = {
                if cell.is_focused {
                    self.set_focused(&content);
                    self.set_cursor(
                        row + 3, start + 1, cell.width
                    );
                    format!("\x1b[7;36;47m {:<width$} \x1b[27;39;49m|",
                        visible, width = width - 3
                    )
                } else {
                    format!(" {:<width$} |", visible, width = width - 3)
                }
            };
            self.push_str_to_frame(&formatted);

            col.set_start(start);
            start += width;
            *i += 1;
            if *i < self.num_cols {
                col = &mut cells.columns[*i];
                width = col.col_width();
            } else {
                break;
            }
        }
    }

    pub fn draw_screen(&mut self, cells: &mut Cells) {
        // reset focused cell
        cells.set_w_cell(self.w_pointer, self.h_pointer);

        let mut id = self.w_offset;
        for i in 1..=self.height {
            let beg = format!("\x1b[{i};1H\x1b[2K");
            self.push_str_to_frame(&beg);

            if i == 1 {
                self.push_str_to_frame("\x1b[4m    |");
                let mut start = 6usize;
                self.print_col_ids(cells, id, start);
            } else if i == 2 {
                self.push_str_to_frame("\x1b[1;30;47mHEAD|\x1b[22;39;49m");
                let mut start = 6usize;
                id = self.w_offset;
                self.print_header(cells, id, start);
            } else {
                let row_id = i.saturating_sub(3) + self.h_offset;
                if row_id >= self.num_rows {
                    break;
                }

                let row_idx = &cells.row_idx.get_cell(row_id).content;
                let row_num = format!(
                    "\x1b[30;47m{row_idx} \x1b[39;49m"
                );
                self.push_str_to_frame(&row_num);

                let start = 6usize;
                id = self.w_offset;
                self.print_row(cells, &mut id, row_id, start);
            }
        }

        self.set_w_page(
            id, self.w_offset
        );
        self.set_h_page(
            self.height.saturating_sub(3) + self.h_offset,
            self.h_offset
        );
    }

    pub fn draw_from_column(&mut self, cells: &mut Cells) {
        let (col_id, _) = cells.w_cell;
        let start = cells.columns[col_id].start;

        let mut c = col_id;
        for row in 1..=self.height {
            let cursor = format!("\x1b[{};{}H\x1b[K\x1b[4m",
                row, start
            );
            self.push_str_to_frame(&cursor);

            match row {
                1 => self.print_col_ids(cells, col_id, start),
                2 => self.print_header(cells, col_id, start),
                _ => {
                    c = col_id;
                    self.print_row(
                        cells, 
                        &mut c, 
                        row.saturating_sub(3) + self.h_offset,
                        start
                    );
                }
            }
        }
        self.set_w_page(c, self.w_offset);
    }

    pub fn draw_focus(&mut self, cells: &mut Cells) {
        // erase the previously-focused cell
        // and whatever follows it
        let (wc_c, wc_l) = cells.w_cell;
        cells.set_w_cell(self.w_pointer, self.h_pointer);

        let prev_start = cells.columns[wc_c].start;
        let cur_start = cells.columns[self.w_pointer].start;
        let mut start = prev_start.min(cur_start);
        let prev_l = 3 + wc_l.saturating_sub(self.h_offset); 
        let beg = format!("\x1b[{};{}H\x1b[K\x1b[4m", prev_l, start);
        self.push_str_to_frame(&beg);

        // redraw previous row and the new row
        // if the new row is different from the previous
        let mut i = wc_c.min(self.w_pointer);
        self.print_row(cells, &mut i, wc_l, start); 
        
        if wc_l != self.h_pointer {
            let cur_l = 3 + self.h_pointer.saturating_sub(self.h_offset); 
            let beg = format!(
                "\x1b[{};{}H\x1b[K\x1b[4m", cur_l, start
            );
            self.push_str_to_frame(&beg);
            
            let mut col = self.w_pointer;
            self.print_row(cells, &mut col, self.h_pointer, start);
        }
    }

    pub fn draw_edited(&mut self, cells: &mut Cells) {
        self.focused_content.clear();

        let (mut id, row) = cells.w_cell;
        let col = &cells.columns[id];
        let cursor = format!(
            "\x1b[{};{}H\x1b[K\x1b[4;7;36;47m ",
            row.saturating_sub(self.h_offset) + 3, col.start
        );
        self.push_str_to_frame(&cursor);

        // take before gap
        let offset = self.write_buffer.offset;
        let max_take = self.write_buffer.content_len
                           .saturating_sub(offset)
                           .min(col.width);
        let take_1 = self.write_buffer.gap_start.min(
            offset + max_take
        );

        let mut c: char = ' ';
        for i in 0..take_1 {
            c = self.write_buffer.data[i];
            if i >= offset {
                self.push_to_frame(c);
            }
            self.focused_content.push(c.clone());
        }
        
        // take after gap
        let mut post_gap = self.write_buffer.gap_start + self.write_buffer.gap_len;
        let take_2 = post_gap + col.width.saturating_sub(take_1.saturating_sub(offset));
        for i in post_gap..self.write_buffer.data.len() {
            c = self.write_buffer.data[i];
            if i < take_2 {
                self.push_to_frame(c.clone());
                post_gap += 1;
            }
            if self.focused_content.len() < self.width {
                self.focused_content.push(c.clone());
            }
        }
        // pad end with whitespace
        while take_2 > post_gap {
            self.push_to_frame(' ');
            post_gap += 1;
        }
        self.push_str_to_frame(" \x1b[27;39;49m|");
        
        // only print rest of row if not editing last cell
        if id < self.w_offset + self.w_page - 1 {
            id += 1;
            let start = cells.columns[id].start;
            self.print_row(cells, &mut id, row, start); 
        }
    }

    fn push_write_buffer_to_frame(&mut self) {
        // take before gap
        let offset = self.write_buffer.offset;
        let max_take = self.write_buffer.content_len
                           .saturating_sub(offset)
                           .min(self.width);
        let take_1 = self.write_buffer.gap_start.min(
            offset + max_take
        );

        for i in 0..take_1 {
            if i >= offset {
                self.push_to_frame(self.write_buffer.data[i]);
            }
        }
        
        // take after gap
        let mut post_gap = self.write_buffer.gap_start + self.write_buffer.gap_len;
        let take_2 = post_gap + self.width.saturating_sub(take_1.saturating_sub(offset));
        for i in post_gap..self.write_buffer.data.len() {
            if i < take_2 {
                self.push_to_frame(self.write_buffer.data[i]);
                post_gap += 1;
            }
        }
    }

    pub fn draw_command(&mut self) {
        let beg = format!("\x1b[{};1H\x1b[2K\x1b[0m:", self.height);
        self.push_str_to_frame(&beg);
       
        self.push_write_buffer_to_frame();

        let (cursor_l, cursor_c) = self.cursor_pos();
        let cursor = format!("\x1b[{};{}H", cursor_l, cursor_c);
        self.push_str_to_frame(&cursor);
    }

    pub fn draw_w_cell(&mut self, cells: &mut Cells) {
        let (mut i, row) = cells.w_cell;
        let cursor = format!("\x1b[{};{}H\x1b[K\x1b[4m",
            row.saturating_sub(self.h_offset) + 3, &cells.columns[i].start
        );
        self.push_str_to_frame(&cursor);
        let start = cells.columns[i].start;
        self.print_row(cells, &mut i, row, start);
    }

    pub fn push_to_frame(&mut self, c: char) {
        self.frame.push(c);
    }

    pub fn push_str_to_frame(&mut self, content: &str) {
        self.frame.push_str(content);
    }

    pub fn flush(&mut self) {
        if self.input_mode != InputMode::Scroll {
            // show cursor
            let (l, c) = self.cursor_pos();
            let cursor = format!("\x1b[{l};{c}H");
            self.push_str_to_frame(&cursor);
        }
        
        let mut out = std::io::stdout();
        write!(out, "{}", self.frame);
        out.flush().unwrap();

        self.frame = String::new();
        self.changed = WinChange::Non;
    }

    pub fn show_csv(&mut self, cells: &mut Cells) {
        match self.changed {
            WinChange::Cell => {
                // redraw the focused cell,
                // with changed content,
                // plus the rest of the line
                //
                // 1 line
                self.draw_w_cell(cells);
                self.flush();
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
                self.draw_focus(cells);
                self.draw_focused_content();
                self.flush();
            }
            WinChange::ColWidth => {
                // redraw the column whose width has changed,
                // plus all columns after
                //
                // all lines
                self.draw_from_column(cells);
                self.draw_focused_content();
                self.flush();
            }
            WinChange::Rows => {
                // shift rows and row_idx,
                // but no need to redraw header and col_ids
                //
                // all lines, except for header and col_ids
                self.draw_screen(cells);
                self.draw_focused_content();
                self.flush();
            }
            WinChange::Columns => {
                // shift columns and col_ids,
                // but no need to redraw row_idx
                //
                // all lines, but not the row_idx column
                self.draw_screen(cells);
                self.draw_focused_content();
                self.flush();
            }
            WinChange::Screen => { // redraw everything on resize (unavoidable)
                self.draw_screen(cells);
                self.draw_focused_content();
                self.flush();
            }
            WinChange::Init => { // first draw sets w_cell
                let mut w_cell = cells.get_column(0).get_cell(0);
                w_cell.set_focused(true);

                self.draw_screen(cells);
                self.draw_focused_content();
                self.flush();
            }
            WinChange::Write => {
                // write contents of WriteBuf,
                // plus the rest of the row following w_cell
                self.draw_edited(cells);
                self.draw_focused_content();
                self.flush();
            }
            WinChange::Command => {
                // write contents of WriteBuf at bottom of screen
                self.draw_command();
                self.flush();
            }
            WinChange::Non => {
                // do nothing
            }
        }
    }

    fn tokenize(input: &str) -> Vec<&str> {
        let mut tokens = Vec::<&str>::new();
        let mut start = 0usize;
        let mut end = 0usize;
        let mut quote: Option<char> = None;
        // "safe warm babies"
        for c in input.chars() {
            match c {
                ' ' => {
                    // only push if not quoting
                    match quote {
                        None => {
                            tokens.push(&input[start..end]);
                            end += 1;
                            start = end;
                        }
                        Some(_) => end += 1,
                    }
                }
                '"' | '\'' => {
                    // if not quoting, start;
                    // if quoting, stop;
                    // increment end regardless
                    match quote {
                        Some(q) => {
                            if q == c {
                                quote = None;
                            }
                        }
                        None => {
                            quote = Some(c);
                        }
                    }
                    end += 1;
                }
                _ => end += 1,
            }
        }

        tokens.push(&input[start..end]);

        tokens
    }

    pub fn process_command(&mut self, cells: &mut Cells) {
        let input = self.write_buffer.as_string();
        let mut tokens = Self::tokenize(&input).into_iter();
        while let Some(tok) = tokens.next() {
            match tok {
                // column name
                // can show, edit, and find column names
                "cn" => {
                    match tokens.next() {
                        // `cn` by itself shows the focused column's name
                        None => self.show_column_name(cells),
                        Some(spec) => {
                            match spec {
                                "to" => {
                                    // `cn to` changes the focused column's name
                                    match tokens.next() {
                                        None => cmd_err::print(
                                                  CmdErr::MissingName,
                                                  spec, self.height),
                                        Some(name) => self.change_col_name(cells, &name),
                                    }
                                }
                                "find" => {
                                    // `cn find` moves the focus to the
                                    // column to find
                                    match tokens.next() {
                                        None => cmd_err::print(
                                                  CmdErr::MissingName,
                                                  spec, self.height),
                                        Some(name) => self.find_column(cells, &name),
                                    }
                                }
                                _ => cmd_err::print(
                                       CmdErr::UnknownSpec,
                                       spec, self.height),
                            }
                        }
                    }
                }
                // column
                // whole-column functions
                "col" => {
                    match tokens.next() {
                        None => cmd_err::print(
                                  CmdErr::MissingSubCmd,
                                  tok, self.height),
                        Some(subcmd) => {
                            match subcmd {
                                "mv" | "move"    => {
                                    match tokens.next() {
                                        None => cmd_err::print(
                                                  CmdErr::MissingLocation,
                                                  subcmd, self.height),
                                        Some(loc) => self.move_focused_column(cells, &loc),
                                    }
                                }
                                "f"  | "find"    => {
                                    match tokens.next() {
                                        None => cmd_err::print(
                                                  CmdErr::MissingValue,
                                                  subcmd, self.height),
                                        Some(val) => self.find_value_in_col(cells, &val),
                                    }
                                }
                                "rm" | "remove"  => (),
                                "n"  | "new"     => (),
                                "uq" | "unique"  => (),
                                "i"  | "isolate" => (),
                                _ => (),
                            }
                        }
                    }
                }

                _ => cmd_err::print(CmdErr::InvalidCommand, tok, self.height),
            }
        }
    }

    fn show_column_name(&mut self, cells: &mut Cells) {
        let col = &cells.header[self.w_pointer];
        self.push_str_to_frame(
            &format!("\x1b[{};1H\x1b[2K{}",
                self.height, col.content)
            );
        self.flush();
    }

    fn change_col_name(&mut self, cells: &mut Cells, new_name: &str) {
        let new_name = new_name.trim_start_matches(['\'', '"']);
        let new_name = new_name.trim_end_matches(['\'', '"']);
        cells.header[self.w_pointer].content = new_name.to_string();
        cells.written = true;

        let start = cells.columns[self.w_pointer].start;
        self.push_str_to_frame(
            &format!("\x1b[2;{}H\x1b[K\x1b[4m", start)
        );
        self.print_header(cells, 
                          self.w_pointer, 
                          start
        );
        self.draw_focused_content();
        self.flush();
    }

    fn find_column(&mut self, cells: &mut Cells, name: &str) {
        let name = name.trim_start_matches(['\'', '"']);
        let name = name.trim_end_matches(['\'', '"']);
        let header = &cells.header;
        for i in 0..header.len() {
            if &header[i].content == name {
                self.set_w_pointer(i);
                self.changed = WinChange::Columns;
                self.show_csv(cells);
                return;
            }
        }
        cmd_err::print(CmdErr::NoName, name, self.height);
    }

    fn move_focused_column(&mut self, cells: &mut Cells, loc: &str) {
        let mut idx = match loc.chars().nth(0).unwrap() {
            // column names must be quoted
            '"' | '\'' => {
                // make sure quotes match
                let last = loc.len() - 1;
                if loc.chars().nth(last).unwrap() == loc.chars().nth(0).unwrap() {
                    let name = &loc[1..(last).max(1)];
                    match cells.header.iter().position(|h| &h.content == name) {
                        Some(index) => index,
                        None => {
                            cmd_err::print(CmdErr::NoName, name, self.height);
                            return;
                        }
                    }
                } else {
                    cmd_err::print(CmdErr::UnmatchedQuote, loc, self.height);
                    return;
                }
            }
            // if unquoted, search col_ids
            _ => {
                // automatically discount loc if longer than any col_id
                let last = cells.col_ids.len() - 1;
                if loc.len() > cells.col_ids[last].content.len() {
                    cmd_err::print(CmdErr::NoId, loc, self.height);
                    return;
                } else {
                    match cells.col_ids.iter().position(|i| &i.content == loc) {
                        Some(index) => index,
                        None => {
                            cmd_err::print(CmdErr::NoId, loc, self.height);
                            return;
                        }
                    }
                }
            }
        };
        
        let f_col = cells.columns.remove(self.w_pointer);
        let f_col_name = cells.header.remove(self.w_pointer);
        // adjust col_id widths
        cells.col_ids[self.w_pointer].width = cells.columns[self.w_pointer].width;
        cells.col_ids[idx].width = f_col.width;
        // move column and header
        cells.columns.insert(idx, f_col);
        cells.header.insert(idx, f_col_name);
        cells.written = true;

        self.set_w_pointer(idx);
        self.show_csv(cells);
    }
    
    fn draw_col_find(&mut self, cells: &mut Cells, idx: usize, indices: &Vec<usize>) {
        self.set_h_pointer(indices[idx]);
        self.show_csv(cells);
        self.push_str_to_frame(
            &format!(
                "\x1b[{};1H\x1b[2K\x1b[0m{}/{}",
                self.height, idx + 1, indices.len()
            )
        );
        self.flush();
    }

    fn find_value_in_col(&mut self, cells: &mut Cells, val: &str) {
        let val = val.trim_start_matches(['\'', '"']);
        let val = val.trim_end_matches(['\'', '"']);
        let rows = &cells.columns[self.w_pointer].cells;
        let indices: Vec<usize> = rows.iter()
            .enumerate()
            .filter(|&(_idx, cell)| cell.content.contains(val))
            .map(|(idx, _cell)| idx)
            .collect();
        
        if indices.len() == 0 {
            self.push_str_to_frame(
                &format!(
                    "\x1b[{};1H\x1b[2KNo instance of '{}' in '{}'",
                    self.height, val, cells.header[self.w_pointer].content)
            );
            self.flush();
            return;
        }
        // hide cursor
        self.push_str_to_frame("\x1b[?25l");

        // - read from stdin: 'n', 'b', or 'esc'
        // - print nth/n at bottom or "No rows in {col_name} contain {val}"
        // - shift self.h_pointer with 'n' or 'b'
        // - return to normal functionality with 'esc'
        let mut idx = {
            // start with the closest example of the value
            let mut diff = rows.len();
            let mut i = 0usize;
            for _ in 0..indices.len() {
                let idx = indices[i];
                let cur_diff = self.h_pointer.max(idx) - self.h_pointer.min(idx);
                if cur_diff < diff {
                    diff = cur_diff;
                    i += 1;
                } else {
                    break;
                }
            } 

            i - 1
        };
        self.draw_col_find(cells, idx, &indices);

        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 1];
        loop {
            match stdin.read_exact(&mut buf) {
                Ok(_) => {
                    match buf {
                        [b'n'] => {
                            idx = (idx + 1) % indices.len();
                            self.draw_col_find(cells, idx, &indices);
                            buf[0] = 0u8;
                        }
                        [b'b'] => {
                            if idx == 0 {
                                idx = indices.len() - 1;
                            } else {
                                idx -= 1;
                            }
                            self.draw_col_find(cells, idx, &indices);
                            buf[0] = 0u8;
                        }
                        [17]   => { // ctrl + q (quit)
                            self.draw_focused_content();
                            self.flush();
                            break;
                        }
                        [3]   => { // ctrl + c
                            // catch to prevent close
                            eprintln!("accidentally pressed ctrl + c");
                            continue;
                        }
                        _     => (),
                    }
                }
                _ => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }
    }
}

// Terminal takeover + signal handling + globals for WIDTH and HEIGHT

static mut ORIG_TERM: Option<termios> = None;

pub fn raw_mode(switch: bool) {
    unsafe {
        let fd = libc::STDIN_FILENO;

        let mut term: termios = std::mem::zeroed();
        tcgetattr(fd, &mut term);

        let mut out = std::io::stdout();
        let mut outstring = String::new();

        match switch {
            true => {
                ORIG_TERM = Some(term);
                let mut raw = term;
                cfmakeraw(&mut raw);
                //re-enable signals
                raw.c_lflag |= libc::ISIG;
                // turn on non-blocking to catch sigs
                raw.c_cc[libc::VMIN] = 0;
                raw.c_cc[libc::VTIME] = 0;
                tcsetattr(fd, TCSANOW, &raw);
    
                // switch to Alternate Screen Buffer
                outstring.push_str("\x1b[?1049h");
                // hide cursor
                outstring.push_str("\x1b[?25l");
            }
            false => {
                if let Some(orig) = ORIG_TERM {
                    tcsetattr(fd, TCSANOW, &orig);
                    // switch back to main buffer
                    outstring.push_str("\x1b[?1049l");
                    // show cursor
                    outstring.push_str("\x1b[?25h");
                }
            }
        };
        write!(out, "{}", outstring).unwrap();
        out.flush().unwrap();
    }
}

// prevent signals from closing program when in command mode
static IN_COMMAND: AtomicBool = AtomicBool::new(false);

static GOT_WINCH: AtomicBool = AtomicBool::new(false);
static GOT_INT: AtomicBool = AtomicBool::new(false);
static GOT_QUIT: AtomicBool = AtomicBool::new(false);

extern "C" fn sig_winch(_sig: c_int) {
    GOT_WINCH.store(true, Ordering::SeqCst);
}

extern "C" fn sig_int(_sig: c_int) {
    if !IN_COMMAND.load(Ordering::Relaxed) {
        GOT_INT.store(true, Ordering::SeqCst);
    }
}

extern "C" fn sig_quit(_sig: c_int) {
    if !IN_COMMAND.load(Ordering::Relaxed) {
        GOT_QUIT.store(true, Ordering::SeqCst);
    }
}

pub fn check_flags(w_info: &mut WinInfo) -> bool {
    if GOT_WINCH.swap(false, Ordering::SeqCst) {
        w_info.set_w_h();
    }

    return GOT_INT.swap(false, Ordering::SeqCst) || 
           GOT_QUIT.swap(false, Ordering::SeqCst);
}

pub fn install_sig_handlers() {
    unsafe {
        //SIGWINCH
        let mut sa_winch: libc::sigaction = std::mem::zeroed();
        sa_winch.sa_sigaction = sig_winch as usize;
        libc::sigemptyset(&mut sa_winch.sa_mask);
        sa_winch.sa_flags = 0;
        libc::sigaction(libc::SIGWINCH, &sa_winch, std::ptr::null_mut());

        //SIGINT
        let mut sa_int: libc::sigaction = std::mem::zeroed();
        sa_int.sa_sigaction = sig_int as usize;
        libc::sigemptyset(&mut sa_int.sa_mask);
        sa_int.sa_flags = 0;
        libc::sigaction(libc::SIGINT, &sa_int, std::ptr::null_mut());
        
        //SIGQUIT
        let mut sa_quit: libc::sigaction = std::mem::zeroed();
        sa_quit.sa_sigaction = sig_quit as usize;
        libc::sigemptyset(&mut sa_quit.sa_mask);
        sa_quit.sa_flags = 0;
        libc::sigaction(libc::SIGQUIT, &sa_quit, std::ptr::null_mut());
    }
}

pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        raw_mode(false);
        let panic_info = format!("Panic: {info}");
        if let Ok(mut log) = OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/csview.log") 
        {
            log.write_all(panic_info.as_bytes());
        } else { eprintln!("couldn't open /tmp/csview.log"); }
        std::process::exit(130);
    }));
}

