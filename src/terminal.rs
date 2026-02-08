use std::{
    cmp,
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
    cells::*,
    csv_io::{poll_stdin, PollEvent},
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

#[derive(Debug, PartialEq)]
enum Sort {
    AscAlph,
    DescAlph,
    AscNum,
    DescNum,
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

// helper macro to print to bottom of screen
// (only works with WinInfo)
macro_rules! print_bottom {
    ($self:expr, $fmt:expr $(, $args:expr)*) => {
        // Here, 'self' and 'self.height' are assumed to be in scope
        // where the macro is called.
        let formatted_string = format!(
            concat!("\x1b[{};1H", "\x1b[2K", "\x1b[0m", $fmt),
            $self.height,
            $($args),*
        );
        
        $self.push_str_to_frame(&formatted_string);
    };
}

impl WinInfo {
    pub fn new() -> Self {
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
            width,
            height,
            w_offset: 0usize,
            h_offset: 0usize,
            num_cols: 0usize,
            num_rows: 0usize,
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

    pub fn set_context(&mut self, con: &Context) {
        self.num_cols = con.cells.num_cols();
        self.num_rows = con.cells.num_rows();
        self.w_pointer = con.w_pointer;
        self.h_pointer = con.h_pointer;
        self.w_offset = con.w_offset;
        self.h_offset = con.h_offset;
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
                        "\x1b[{};1H\x1b[2K\x1b[0m:\x1b[{};{}H\x1b[?25h",
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

    pub fn set_w_h(&mut self, cells: &mut Cells) {
        unsafe {
            let mut ws: winsize = mem::zeroed();
            if ioctl(STDOUT_FILENO, TIOCGWINSZ.into(), &mut ws) == 0 {
                self.width = ws.ws_col as usize;
                self.height = ws.ws_row as usize;
            }
            self.draw_screen(cells);
            self.draw_focused_content();
            self.flush();
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
                self.changed = WinChange::Columns;
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
                self.changed = WinChange::Screen;
            } else if self.h_pointer >= self.h_offset + self.h_page {
                let diff = self.h_pointer.saturating_sub(old_h);
                self.h_offset = (self.h_offset + diff).min(
                    self.num_rows.saturating_sub(self.h_page)
                );
                self.changed = WinChange::Screen;
            }
        } else {
            if self.h_pointer != self.num_rows.saturating_sub(1) {
                self.h_pointer = self.num_rows.saturating_sub(1);
                self.h_offset = self.num_rows.saturating_sub(self.h_page);
                self.changed = WinChange::Screen;
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
        print_bottom!(
            self, "{}", self.focused_content
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
                        row - self.h_offset + 3, start + 1, cell.width
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
        if self.h_pointer < cells.columns[0].len() {
            cells.set_w_cell(self.w_pointer, self.h_pointer);
        } else {
            self.h_pointer = cells.columns[0].len() - 1;
        }
        // wipe screen once
        self.push_str_to_frame("\x1b[1;1H\x1b[2J");
        
        let mut id = self.w_offset;
        for i in 1..self.height {
            // move cursor to beginning of line
            self.push_str_to_frame(&format!("\x1b[{i};1H"));

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

                let row_num = format!(
                    "\x1b[30;47m{:04X} \x1b[39;49m", row_id
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
        let col_id = self.w_pointer;
        let start = cells.columns[col_id].start;

        let mut c = col_id;
        for row in 1..self.height {
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

    pub fn draw_rows(&mut self, cells: &mut Cells) {
        let (_, prev_row) = cells.w_cell;
        cells.set_w_cell(self.w_pointer, self.h_pointer);

        let st_row = prev_row.min(self.h_pointer);
        let term_row = st_row - self.h_offset + 3;
        self.push_str_to_frame(
            &format!("\x1b[{};1H\x1b[4m", term_row)
        );

        for row in term_row..self.height {
            let row_id = row - 3 + self.h_offset;
            self.push_str_to_frame(
                &format!(
                    "\x1b[{};1H\x1b[2K\x1b[30;47m{:04X} \x1b[39;49m",
                    row, row_id
                )
            );

            let start = 6usize;
            let mut id = self.w_offset;
            self.print_row(cells, &mut id, row_id, start);
        }
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
                self.draw_focus(cells);
                self.draw_focused_content();
                self.flush();
            }
            WinChange::ColWidth => {
                // redraw the column whose width has changed,
                // plus all columns after
                self.draw_from_column(cells);
                self.draw_focused_content();
                self.flush();
            }
            WinChange::Rows => {
                // draw rows from (previous) w_cell.1
                self.draw_rows(cells);
                self.draw_focused_content();
                self.flush();
            }
            WinChange::Columns => {
                // currently, same as WinChange::Screen
                self.draw_screen(cells);
                self.draw_focused_content();
                self.flush();
            }
            WinChange::Screen => { // redraw everything on resize
                self.draw_screen(cells);
                self.draw_focused_content();
                self.flush();
            }
            WinChange::Init => { // same as screenfirst draw sets w_cei
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

    fn tokenize(input: &str, delim: char) -> Vec<&str> {
        let mut tokens = Vec::<&str>::new();
        let mut start = 0usize;
        let mut end = 0usize;
        let mut quote: Option<char> = None;
        
        for c in input.chars() {
            match c {
                ch if ch == delim => {
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
        let mut tokens = Self::tokenize(&input, ' ');
        match tokens[0] {
            // column name
            // can show, edit, and find column names
            "cn" => self.cn_cmd(
                        tokens,
                        cells
                    ),
            // column
            // whole-column functions
            "col" => self.col_cmd(
                        tokens,
                        cells
                    ),
            // row
            // row operations
            "row" => self.row_cmd(
                        tokens,
                        cells
                    ),
            // sheet
            // whole-sheet operations
            "sh" | "sheet" => self.sheet_cmd(
                                    tokens,
                                    cells
                                  ),
            // invalid
            _ => cmd_err::print(
                    CmdErr::InvalidCommand, 
                    tokens[0], 
                    self.height
                ),
        }
    }

    fn cn_cmd(&mut self, tokens: Vec<&str>, cells: &mut Cells) {
        let mut tokens = tokens.into_iter();
        let _ = tokens.next();
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
                                        spec, 
                                        self.height
                                    ),
                            Some(name) => self.change_col_name(cells, &name),
                        }
                    }
                    "f" | "find" => {
                        // `cn find` moves the focus to the
                        // column to find
                        match tokens.next() {
                            None => cmd_err::print(
                                        CmdErr::MissingName,
                                        spec, 
                                        self.height
                                    ),
                            Some(name) => self.find_column(cells, &name),
                        }
                    }
                    _ => cmd_err::print(
                            CmdErr::UnknownSpec,
                            spec, 
                            self.height
                        ),
                }
            }
        }
    }
    
    fn col_cmd(&mut self, tokens: Vec<&str>, cells: &mut Cells) {
        let mut tokens = tokens.into_iter();
        let tok = tokens.next().unwrap();
        let subcmd = match tokens.next() {
            None => {
                cmd_err::print(
                    CmdErr::MissingSubCmd,
                    tok, 
                    self.height
                );
                return;
            }
            Some(sc) => sc,
        };

        match subcmd {
            "mv" | "move"    => {
                match tokens.next() {
                    None => cmd_err::print(
                                CmdErr::MissingLocation,
                                subcmd, 
                                self.height
                            ),
                    Some(loc) => self.move_focused_column(cells, &loc),
                }
            }
            "f"  | "find"    => {
                match tokens.next() {
                    None => cmd_err::print(
                                CmdErr::MissingValue,
                                subcmd, 
                                self.height
                            ),
                    Some(val) => self.find_value_in_col(cells, &val),
                }
            }
            "n"  | "new"     => {
                match tokens.next() {
                    None => cmd_err::print(
                                CmdErr::MissingName,
                                subcmd, 
                                self.height
                            ),
                    Some(name) => self.new_column(cells, &name),
                }
            }
            "rm" | "remove"  => {
                match tokens.next() {
                    Some(_) => cmd_err::print(
                                    CmdErr::TooManyArgs,
                                    subcmd, 
                                    self.height
                                ),
                    None => self.remove_column(cells),
                }
            }
            "g"  | "group"   => {
                match tokens.next() {
                    None => cmd_err::print(
                                CmdErr::MissingList,
                                subcmd, 
                                self.height
                            ),
                    Some(list) => self.group_columns(cells, &list),
                }
            }
            "u"  | "unique"  => self.show_unique_column_values(cells),
            "s"  | "sort"    => {
                let mut s_dir = Sort::AscAlph;
                match tokens.next() {
                    None => self.sort_focused_column(cells, s_dir),
                    Some(spec) => {
                        match spec {
                            "a" => (),
                            "r" | "ar" => s_dir = Sort::DescAlph,
                            "n" => s_dir = Sort::AscNum,
                            "nr" => s_dir = Sort::DescNum,
                            _ => {
                                cmd_err::print(
                                    CmdErr::UnknownSpec,
                                    spec, 
                                    self.height
                                );
                                return;
                            }
                        }
                        self.sort_focused_column(cells, s_dir);
                    }
                }
            }
            "rv" | "revert" => self.revert_focused_column(cells),
            "fn" | "fillna" => {
                match tokens.next() {
                    None => cmd_err::print(
                                CmdErr::MissingValue,
                                subcmd, 
                                self.height
                            ),
                    Some(val) => self.col_fillna(cells, &val),
                }
            }
            "r"  | "replace" => {
                match tokens.next() {
                    None => cmd_err::print(
                                CmdErr::MissingValue,
                                subcmd, 
                                self.height
                            ),
                    Some(targ) => {
                        match tokens.next() {
                            None => cmd_err::print(
                                        CmdErr::MissingValue,
                                        subcmd, 
                                        self.height
                                    ),
                            Some(new) => self.replace_in_col(cells, &targ, &new),
                        }
                    }
                }
            }
            _ => cmd_err::print(
                    CmdErr::InvalidSubCmd,
                    subcmd, 
                    self.height
                ),
        }
    }

    fn row_cmd(&mut self, tokens: Vec<&str>, cells: &mut Cells) {
        let mut tokens = tokens.into_iter();
        let tok = tokens.next().unwrap();
        let subcmd = match tokens.next() {
            None => {
                cmd_err::print(
                    CmdErr::MissingSubCmd,
                    tok, 
                    self.height
                );
                return;
            }
            Some(sc) => sc,
        };
        
        match subcmd {
            "i"  | "insert" => self.insert_row(cells),
            "d"  | "delete" => self.delete_row(cells),
            "mv" | "move"   => {
                match tokens.next() {
                    None => cmd_err::print(
                                CmdErr::MissingRange,
                                subcmd, 
                                self.height
                            ),
                    Some(range) => {
                        match tokens.next() {
                            None => cmd_err::print(
                                        CmdErr::MissingTarget,
                                        subcmd, 
                                        self.height
                                    ),
                            Some(target) => self.move_rows(
                                            cells, 
                                            &range, 
                                            &target
                                        ),
                        }
                    }
                }
            }
            _ =>  cmd_err::print(
                    CmdErr::InvalidSubCmd,
                    tok, 
                    self.height
                ),
        }
    }
        
    fn sheet_cmd(&mut self, tokens: Vec<&str>, cells: &mut Cells) {
        let tok = tokens[0];
        let subcmd = match tokens.get(1) {
            None =>{
                cmd_err::print(
                    CmdErr::MissingSubCmd,
                    tok, 
                    self.height
                );
                return;
            }
            Some(sc) => *sc,
        };
        
        match subcmd {
            "sb" | "sortby" => {
                let mut tokens = tokens.into_iter();
                for i in 0..2 {
                    let _ = tokens.next();
                }
                match tokens.next() {
                    None => cmd_err::print(
                                CmdErr::MissingName,
                                tok,
                                self.height
                            ),
                    Some(col) => {
                        let s_dir = match tokens.next() {
                            None => Sort::AscAlph,
                            Some(dir) => {
                                match dir {
                                    "a" => Sort::AscAlph,
                                    "r" | "ar" => Sort::DescAlph,
                                    "n" => Sort::AscNum,
                                    "nr" => Sort::DescNum,
                                    _ => {
                                        cmd_err::print(
                                            CmdErr::InvalidArg,
                                            dir, 
                                            self.height
                                        );
                                        return;
                                    }
                                }
                            }
                        };
                        self.sort_by(cells, &col, s_dir);
                    }
                }
            }
            "sl" | "slice" => self.slice(tokens, cells),
            _ => cmd_err::print(
                    CmdErr::InvalidSubCmd,
                    subcmd, 
                    self.height
                ),
        }
    }

    fn trim_quotes(q: &str) -> &str {
        if &q[..1] == "'" || &q[..1] == "\"" {
            return &q[1..q.len() - 1];
        }
        q
    }

    // cn functions
    //
    fn show_column_name(&mut self, cells: &mut Cells) {
        let col = &cells.header[self.w_pointer];
        print_bottom!(
            self, "{}", col.content
        );
        self.flush();
    }

    fn change_col_name(&mut self, cells: &mut Cells, new_name: &str) {
        let new_name = Self::trim_quotes(new_name);
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
        let name = Self::trim_quotes(name);
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

    // column functions
    //
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
        self.changed = WinChange::Columns;
        self.show_csv(cells);
    }
    
    fn draw_col_find(&mut self, cells: &mut Cells, idx: usize, indices: &Vec<usize>) {
        self.set_h_pointer(indices[idx]);
        self.show_csv(cells);
        print_bottom!(
            self, 
            "{}/{}",
            idx + 1, indices.len()
        );
        self.flush();
    }

    fn find_value_in_col(&mut self, cells: &mut Cells, val: &str) {
        let val = Self::trim_quotes(val);
        let rows = &cells.columns[self.w_pointer].cells;
        let indices: Vec<usize> = rows.iter()
            .enumerate()
            .filter(|&(_idx, cell)| cell.content.contains(val))
            .map(|(idx, _cell)| idx)
            .collect();
        
        if indices.len() == 0 {
            print_bottom!(
                self,
                "No instance of '{}' in '{}'",
                val, cells.header[self.w_pointer].content
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

        let mut buf = [0u8; 1];
        loop {
            match poll_stdin(&mut buf) {
                Ok(PollEvent::Sig) => {
                    match check_flags() {
                        SigFlag::Winch => self.set_w_h(cells),
                        SigFlag::Int | SigFlag::Quit => continue, // will never catch
                        SigFlag::Non => break, // must've been quit/int
                    }
                }
                Ok(PollEvent::Data(0)) => continue,
                Ok(PollEvent::Data(n)) => {
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
                        _   => {
                            self.draw_focused_content();
                            self.flush();
                            break;
                        }
                    }
                }
                Err(e) => {
                    print_bottom!(
                        self,
                        "ERR: {}",
                        e
                    );
                    self.flush();
                }
            }
        }
    }

    fn new_column(&mut self, cells: &mut Cells, name: &str) {
        let name = Self::trim_quotes(name);;
       
        let mut w_cell = cells.w_cell();
        w_cell.is_focused = false;

        let mut new_col = Column::new();
        for i in 0..self.num_rows {
            new_col.push_cell(Cell::new(""));
        }
        cells.insert_column(self.w_pointer + 1, new_col);
        
        let col_name = Cell::new(name);
        cells.insert_col_name(self.w_pointer + 1, col_name);
        
        cells.increment_col_ids();
        // make sure each col_id's width
        // matches the width of the column below
        for i in self.w_pointer + 1..cells.col_ids.len() {
            cells.col_ids[i].width = cells.header[i].width;
        }

        cells.written = true;
        self.num_cols += 1;
        self.set_w_pointer(self.w_pointer + 1);
        self.draw_screen(cells);
        self.set_focused("");
        self.draw_focused_content();
        self.flush();
    }

    fn remove_column(&mut self, cells: &mut Cells) {
        // confirm remove
        let col_name = cells.header[self.w_pointer].content.clone();
        print_bottom!(
            self,
            "\x1b[?25lConfirm remove column '{}' with 'y': ",
            col_name
        );
        self.flush();
        let mut buffer = [0u8; 1];
        loop {
            match std::io::stdin().read(&mut buffer) {
                Ok(0) => std::thread::sleep(std::time::Duration::from_millis(10)),
                Ok(n) => {
                    match &buffer[..n] {
                        [b'y'] | [b'Y']  => {
                            cells.columns.remove(self.w_pointer);
                            cells.header.remove(self.w_pointer);
                            cells.col_ids.pop();
                            for i in self.w_pointer..cells.col_ids.len() {
                                cells.col_ids[i].width = cells.header[i].width;
                            }

                            cells.written = true;
                            self.num_cols -= 1;
                            self.draw_screen(cells);
                            
                            print_bottom!(
                                self,
                                "Removed column '{}'",
                                col_name
                            );
                            self.flush();

                            break;
                        }
                        _ => {
                            print_bottom!(
                                self,
                                "Column '{}' was not removed.",
                                col_name
                            );
                            self.flush();
                            break;
                        }
                    }
                }
                _ => {
                    cmd_err::print(CmdErr::StdinErr, "col remove", self.height);
                    break;
                }
            }
        }
    }

    fn group_columns(&mut self, cells: &mut Cells, list: &str) {
        let mut columns = Self::tokenize(list, ',').into_iter();
        // columns.next() for sure has a value here
        let p_col_name = columns.next().unwrap();
        let p_col_name = Self::trim_quotes(p_col_name);

        let mut p_col_idx = match cells.get_col_idx(p_col_name) {
            Some(idx) => idx,
            None      => {
                cmd_err::print(
                    CmdErr::NoName, p_col_name, self.height
                );
                return;
            }
        };

        // collect ids before removing the columns,
        // in case of an error
        let mut ids = Vec::<usize>::with_capacity(columns.len());

        while let Some(name) = columns.next() {
            let name = Self::trim_quotes(name);
            match cells.get_col_idx(name) {
                Some(idx) => ids.push(idx),
                None => {
                    cmd_err::print(
                        CmdErr::NoName, name, self.height
                    );
                    return;
                }
            }
        }
            
        let mut i = 1usize;
        let mut dec = 0usize;
        for idx in ids {
            let idx = {
                if idx < p_col_idx {
                    dec += 1;
                    idx - (dec - 1)
                } else {
                    idx
                }
            };
            let mut col = cells.columns.remove(idx);
            col.get_cell(self.h_pointer).set_focused(false);
            let col_name = cells.header.remove(idx);
            let col_width = col.width;
            cells.columns.insert(p_col_idx - dec + i, col);
            cells.header.insert(p_col_idx - dec + i, col_name);
            cells.col_ids[p_col_idx - dec + i].width = col_width;
            cells.col_ids[idx].width = cells.columns[idx].width;
            i += 1;
        }

        if self.w_pointer != p_col_idx {
            self.set_w_pointer(p_col_idx - dec);
            self.draw_screen(cells);
        } else {
            self.draw_from_column(cells);
        }

        self.draw_focused_content();
        self.flush();
        cells.written = true;
    }

    fn show_unique_column_values(&mut self, cells: &mut Cells) {
        let orig = &cells.columns[self.w_pointer];

        let mut uq = Column::new();
        uq.start = orig.start;
        uq.width = orig.width;

        for o_cell in &orig.cells {
            if o_cell.content == "" {
                continue;
            }
            let mut seen = false;
            for u_cell in &uq.cells {
                if u_cell.content == o_cell.content {
                    seen = true;
                    break;
                }
            }
            if !seen {
                let mut new_cell = o_cell.clone();
                new_cell.text_offset = 0usize;
                uq.push_cell(new_cell);
            }
        }

        if uq.len() == 0 {
            print_bottom!(
                self,
                "No values in '{}'",
                cells.header[self.w_pointer].content
            );
            self.flush();
            return;
        }

        // temporarily remove original column
        drop(orig);
        cells.w_cell().is_focused = false;
        let orig = cells.columns.remove(self.w_pointer);
        // insert uq at same place,
        // with values offset vertically
        for i in 0..self.h_offset {
            uq.insert_cell(i, Cell::new(""));
        }
        let uq_st = self.h_offset;
        let uq_len = uq.len() - uq_st;
        let uq_end = uq.len() - 1;
        for i in uq.len()..orig.len() {
            uq.insert_cell(i, Cell::new(""));
        }
        cells.insert_column(self.w_pointer, uq);
        cells.set_w_cell(cells.w_cell.0, cells.w_cell.1);
        self.draw_from_column(cells);
        let values = "values";
        print_bottom!(
            self,
            "\x1b[?25l{} unique {} in '{}'",
            uq_len, 
            &values[0..6-2usize.saturating_sub(uq_len)], 
            cells.header[self.w_pointer].content
        );
        self.flush();
        let mut buf = [0u8; 8];
        loop {
            match poll_stdin(&mut buf) {
                Ok(PollEvent::Sig) => {
                    match check_flags() {
                        SigFlag::Winch => self.set_w_h(cells),
                        SigFlag::Int | SigFlag::Quit => (), // will never catch this
                        SigFlag::Non => break, // must've caught quit/int
                    }
                }
                Ok(PollEvent::Data(0)) => continue,
                Ok(PollEvent::Data(n)) => {
                    match &buf[..n] {
                        [27, 91, 65] => {
                            if self.h_pointer > uq_st {
                                self.set_h_pointer(self.h_pointer.saturating_sub(1));
                                self.show_csv(cells);
                            }
                        }
                        [27, 91, 66] => {
                            if self.h_pointer < uq_end {
                                self.set_h_pointer(self.h_pointer + 1);
                                self.show_csv(cells);
                            }
                        }
                        [17] => break,
                        _   => continue,
                    }
                }
                Err(e) => {
                    print_bottom!(
                        self,
                        "ERR: {}",
                        e
                    );
                    self.flush();
                }
            }
        }
        // restore original
        cells.columns.remove(self.w_pointer);
        cells.insert_column(self.w_pointer, orig);
        cells.set_w_cell(cells.w_cell.0, cells.w_cell.1);
        self.draw_from_column(cells);
        self.draw_focused_content();
        self.flush();
    }

    fn sort_indices(col: &mut Column, dir: Sort) {
        let mut indices = col.indices.clone();
        match dir {
            Sort::AscAlph => 
                indices.sort_unstable_by(|&i, &j| {
                    let a = {
                        match col.view_cell(i) {
                            "" => &format!("{}a", col.view_cell(j)),
                            _ => col.view_cell(i),
                        }
                    };
                    let b = {
                        match col.view_cell(j) {
                            "" => &format!("{}a", col.view_cell(i)),
                            _ => col.view_cell(j),
                        }
                    };
                    a.cmp(&b)
                }),
            Sort::DescAlph => 
                indices.sort_unstable_by(|&i, &j| {
                    let a = col.view_cell(i);
                    let b = col.view_cell(j);
                    b.cmp(&a)
                }),
            Sort::AscNum => {
                indices.sort_unstable_by(|&i, &j| {
                    let a = match col.cells[i].content.parse::<f32>() {
                        Ok(num) => num,
                        Err(_)  => f32::MAX,
                    };
                    let b = match col.cells[j].content.parse::<f32>() {
                        Ok(num) => num,
                        Err(_)  => f32::MAX,
                    };
                    a.total_cmp(&b)
                })
            }
            Sort::DescNum => {
                indices.sort_unstable_by(|&i, &j| {
                    let a = match col.cells[i].content.parse::<f32>() {
                        Ok(num) => num,
                        Err(_)  => f32::MIN,
                    };
                    let b = match col.cells[j].content.parse::<f32>() {
                        Ok(num) => num,
                        Err(_)  => f32::MIN,
                    };
                    b.total_cmp(&a)
                })
            }
        }

        col.indices = indices;
    }

    fn sort_focused_column(&mut self, cells: &mut Cells, dir: Sort) {
        cells.w_cell().is_focused = false;
        let mut col = &mut cells.columns[self.w_pointer];
        
        Self::sort_indices(&mut col, dir);
    
        cells.set_w_cell(self.w_pointer, self.h_pointer);
        cells.written = true;
        self.draw_from_column(cells);
        self.draw_focused_content();
        self.flush();
    }

    fn revert_focused_column(&mut self, cells: &mut Cells) {
        cells.w_cell().is_focused = false;
        let mut col = &mut cells.columns[self.w_pointer];
        col.indices = (0..col.len()).collect();
        cells.set_w_cell(self.w_pointer, self.h_pointer);
        cells.written = true;
        self.draw_from_column(cells);
        self.draw_focused_content();
        self.flush();
    }

    fn col_fillna(&mut self, cells: &mut Cells, val: &str) {
        let val = Self::trim_quotes(val);
        let mut col = &mut cells.columns[self.w_pointer];
        for cell in &mut col.cells {
            if &cell.content == "" {
                cell.content.push_str(val);
            }
        }
        cells.written = true;
        self.draw_from_column(cells);
        self.draw_focused_content();
        self.flush();
    }

    fn replace_in_col(&mut self, cells: &mut Cells, targ: &str, new: &str) {
        let t = Self::trim_quotes(targ);
        let n = Self::trim_quotes(new);

        let mut col = &mut cells.columns[self.w_pointer];
        for cell in &mut col.cells {
            if &cell.content == t {
                cell.content = n.to_string();
            }
        }
        cells.written = true;
        self.draw_from_column(cells);
        self.draw_focused_content();
        self.flush();
    }

    // row functions
    //
    fn insert_row(&mut self, cells: &mut Cells) {
        for col in &mut cells.columns {
            col.insert_cell(self.h_pointer + 1, Cell::new(""));
        }
        cells.written = true;
        self.num_rows += 1;
        self.set_h_pointer(self.h_pointer + 1);
        if self.changed == WinChange::Focus {
            self.changed = WinChange::Rows;
        }
        self.show_csv(cells);
    }

    fn delete_row(&mut self, cells: &mut Cells) {
        print_bottom!(
            self,
            "\x1b[?25lConfirm remove row {:04X} with 'y': ",
            self.h_pointer
        );
        self.flush();
        let mut buffer = [0u8; 1];
        loop {
            match std::io::stdin().read(&mut buffer) {
                Ok(0) => std::thread::sleep(std::time::Duration::from_millis(10)),
                Ok(n) => {
                    match &buffer[..n] {
                        [b'y'] | [b'Y']  => {
                            cells.set_w_cell(self.w_pointer, self.h_pointer.saturating_sub(1));
                            for col in &mut cells.columns {
                                col.remove_cell(self.h_pointer);
                            }
                            cells.written = true;
                           
                            self.num_rows -= 1;
                           
                            if self.h_offset < self.num_rows.saturating_sub(self.h_page) {
                                self.draw_rows(cells);
                            } else {
                                self.h_offset -= 1;
                                self.draw_screen(cells);
                            }
                            
                            print_bottom!(
                                self,
                                "Removed row {:04X}",
                                self.h_pointer
                            );
                            self.flush();

                            break;
                        }
                        _ => {
                            print_bottom!(
                                self,
                                "Row {:04X} was not deleted.",
                                self.h_pointer
                            );
                            self.flush();
                            break;
                        }
                    }
                }
                _ => {
                    cmd_err::print(CmdErr::StdinErr, "row delete", self.height);
                    break;
                }
            }
        }
    }

    const VALID_HEX: [char; 22] = [
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9',
        'A', 'B', 'C', 'D', 'E', 'F',
        'a', 'b', 'c', 'd', 'e', 'f',
    ];

    fn compute_valid_hex(c: char) -> Result<usize, CmdErr> {
        let n = (c == Self::VALID_HEX[0]) as usize +
                2 * (c == Self::VALID_HEX[1]) as usize +
                3 * (c == Self::VALID_HEX[2]) as usize +
                4 * (c == Self::VALID_HEX[3]) as usize +
                5 * (c == Self::VALID_HEX[4]) as usize +
                6 * (c == Self::VALID_HEX[5]) as usize +
                7 * (c == Self::VALID_HEX[6]) as usize +
                8 * (c == Self::VALID_HEX[7]) as usize +
                9 * (c == Self::VALID_HEX[8]) as usize +
                10 * (c == Self::VALID_HEX[9]) as usize +
                11 * (c == Self::VALID_HEX[10]) as usize +
                12 * (c == Self::VALID_HEX[11]) as usize +
                13 * (c == Self::VALID_HEX[12]) as usize +
                14 * (c == Self::VALID_HEX[13]) as usize +
                15 * (c == Self::VALID_HEX[14]) as usize +
                16 * (c == Self::VALID_HEX[15]) as usize +
                11 * (c == Self::VALID_HEX[16]) as usize +
                12 * (c == Self::VALID_HEX[17]) as usize +
                13 * (c == Self::VALID_HEX[18]) as usize +
                14 * (c == Self::VALID_HEX[19]) as usize +
                15 * (c == Self::VALID_HEX[20]) as usize +
                16 * (c == Self::VALID_HEX[21]) as usize;

        if n > 0 { return Ok(n - 1); }
        Err(CmdErr::InvalidHex)
    }

    fn hex_to_dec(hex: &str) -> Result<usize, CmdErr> {
        let hex = Self::trim_quotes(hex);
        let len = hex.len();
        let mut hex = hex.chars();
        let mut num = 0usize;
        for i in (0..len).rev() {
            match Self::compute_valid_hex(hex.nth(0).unwrap()) {
                Ok(d) => num += (d * (16usize.pow(i as u32))),
                Err(e) => return Err(e),
            }
        }
        Ok(num)
    }

    fn str_to_dec(s: &str) -> Result<usize, CmdErr> {
        match s.chars().nth(0).unwrap() {
            '\'' | '"' => Self::hex_to_dec(s),
            _       => {
                match s.parse::<usize>() {
                    Ok(val) => Ok(val),
                    Err(_)  => return Err(CmdErr::InvalidDec),
                }
            }
        }
    }

    fn parse_range(&mut self, range: &str) -> Result<(usize, usize), CmdErr> {
        // range can be expressed as:
        // - a single index (e.g. '63' or 99)
        // - two indices separated by a hyphen (e.g. '104D'-'105F')
        // - an index, a '+', and how many rows to include (e.g. 'A7'+10)
        // quotes refer to printed indices
        let mut tokens: Vec<&str> = Vec::new();
        tokens.push(range);
        for c in range.chars() {
            match c {
                ch if ch == '-' || ch == '+' => {
                    tokens = Self::tokenize(range, ch);
                    break;
                }
                _   => continue,
            };
        }

        // if token is empty str, start at first row
        let lo = match tokens[0] {
            "" => 0usize,
            _ => Self::str_to_dec(tokens[0])?,
        };

        // if token is empty str, end at last row
        let hi = match tokens.get(1) {
            Some(val) => {
                match *val {
                    "" => self.num_rows - 1,
                    _ => Self::str_to_dec(val)?,
                }
            }
            None => lo,
        };
        
        Ok((lo, hi))
    }

    fn move_rows(&mut self, cells: &mut Cells, range: &str, target: &str) {
        let (t_lo, t_hi) = match self.parse_range(range) {
            Ok((l, h)) => (l, h),
            Err(e) => {
                cmd_err::print(e, "rows mv", self.height);
                return;
            }
        };

        let lo = t_lo.min(t_hi);
        let hi = t_hi.max(t_lo);

        if lo < 0 || lo >= self.num_rows {
            cmd_err::print(CmdErr::InvalidIndex, &format!("{lo}"), self.height);
            return;
        }

        if hi >= self.num_rows {
            cmd_err::print(CmdErr::InvalidIndex, &format!("{hi}"), self.height);
            return;
        }

        let target = match Self::str_to_dec(target) {
            Ok(d) => d,
            Err(e) => {
                cmd_err::print(e, "rows mv", self.height);
                return;
            }
        };

        cells.w_cell().is_focused = false;

        let new_rows = (target + 1 + hi - lo).saturating_sub(self.num_rows);
        self.num_rows += new_rows;

        let range = hi + 1 - lo;
        let t_l = target;
        let t_h = t_l + range - 1;

        let no_overlap = ((lo > t_h) || (t_l > hi)) as usize;

        if target > lo {
            for col in &mut cells.columns {
                // if moving beyond col.len(), reserve space
                for _ in 0..new_rows {
                    col.push_cell(Cell::new(""));
                }

                for i in 0..range {
                    col.swap_cells(hi - i, t_h - i);
                }

                for _ in 0..(range * no_overlap) {
                    col.bubble_up(lo, t_l.saturating_sub(1));
                }
            }
        } else {
            for col in &mut cells.columns {
                for _ in 0..new_rows {
                    col.push_cell(Cell::new(""));
                }

                for i in 0..range {
                    col.swap_cells(lo + i, t_l + i);
                }

                for _ in 0..(range * no_overlap) {
                    col.bubble_down(hi, t_h + 1);
                }
            }
        }
        
        cells.written = true;
        self.draw_screen(cells);
        self.draw_focused_content();
        self.flush();
    }

    // sheet functions
    //
    fn sort_by(&mut self, cells: &mut Cells, col_name: &str, sort_dir: Sort) {
        let col_name = Self::trim_quotes(col_name);
        match cells.get_col_idx(col_name) {
            None => {
                cmd_err::print(
                      CmdErr::NoName,
                      col_name, self.height
                    );
                return;
            }
            Some(col_idx) => {
                cells.w_cell().is_focused = false;
                {
                    let mut col = &mut cells.columns[col_idx];
                    Self::sort_indices(&mut col, sort_dir);
                }

                for i in 0..col_idx {
                    cells.columns[i].indices = cells.columns[col_idx].indices.clone();
                }
                for i in col_idx + 1..cells.columns.len() {
                    cells.columns[i].indices = cells.columns[col_idx].indices.clone();
                }
                cells.written = true;
                cells.set_w_cell(self.w_pointer, self.h_pointer);
                self.draw_screen(cells);
                self.draw_focused_content();
                self.flush();
            }
        }
    }

    fn slice(&mut self, tokens: Vec<&str>, cells: &mut Cells) {
        let mut tokens = tokens.into_iter();
        for i in 0..2 {
            let _ = tokens.next();
        }
        let or = match tokens.next() {
            Some(o) => o,
            None => {
                cmd_err::print(
                    CmdErr::MissingValue,
                    "slice {'col' or 'row'}",
                    self.height
                );
                return;
            }
        };

       let range = match tokens.next() {
           Some(r) => r,
           None => {
               cmd_err::print(
                   CmdErr::MissingRange,
                   "slice",
                   self.height
                );
               return;
            }
        };

        match or {
            "col" => self.slice_cols(&range, cells),
            "row" => self.slice_rows(&range, cells),
            _ => cmd_err::print(
                    CmdErr::InvalidArg,
                    or,
                    self.height
                ),
        }
    }

    fn slice_cols(&mut self, range: &str, cells: &mut Cells) {
        print_bottom!(self, "slicing cols in range {}", range); 
    }

    fn slice_rows(&mut self, range: &str, cells: &mut Cells) {
        print_bottom!(self, "slicing rows in range {}", range);
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

pub enum SigFlag {
    Winch,
    Int,
    Quit,
    Non,
}

pub fn check_flags() -> SigFlag {
    if GOT_WINCH.swap(false, Ordering::SeqCst) {
        return SigFlag::Winch;
    }

    if GOT_INT.swap(false, Ordering::SeqCst) {
        return SigFlag::Int;
    }

    if GOT_QUIT.swap(false, Ordering::SeqCst) {
        return SigFlag::Quit;
    }

    SigFlag::Non
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
        match std::fs::write("/tmp/csview_panic.log", panic_info) {
            Ok(()) => (),
            Err(_) => println!("csview panicked; couldn't open /tmp/csview_panic.log"),
        }
        std::process::exit(130);
    }));
}

