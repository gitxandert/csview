use std::{
    collections::HashSet,
    mem,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering}
};
use libc::{
    self, c_int, ioctl, winsize, STDOUT_FILENO, TIOCGWINSZ,
    termios, tcgetattr, tcsetattr, cfmakeraw, TCSANOW,
};

use crate::{
    cells::*,
    cmd_err::{self, CmdErr},
    csv_io::{
        append_cmd_history, blank_csv, load_cmd_history, load_csv, make_col_ids,
        poll_stdin, save_backup, write_to_file, PollEvent,
    },
    history::Checkpoint,
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

// word boundaries for tab / shift + tab jumps
pub fn is_delimiter(c: char) -> bool {
    !c.is_alphanumeric()
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
        // the gap can't move past the end of the content
        let pos = pos.min(self.content_len);
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

    // chars on either side of the cursor
    fn prev_char(&self) -> Option<char> {
        self.gap_start.checked_sub(1).map(|i| self.data[i])
    }

    fn next_char(&self) -> Option<char> {
        match self.gap_start < self.content_len {
            true => Some(self.data[self.gap_start + self.gap_len]),
            false => None,
        }
    }

    fn grow(&mut self) {
        let old_cap = self.data.len();
        let new_cap = (old_cap * 2).max(1);

        let mut new_data = vec![' '; new_cap];

        for i in 0..self.gap_start {
            new_data[i] = self.data[i];
        }

        // content after the gap moves to the end of the new buffer
        let post_len = old_cap - (self.gap_start + self.gap_len);
        let old_post_start = self.gap_start + self.gap_len;
        let new_post_start = new_cap - post_len;

        for i in 0..post_len {
            new_data[new_post_start + i] = self.data[old_post_start + i];
        }

        self.data = new_data;
        self.capacity = new_cap;
        self.gap_len = new_post_start - self.gap_start;
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
    bar_start: usize, // first sheet shown in the sheet bar
    num_cols: usize,
    num_rows: usize,
    frame: String,
    focused_content: String,
    cursor: Cursor,
    write_buffer: WriteBuf,
    pub yanked: String,
    cmd_history: Vec<String>,
    cmd_history_idx: Option<usize>, // None while not browsing history
    cmd_draft: String, // typed command, kept while browsing
    cmd_history_file: Option<PathBuf>, // None keeps history in memory only
    split_mode: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PaneRect {
    top: usize,
    left: usize,
    width: usize,
    height: usize,
}

const MIN_PANE_WIDTH: usize = 22;
const MIN_PANE_HEIGHT: usize = 4;

// oldest commands are dropped once the history grows past this
const MAX_CMD_HISTORY: usize = 100;

// helper macro to print to bottom of screen
// (only works with WinInfo)
macro_rules! print_bottom {
    ($self:expr, $fmt:expr $(, $args:expr)*) => {
        let formatted_string = format!(
            concat!("\x1b[{};1H", "\x1b[2K", "\x1b[0m", $fmt),
            $self.height,
            $($args),*
        );
        let fit: String = formatted_string.chars().take($self.width).collect();
        
        $self.push_str_to_frame(&fit);
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
            bar_start: 0usize,
            w_pointer: 0usize,
            h_pointer: 0usize,
            w_page: 0usize,
            h_page: 0usize,
            cursor: Cursor::new(),
            write_buffer: WriteBuf::new(1024),
            yanked: String::new(),
            cmd_history: Vec::<String>::new(),
            cmd_history_idx: None,
            cmd_draft: String::new(),
            cmd_history_file: None,
            split_mode: false,
        }
    }

    pub fn set_context(&mut self, con: &mut Context) {
        self.num_cols = con.cells.num_cols();
        self.num_rows = con.cells.visible_rows();
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
        self.h_page = end.saturating_sub(beg).saturating_sub(1);
    }

    // keep the focus inside the sheet and the offsets at or before the focus;
    // row/column counts can shrink under the pointers (deleting, slicing, undo)
    pub fn clamp_view(&mut self) {
        self.w_pointer = self.w_pointer.min(self.num_cols.saturating_sub(1));
        self.h_pointer = self.h_pointer.min(self.num_rows.saturating_sub(1));
        self.w_offset = self.w_offset.min(self.w_pointer);
        self.h_offset = self.h_offset.min(self.h_pointer);
    }

    pub fn set_write_mode(&mut self, w: bool) {
        let mut out = std::io::stdout();
        match w {
            true => {
                self.input_mode = InputMode::Write;
                let _ = write!(out, "\x1b[?25h");
                self.cursor.offset = 0usize;
            }
            false => {
                self.input_mode = InputMode::Scroll;
                print_bottom!(self, "\x1b[?25l{}", self.focused_content);
            }
        }
        out.flush().unwrap();
    }

    pub fn set_command_mode(&mut self, b: bool, col: &Column) {
        match b {
            true => {
                IN_COMMAND.store(true, Ordering::SeqCst);
                self.input_mode = InputMode::Command;
                // set cursor to a space after a colon at the bottom of the screen
                self.set_cursor(self.height, 2, self.width.saturating_sub(1));
                self.cursor.offset = 0usize;
                self.write_buffer.reset();
                self.cmd_history_idx = None;
                self.cmd_draft.clear();
                self.push_str_to_frame(
                    &format!(
                        "\x1b[{};1H\x1b[2K\x1b[0m:\x1b[{};{}H\x1b[?25h",
                        self.height, self.cursor.line, self.cursor.col
                    )
                );
            }
            false => {
                IN_COMMAND.store(false, Ordering::SeqCst);
                // reset cursor
                self.set_cursor(
                    self.h_pointer.saturating_sub(self.h_offset) + 3,
                    col.start + 1,
                    col.width
                );
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

    pub fn set_yanked(&mut self, cell: &Cell) {
        self.yanked = cell.content.clone();
    }

    pub fn paste(&mut self, cells: &mut Cells) {
        let content = self.yanked.clone();
        self.paste_content(cells, &content);
    }

    pub fn paste_content(&mut self, cells: &mut Cells, content: &str) {
        let cell = cells.w_cell();
        cell.content = content.to_string();
        cell.escape_sequences.clear();
        self.set_focused(&cell.content);
        self.draw_focus(cells);
        cells.written = true;
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
        cell.escape_sequences.clear();
        cell.text_offset = 0;
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
            let diff = buf.content_len.saturating_sub(buf.offset);
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
                buf.offset = buf.offset.saturating_sub(1);
                buf.move_gap(buf.gap_start.saturating_sub(1));
            }
        } else {
            cursor.offset = cursor.offset.saturating_sub(1);
            buf.move_gap(buf.gap_start.saturating_sub(1));
        }
    }

    // move the cursor one char at a time so scrolling stays in sync;
    // stops early if the cursor can't move any further
    fn move_cursor_while(&mut self, right: bool, keep_going: impl Fn(&WriteBuf) -> bool) {
        while keep_going(&self.write_buffer) {
            let before = self.write_buffer.gap_start;
            match right {
                true => self.move_cursor_right(),
                false => self.move_cursor_left(),
            }
            if self.write_buffer.gap_start == before { break; }
        }
    }

    // tab: jump to the start of the next word
    pub fn jump_word_right(&mut self) {
        self.move_cursor_while(true, |b| b.next_char().is_some_and(|c| !is_delimiter(c)));
        self.move_cursor_while(true, |b| b.next_char().is_some_and(is_delimiter));
    }

    // shift + tab: jump to the start of the previous word
    pub fn jump_word_left(&mut self) {
        self.move_cursor_while(false, |b| b.prev_char().is_some_and(is_delimiter));
        self.move_cursor_while(false, |b| b.prev_char().is_some_and(|c| !is_delimiter(c)));
    }

    pub fn move_cursor_home(&mut self) {
        self.move_cursor_while(false, |b| b.gap_start > 0);
    }

    pub fn move_cursor_end(&mut self) {
        self.move_cursor_while(true, |b| b.gap_start < b.content_len);
    }

    fn cursor_pos(&self) -> (usize, usize) {
        (self.cursor.line, self.cursor.col + self.cursor.offset)
    }

    pub fn set_w_h(&mut self, csvs: &mut Csvs) {
        unsafe {
            let mut ws: winsize = mem::zeroed();
            if ioctl(STDOUT_FILENO, TIOCGWINSZ.into(), &mut ws) == 0 {
                self.width = ws.ws_col as usize;
                self.height = ws.ws_row as usize;
            }
            self.changed = WinChange::Screen;
            self.show_workspace(csvs);
        }
    }
    
    pub fn set_w_pointer(&mut self, w: usize) {
        if w < self.num_cols {
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
                self.w_offset = self.num_cols.saturating_sub(self.w_page.max(1));
                self.changed = WinChange::Columns;
            }
        }
    }

    pub fn set_w_offset(&mut self, w: usize) {
        if w < self.num_cols {
            let old_w = self.w_offset;
            self.w_offset = w.min(self.num_cols.saturating_sub(self.w_page));

            // keep w_pointer at same relative spot it was before
            if old_w > self.w_offset {
                let diff = old_w.saturating_sub(self.w_offset);
                self.w_pointer = self.w_pointer.saturating_sub(diff);
                self.changed = WinChange::Columns;
            } else if old_w < self.w_offset {
                let diff = self.w_offset.saturating_sub(old_w);
                self.w_pointer = (self.w_pointer + diff).min(
                    self.num_cols.saturating_sub(1)
                );
                self.changed = WinChange::Columns;
            }
        }
    }
    
    pub fn set_h_pointer(&mut self, h: usize) {
        if h < self.num_rows.saturating_sub(1) {
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
                self.h_offset = self.num_rows.saturating_sub(self.h_page.max(1));
                self.changed = WinChange::Screen;
            }
        }
    }

    pub fn set_h_offset(&mut self, h: usize) {
        if h <= self.num_rows.saturating_sub(1).saturating_sub(self.h_page + 1) {
            let old_h = self.h_offset;
            self.h_offset = h;

            // keep h_pointer at same relative spot it was before
            if old_h > self.h_offset {
                let diff = old_h.saturating_sub(self.h_offset);
                self.h_pointer = self.h_pointer.saturating_sub(diff);
                self.changed = WinChange::Rows;
            } else if self.h_offset > old_h {
                let diff = self.h_offset.saturating_sub(old_h);
                self.h_pointer = (self.h_pointer + diff).min(
                    self.num_rows.saturating_sub(1)
                );
                self.changed = WinChange::Rows;
            }
        }
    }

    pub fn set_focused(&mut self, focused: &str) {
        self.focused_content = Self::terminal_safe_prefix(focused, self.width);
    }

    pub fn add_to_write_buffer(&mut self, content: &str) {
        // insert one char at a time so the cursor and scroll offset stay
        // in sync with the gap when the text overflows the window
        for c in content.chars() {
            self.write_buffer.insert(c);
            self.offset_cursor();
        }

        self.changed = WinChange::Write;
    }

    pub fn paste_yanked_to_write_buffer(&mut self) {
        let yanked = self.yanked.clone();
        self.add_to_write_buffer(&yanked);
    }

    fn offset_cursor(&mut self) {
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

    // load earlier sessions' commands and save new ones to path
    pub fn set_cmd_history_file(&mut self, path: PathBuf) {
        self.cmd_history = load_cmd_history(&path, MAX_CMD_HISTORY);
        self.cmd_history_file = Some(path);
    }

    fn push_cmd_history(&mut self, cmd: &str) {
        self.cmd_history_idx = None;
        if cmd.trim().is_empty()
        || cmd.contains('\n')
        || self.cmd_history.last().map(|c| c.as_str()) == Some(cmd) {
            return;
        }
        self.cmd_history.push(cmd.to_string());
        if let Some(path) = &self.cmd_history_file {
            // history is a convenience; don't interrupt over it
            let _ = append_cmd_history(path, cmd);
        }
        if self.cmd_history.len() > MAX_CMD_HISTORY {
            self.cmd_history.remove(0);
        }
    }

    // replace the command being typed with an older (up) or newer (down) one
    pub fn browse_cmd_history(&mut self, older: bool) {
        let len = self.cmd_history.len();
        let idx = match (self.cmd_history_idx, older) {
            (_, _) if len == 0 => return,
            (None, true) => {
                self.cmd_draft = self.write_buffer.as_string();
                Some(len - 1)
            }
            (None, false) => return,
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i + 1 < len => Some(i + 1),
            (Some(_), false) => None,
        };
        self.cmd_history_idx = idx;

        let cmd = match idx {
            Some(i) => self.cmd_history[i].clone(),
            None => mem::take(&mut self.cmd_draft),
        };
        self.write_buffer.reset();
        self.cursor.offset = 0usize;
        // add one char at a time, as if typed
        for c in cmd.chars() {
            self.add_to_write_buffer(&c.to_string());
        }
        self.changed = WinChange::Command;
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
        match self.input_mode {
            InputMode::Write => {
                let focused_chars = self.focused_content.chars().collect::<Vec<char>>();
                let len = focused_chars.len();
                let hil_st = self.write_buffer.gap_start.min(len.saturating_sub(1));
                let hil_end = (hil_st + 1).min(len);
                let before = focused_chars[..hil_st].iter().collect::<String>();
                let highlighted = focused_chars[hil_st..hil_end].iter().collect::<String>();
                let after = focused_chars[hil_end..len].iter().collect::<String>();
                let content = format!("{before}\x1b[7m{highlighted}\x1b[27m{after}");
                print_bottom!(self, "{}", content);
            }
            _ => {
                print_bottom!(self, "{}", &self.focused_content);
            }
        }
    }

    fn terminal_safe_char(c: char) -> char {
        match c.is_control() {
            true => ' ',
            false => c,
        }
    }

    pub(crate) fn terminal_safe_prefix(content: &str, width: usize) -> String {
        content
            .chars()
            .map(Self::terminal_safe_char)
            .take(width)
            .collect()
    }

    fn terminal_safe_window(content: &str, offset: usize, width: usize) -> String {
        content
            .chars()
            .skip(offset)
            .map(Self::terminal_safe_char)
            .take(width)
            .collect()
    }
    
    fn build_focused_content_rows(&self, raw_content: &str) -> Vec<String> {
        let width = self.width.max(1);
        let mut rows = Vec::<String>::new();
        let mut current_row = String::new();
        let mut active_formatting = String::new();
        let mut col = 0usize;
        let mut chars = raw_content.chars().peekable();

        while let Some(c) = chars.next() {
            match c {
                '\x1b' => {
                    let mut seq = String::from(c);
                    while let Some(next) = chars.next() {
                        seq.push(next);
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }

                    if seq.ends_with('m') {
                        if seq == "\x1b[m" || seq == "\x1b[0m" {
                            active_formatting.clear();
                        } else {
                            active_formatting.push_str(&seq);
                        }
                    }

                    current_row.push_str(&seq);
                }
                '\n' => {
                    rows.push(current_row);
                    current_row = active_formatting.clone();
                    col = 0;
                }
                _ => {
                    current_row.push(c);
                    col += 1;
                    if col >= width {
                        rows.push(current_row);
                        current_row = active_formatting.clone();
                        col = 0;
                    }
                }
            }
        }

        rows.push(current_row);
        rows
    }

    fn draw_focused_content_page(&mut self, rows: &[String], row_offset: usize) {
        let mut display_content = String::new();

        for (i, row) in rows.iter().skip(row_offset).take(self.height).enumerate() {
            if i > 0 {
                display_content.push('\r');
                display_content.push('\n');
            }
            display_content.push_str(row);
        }

        self.push_str_to_frame(&format!("\x1b[1;1H\x1b[2J{}", display_content));
        self.flush();
    }

    pub fn display_focused_content(&mut self, csvs: &mut Csvs) {
        let raw_content = csvs.get_cells().w_cell().raw_content();
        let mut rows = self.build_focused_content_rows(&raw_content);
        let mut row_offset = 0usize;

        self.draw_focused_content_page(&rows, row_offset);

        let mut buf = [0u8; 3];
        loop {
            match poll_stdin(&mut buf) {
                Ok(PollEvent::Sig) => {
                    match check_flags() {
                        SigFlag::Winch => {
                            unsafe {
                                let mut ws: winsize = mem::zeroed();
                                if ioctl(STDOUT_FILENO, TIOCGWINSZ.into(), &mut ws) == 0 {
                                    self.width = ws.ws_col as usize;
                                    self.height = ws.ws_row as usize;
                                }
                            }
                            rows = self.build_focused_content_rows(&raw_content);
                            row_offset = row_offset.min(rows.len().saturating_sub(1));
                            self.draw_focused_content_page(&rows, row_offset);
                        }
                        SigFlag::Int | SigFlag::Quit => continue, // will never catch
                        SigFlag::Non => break, // must've been quit/int
                    }
                }
                Ok(PollEvent::Data(0)) => continue,
                Ok(PollEvent::Data(n)) => {
                    match &buf[..n] {
                        [27, 91, 65] => {
                            row_offset = row_offset.saturating_sub(1);
                            self.draw_focused_content_page(&rows, row_offset);
                        }
                        [27, 91, 66] => {
                            row_offset = (row_offset + 1).min(rows.len().saturating_sub(1));
                            self.draw_focused_content_page(&rows, row_offset);
                        }
                        _ => break,
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
        
        self.draw_context(csvs);
        self.draw_screen(csvs.get_cells());
        self.draw_focused_content();
        self.flush();
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
            let content = Self::terminal_safe_window(
                &col_name.content,
                col_name.text_offset,
                col_name.width
            );
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
            // a column shorter than the sheet draws as blank
            if row >= col.indices.len() {
                self.push_str_to_frame(&format!(" {:<width$} |", "", width = width - 3));
                col.set_start(start);
                start += width;
                *i += 1;
                if *i < self.num_cols {
                    col = &mut cells.columns[*i];
                    width = col.col_width();
                    continue;
                }
                break;
            }
            let cell = col.get_cell(row);
            let content = &cell.content;
            let visible = Self::terminal_safe_window(
                content,
                cell.text_offset,
                cell.width
            );
            let formatted = {
                if cell.is_focused {
                    self.set_focused(&content);
                    self.set_cursor(
                        row.saturating_sub(self.h_offset) + 3, start + 1, cell.width
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

    // switch focus to sheet id; false if there is nothing to switch to
    fn focus_sheet(&mut self, csvs: &mut Csvs, id: usize) -> bool {
        if id == csvs.handle || id >= csvs.num_contexts() {
            return false;
        }
        csvs.save_context(self);
        csvs.set_handle(id);
        self.set_context(csvs.get_context());
        true
    }

    pub fn switch_sheet(&mut self, csvs: &mut Csvs, id: usize) {
        if !self.focus_sheet(csvs, id) {
            return;
        }
        self.changed = WinChange::Screen;
        self.show_workspace(csvs);
    }

    pub fn switch_pane(&mut self, csvs: &mut Csvs) {
        let id = match csvs.other_pane() {
            Some(id) => id,
            None => return,
        };
        self.switch_sheet(csvs, id);
    }

    pub fn active_view_right(&self, csvs: &Csvs) -> usize {
        let split = match csvs.split {
            Some(split) => split,
            None => return self.width,
        };
        let (first, second) = match self.split_rects(split.direction) {
            Some(rects) => rects,
            None => return self.width,
        };
        let rect = if csvs.handle == split.first { first } else { second };
        rect.left + rect.width - 1
    }

    // lays out the sheet bar: returns the first sheet shown, the entries
    // to print (as many as fit, the last one possibly cut short with '…'),
    // and whether sheets are hidden to the left and to the right.
    // two columns are always kept free for the '<' and '>' markers
    fn layout_sheet_bar(
        names: &[String],
        handle: usize,
        start: usize,
        width: usize,
    ) -> (usize, Vec<String>, bool, bool) {
        let n = names.len();
        if n == 0 {
            return (0, Vec::new(), false, false);
        }
        let avail = width.saturating_sub(2);
        let full = |i: usize| format!("{}: {} ", i, names[i]);
        let handle = handle.min(n - 1);

        let mut start = start.min(n - 1);
        if handle < start {
            start = handle;
        }
        while start < handle
        && (start..=handle).map(|i| full(i).chars().count()).sum::<usize>() > avail {
            start += 1;
        }

        let mut entries = Vec::<String>::new();
        let mut used = 0usize;
        let mut filled = false;
        for i in start..n {
            let entry = full(i);
            let w = entry.chars().count();
            if used + w <= avail {
                used += w;
                entries.push(entry);
                continue;
            }
            // cut the name short if at least one char of it fits
            let prefix = format!("{}: ", i);
            let room = (avail - used).saturating_sub(prefix.chars().count() + 1);
            if room > 0 {
                let cut: String = names[i].chars().take(room).collect();
                entries.push(format!("{prefix}{cut}…"));
            }
            filled = true;
            break;
        }
        // the last sheet fit with room to spare (sheets closed, window
        // widened), so pull in earlier sheets until the bar is full
        while !filled && start > 0 {
            let entry = full(start - 1);
            let w = entry.chars().count();
            if used + w > avail {
                break;
            }
            used += w;
            start -= 1;
            entries.insert(0, entry);
        }

        let right = start + entries.len() < n;
        (start, entries, start > 0, right)
    }

    pub fn draw_context(&mut self, csvs: &mut Csvs) {
        self.push_str_to_frame(
            &format!(
                "\x1b[{};1H\x1b[2K\x1b[0m\x1b[30;47m",
                self.height.saturating_sub(1)
            )
        );

        let names: Vec<String> = csvs.contexts
            .iter_mut()
            .map(|con| con.cells().filename.clone())
            .collect();
        let (start, entries, left, right) = Self::layout_sheet_bar(
            &names, csvs.handle, self.bar_start, self.width
        );
        self.bar_start = start;

        let mut used = 0usize;
        let mut bar = String::from(if left { "<" } else { " " });
        for (k, entry) in entries.iter().enumerate() {
            used += entry.chars().count();
            if start + k == csvs.handle {
                bar.push_str(&format!("\x1b[7;36m{}\x1b[27;39m\x1b[30m", entry));
            } else {
                bar.push_str(entry);
            }
        }
        bar.push_str(&" ".repeat(self.width.saturating_sub(used + 2)));
        bar.push(if right { '>' } else { ' ' });
        bar.push_str("\x1b[39;49m");

        if self.width < 2 {
            bar = format!("{}\x1b[39;49m", " ".repeat(self.width));
        }
        self.push_str_to_frame(&bar);
    }

    fn split_rects(&self, direction: SplitDirection) -> Option<(PaneRect, PaneRect)> {
        let content_height = self.height.saturating_sub(2);
        match direction {
            SplitDirection::Vertical => {
                if self.width < MIN_PANE_WIDTH * 2 + 1
                || content_height < MIN_PANE_HEIGHT {
                    return None;
                }
                let first_width = (self.width - 1) / 2;
                let second_width = self.width - first_width - 1;
                Some((
                    PaneRect { top: 1, left: 1, width: first_width, height: content_height },
                    PaneRect {
                        top: 1,
                        left: first_width + 2,
                        width: second_width,
                        height: content_height,
                    },
                ))
            }
            SplitDirection::Horizontal => {
                if self.width < MIN_PANE_WIDTH
                || content_height < MIN_PANE_HEIGHT * 2 + 1 {
                    return None;
                }
                let first_height = (content_height - 1) / 2;
                let second_height = content_height - first_height - 1;
                Some((
                    PaneRect { top: 1, left: 1, width: self.width, height: first_height },
                    PaneRect {
                        top: first_height + 2,
                        left: 1,
                        width: self.width,
                        height: second_height,
                    },
                ))
            }
        }
    }

    fn clear_pane(&mut self, rect: PaneRect) {
        for row in rect.top..rect.top + rect.height {
            self.push_str_to_frame(&format!(
                "\x1b[{row};{}H\x1b[{}X",
                rect.left,
                rect.width,
            ));
        }
    }

    fn draw_split_pane(
        &mut self,
        cells: &mut Cells,
        id: usize,
        rect: PaneRect,
        active: bool,
    ) {
        self.clamp_view();
        cells.set_w_cell(self.w_pointer, self.h_pointer);
        self.clear_pane(rect);

        let marker = if active { "*" } else { " " };
        let title = Self::terminal_safe_prefix(
            &format!("{marker} {id}: {}", cells.filename),
            rect.width.saturating_sub(1),
        );
        let style = if active { "\x1b[7;36;47m" } else { "\x1b[30;47m" };
        self.push_str_to_frame(&format!(
            "\x1b[{};{}H{}{}\x1b[27;39;49m",
            rect.top,
            rect.left,
            style,
            title,
        ));

        let table_top = rect.top + 1;
        let prefix_width = 5usize;
        let mut shown = Vec::<(usize, usize, usize)>::new();
        let mut used = prefix_width;
        let mut col = self.w_offset;
        while col < self.num_cols {
            let col_width = cells.columns[col].col_width();
            if used + col_width >= rect.width {
                break;
            }
            let start = rect.left + used;
            cells.columns[col].set_start(start);
            shown.push((col, start, col_width));
            used += col_width;
            col += 1;
        }
        self.w_page = shown.len().max(1);
        self.h_page = rect.height.saturating_sub(3).max(1);

        if rect.height > 1 {
            self.push_str_to_frame(&format!(
                "\x1b[{table_top};{}H\x1b[4m    |",
                rect.left,
            ));
            for &(idx, start, width) in &shown {
                let value = &cells.col_ids[idx].content;
                self.push_str_to_frame(&format!(
                    "\x1b[{table_top};{start}H {:^width$} |",
                    value,
                    width = width - 3,
                ));
            }
        }
        if rect.height > 2 {
            let row = table_top + 1;
            self.push_str_to_frame(&format!(
                "\x1b[{row};{}H\x1b[1;30;47mHEAD|\x1b[22;39;49m",
                rect.left,
            ));
            for &(idx, start, width) in &shown {
                let value = Self::terminal_safe_window(
                    &cells.header[idx].content,
                    cells.header[idx].text_offset,
                    width - 3,
                );
                self.push_str_to_frame(&format!(
                    "\x1b[{row};{start}H\x1b[30;47m {:<width$} |\x1b[39;49m",
                    value,
                    width = width - 3,
                ));
            }
        }

        let rows = rect.height.saturating_sub(3);
        for displayed in 0..rows {
            let sheet_row = self.h_offset + displayed;
            if sheet_row >= self.num_rows {
                break;
            }
            let row = table_top + 2 + displayed;
            self.push_str_to_frame(&format!(
                "\x1b[{row};{}H\x1b[30;47m{:04X} \x1b[39;49m",
                rect.left,
                sheet_row,
            ));
            for &(idx, start, width) in &shown {
                let focused = active && idx == self.w_pointer && sheet_row == self.h_pointer;
                let (value, full_value) = if focused && self.input_mode == InputMode::Write {
                    let full = self.write_buffer.as_string();
                    (
                        Self::terminal_safe_window(
                            &full,
                            self.write_buffer.offset,
                            width - 3,
                        ),
                        full,
                    )
                } else if sheet_row < cells.columns[idx].indices.len() {
                    let cell = cells.columns[idx].get_cell(sheet_row);
                    (
                        Self::terminal_safe_window(&cell.content, cell.text_offset, width - 3),
                        cell.content.clone(),
                    )
                } else {
                    (String::new(), String::new())
                };
                let style = if focused { "\x1b[7;36;47m" } else { "" };
                let reset = if focused { "\x1b[27;39;49m" } else { "" };
                self.push_str_to_frame(&format!(
                    "\x1b[{row};{start}H{style} {:<width$} {reset}|",
                    value,
                    width = width - 3,
                ));
                if focused {
                    self.set_focused(&full_value);
                    self.set_cursor(row, start + 1, width - 3);
                }
            }
        }
    }

    fn draw_split(&mut self, csvs: &mut Csvs) -> bool {
        let split = match csvs.split {
            Some(split) => split,
            None => return false,
        };
        let (first_rect, second_rect) = match self.split_rects(split.direction) {
            Some(rects) => rects,
            None => return false,
        };

        csvs.save_context(self);
        let active = csvs.handle;
        let inactive = if active == split.first { split.second } else { split.first };
        let inactive_rect = if inactive == split.first { first_rect } else { second_rect };
        let active_rect = if active == split.first { first_rect } else { second_rect };

        {
            let con = &mut csvs.contexts[inactive];
            self.set_context(con);
            self.draw_split_pane(&mut con.cells, inactive, inactive_rect, false);
            con.save(self);
        }
        {
            let con = &mut csvs.contexts[active];
            self.set_context(con);
            self.draw_split_pane(&mut con.cells, active, active_rect, true);
        }

        match split.direction {
            SplitDirection::Vertical => {
                let col = first_rect.left + first_rect.width;
                for row in 1..=first_rect.height {
                    self.push_str_to_frame(&format!("\x1b[{row};{col}H\x1b[2m|\x1b[22m"));
                }
            }
            SplitDirection::Horizontal => {
                let row = first_rect.top + first_rect.height;
                self.push_str_to_frame(&format!(
                    "\x1b[{row};1H\x1b[2m{}\x1b[22m",
                    "-".repeat(self.width.saturating_sub(1)),
                ));
            }
        }
        true
    }

    pub fn show_workspace(&mut self, csvs: &mut Csvs) {
        let was_split = self.split_mode;
        self.split_mode = csvs.split.is_some();
        if csvs.split.is_none() {
            if was_split {
                self.changed = WinChange::Screen;
            }
            if matches!(self.changed, WinChange::Screen | WinChange::Init) {
                self.draw_context(csvs);
            }
            self.show_csv(csvs.get_cells());
            return;
        }
        if self.input_mode == InputMode::Command {
            if self.changed == WinChange::Command {
                self.draw_command();
                self.flush();
            }
            return;
        }
        let changed = self.changed != WinChange::Non;
        if self.draw_split(csvs) {
            self.draw_context(csvs);
            if changed {
                self.draw_focused_content();
            }
        } else {
            self.split_mode = false;
            self.draw_screen(csvs.get_cells());
            self.split_mode = true;
            self.draw_context(csvs);
            print_bottom!(self, "Terminal too small for split");
        }
        self.flush();
    }

    pub fn draw_screen(&mut self, cells: &mut Cells) {
        if self.split_mode {
            return;
        }
        // reset focused cell
        self.clamp_view();
        cells.set_w_cell(self.w_pointer, self.h_pointer);
        
        let mut id = self.w_offset;
        for i in 1..self.height.saturating_sub(1) {
            // move cursor to beginning of line
            self.push_str_to_frame(&format!("\x1b[{i};1H\x1b[2K"));
            let start = 6usize;
            if i == 1 {
                self.push_str_to_frame("\x1b[4m    |");
                self.print_col_ids(cells, id, start);
            } else if i == 2 {
                self.push_str_to_frame("\x1b[1;30;47mHEAD|\x1b[22;39;49m");
                id = self.w_offset;
                self.print_header(cells, id, start);
            } else {
                let row_id = i.saturating_sub(3) + self.h_offset;
                if row_id >= self.num_rows {
                    continue;
                }

                let row_num = format!(
                    "\x1b[30;47m{:04X} \x1b[39;49m", row_id
                );
                self.push_str_to_frame(&row_num);

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
        if self.split_mode {
            return;
        }
        self.clamp_view();
        let col_id = self.w_pointer;
        let start = cells.columns[col_id].start;

        let mut c = col_id;
        let lim = self.height.saturating_sub(1).min(
            self.num_rows.saturating_sub(self.h_offset) + 3
        );
        for row in 1..lim {
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
        if self.split_mode {
            return;
        }
        self.clamp_view();
        let (_, prev_row) = cells.w_cell;
        cells.set_w_cell(self.w_pointer, self.h_pointer);

        // redraw from the earlier of the two rows, or from the top
        // if that row has scrolled out of view
        let st_row = prev_row.min(self.h_pointer).max(self.h_offset);
        let term_row = st_row - self.h_offset + 3;
        self.push_str_to_frame(
            &format!("\x1b[{};1H\x1b[4m", term_row)
        );

        let lim = self.height.saturating_sub(1).min(
            self.num_rows.saturating_sub(self.h_offset) + 3
        );
        for row in term_row..lim {
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
        if self.split_mode {
            return;
        }
        // erase the previously-focused cell
        // and whatever follows it
        self.clamp_view();
        let (wc_c, wc_l) = cells.w_cell;
        // the previous focus may be gone if the sheet shrank
        let wc_c = wc_c.min(cells.num_cols().saturating_sub(1));
        let wc_l = wc_l.min(self.num_rows.saturating_sub(1));
        cells.set_w_cell(self.w_pointer, self.h_pointer);

        let prev_start = cells.columns[wc_c].start;
        let cur_start = cells.columns[self.w_pointer].start;
        let start = prev_start.min(cur_start);
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
        if self.split_mode {
            return;
        }
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

        for i in 0..take_1 {
            let c = self.write_buffer.data[i];
            let display_c = Self::terminal_safe_char(c);
            if i >= offset {
                self.push_to_frame(display_c);
            }
            self.focused_content.push(display_c);
        }
        
        // take after gap
        let mut post_gap = self.write_buffer.gap_start + self.write_buffer.gap_len;
        let take_2 = post_gap + col.width.saturating_sub(take_1.saturating_sub(offset));
        for i in post_gap..self.write_buffer.data.len() {
            let c = self.write_buffer.data[i];
            let display_c = Self::terminal_safe_char(c);
            if i < take_2 {
                self.push_to_frame(display_c);
                post_gap += 1;
            }
            if self.focused_content.chars().count() < self.width {
                self.focused_content.push(display_c);
            }
        }
        // pad end with whitespace
        while take_2 > post_gap {
            self.push_to_frame(' ');
            post_gap += 1;
        }
        self.push_str_to_frame(" \x1b[27;39;49m|");
        
        // only print rest of row if not editing last cell
        if id + 1 < self.w_offset + self.w_page {
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
        let post_gap = self.write_buffer.gap_start + self.write_buffer.gap_len;
        let take_2 = post_gap + self.width.saturating_sub(take_1.saturating_sub(offset));
        for i in post_gap..self.write_buffer.data.len() {
            if i < take_2 {
                self.push_to_frame(self.write_buffer.data[i]);
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
        if self.split_mode {
            return;
        }
        let (mut i, row) = cells.w_cell;
        let cursor = format!("\x1b[{};{}H\x1b[K\x1b[4m",
            row.saturating_sub(self.h_offset) + 3, &cells.columns[i].start
        );
        self.push_str_to_frame(&cursor);
        let start = cells.columns[i].start;
        self.print_row(cells, &mut i, row, start);
        self.set_w_page(
            i, self.w_offset
        );
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
        if self.split_mode && !self.frame.is_empty() {
            let _ = write!(out, "\x1b[?2026h{}\x1b[?2026l", self.frame);
        } else {
            let _ = write!(out, "{}", self.frame);
        }
        out.flush().unwrap();

        self.frame = String::new();
        self.changed = WinChange::Non;
    }

    pub fn show_csv(&mut self, cells: &mut Cells) {
        if self.split_mode {
            return;
        }
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
        let mut quote: Option<char> = None;
        
        // track byte offsets, since input may hold multi-byte chars
        for (end, c) in input.char_indices() {
            match c {
                ch if ch == delim => {
                    // only push if not quoting
                    if quote.is_none() {
                        tokens.push(&input[start..end]);
                        start = end + c.len_utf8();
                    }
                }
                '"' | '\'' => {
                    // if not quoting, start;
                    // if quoting, stop
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
                }
                _ => (),
            }
        }

        tokens.push(&input[start..]);

        tokens
    }

    fn tokenize_range(range: &str) -> (Vec<&str>, Option<char>) {
        let mut tokens = Vec::<&str>::new();
        for c in range.chars() {
            match c {
                ch if ch == '-' || ch == '+' => {
                    tokens = Self::tokenize(range, ch);
                    return (tokens, Some(ch));
                }
                _   => continue,
            };
        }
        
        tokens.push(range);
        (tokens, None)
    }

    pub fn process_command(&mut self, csvs: &mut Csvs) -> SigFlag {
        let input = self.write_buffer.as_string();
        self.push_cmd_history(&input);
        let tokens = Self::tokenize(&input, ' ');
        let checkpoint = Checkpoint::take(csvs, self, &tokens);
        match tokens[0] {
            // quit
            "q" | "quit" => return SigFlag::Quit,
            // closing the only sheet closes the program
            "sh" | "sheet"
                if matches!(tokens.get(1), Some(&"c") | Some(&"close"))
                && csvs.num_contexts() == 1 => return SigFlag::Quit,
            // column name
            // can show, edit, and find column names
            "cn" => self.cn_cmd(
                        tokens,
                        csvs.get_cells()
                    ),
            // column
            // whole-column functions
            "col" => self.col_cmd(
                        tokens,
                        csvs
                    ),
            // row
            // row operations
            "row" => self.row_cmd(
                        tokens,
                        csvs
                    ),
            // sheet
            // whole-sheet operations
            "sh" | "sheet" => self.sheet_cmd(
                                    tokens,
                                    csvs
                                  ),
            // invalid
            _ => cmd_err::print(
                    CmdErr::InvalidCommand(tokens[0]), 
                    self.height
                ),
        }
        checkpoint.commit(csvs);
        // keep the cached counts in step with whatever the command changed
        let cells = csvs.get_cells();
        self.num_cols = cells.num_cols();
        self.num_rows = cells.visible_rows();
        self.clamp_view();
        SigFlag::Non
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
                                        CmdErr::MissingName(spec), 
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
                                        CmdErr::MissingName(spec), 
                                        self.height
                                    ),
                            Some(name) => self.find_column(cells, &name),
                        }
                    }
                    _ => cmd_err::print(
                            CmdErr::UnknownSpec(spec), 
                            self.height
                        ),
                }
            }
        }
    }
    
    fn col_cmd(&mut self, tokens: Vec<&str>, csvs: &mut Csvs) {
        let tok = tokens[0];
        let subcmd = match tokens.get(1) {
            None => {
                cmd_err::print(
                    CmdErr::MissingSubCmd(tok), 
                    self.height
                );
                return;
            }
            Some(sc) => *sc,
        };

        match subcmd {
            "c"  | "count"   => self.print_col_count(),
            "mv" | "move"    => {
                match tokens.get(2) {
                    None => cmd_err::print(
                                CmdErr::MissingRange(subcmd), 
                                self.height
                            ),
                    Some(range) => {
                        match tokens.get(3) {
                            None => cmd_err::print(
                                        CmdErr::MissingLocation(subcmd),
                                        self.height
                                    ),
                            Some(loc) => self.move_columns(
                                            csvs.get_cells(), 
                                            &range,
                                            &loc,
                                        ),
                        }
                    }
                }
            }
            "f"  | "find"    => {
                match tokens.get(2) {
                    None => cmd_err::print(
                                CmdErr::MissingValue(subcmd), 
                                self.height
                            ),
                    Some(val) => self.find_value_in_col(
                                    csvs, 
                                    &val,
                                    tokens
                                ),
                                
                }
            }
            "n"  | "new"     => {
                match tokens.get(2) {
                    None => cmd_err::print(
                                CmdErr::MissingName(subcmd), 
                                self.height
                            ),
                    Some(name) => self.new_column(
                                    csvs.get_cells(), 
                                    &name
                                ),
                }
            }
            "rm" | "remove"  => {
                match tokens.get(2) {
                    Some(_) => cmd_err::print(
                                    CmdErr::TooManyArgs(subcmd), 
                                    self.height
                                ),
                    None => self.remove_column(csvs),
                }
            }
            "g"  | "group"   => {
                match tokens.get(2) {
                    None => cmd_err::print(
                                CmdErr::MissingList(subcmd), 
                                self.height
                            ),
                    Some(list) => self.group_columns(
                                            csvs.get_cells(), 
                                            &list
                                        ),
                }
            }
            "u"  | "unique"  => self.show_unique_column_values(csvs),
            "s"  | "sort"    => {
                let mut s_dir = Sort::AscAlph;
                match tokens.get(2) {
                    None => self.sort_focused_column(
                                            csvs.get_cells(), 
                                            s_dir
                                        ),
                    Some(spec) => {
                        match *spec {
                            "a" => (),
                            "r" | "ar" => s_dir = Sort::DescAlph,
                            "n" => s_dir = Sort::AscNum,
                            "nr" => s_dir = Sort::DescNum,
                            _ => {
                                cmd_err::print(
                                    CmdErr::UnknownSpec(spec), 
                                    self.height
                                );
                                return;
                            }
                        }
                        self.sort_focused_column(
                                    csvs.get_cells(), 
                                    s_dir
                                );
                    }
                }
            }
            "rv" | "revert" => self.revert_focused_column(
                                    csvs.get_cells()
                                ),
            "fn" | "fillna" => {
                match tokens.get(2) {
                    None => cmd_err::print(
                                CmdErr::MissingValue(subcmd),
                                self.height
                            ),
                    Some(val) => self.col_fillna(
                                        csvs.get_cells(), 
                                        &val
                                    ),
                }
            }
            "r"  | "replace" => {
                match tokens.get(2) {
                    None => cmd_err::print(
                                CmdErr::MissingValue(subcmd),
                                self.height
                            ),
                    Some(targ) => {
                        match tokens.get(3) {
                            None => cmd_err::print(
                                        CmdErr::MissingValue(subcmd),
                                        self.height
                                    ),
                            Some(new) => self.replace_in_col(
                                                csvs.get_cells(), 
                                                &targ, 
                                                &new
                                            ),
                        }
                    }
                }
            }
            "a" | "add" => self.add_vals_in_col(csvs.get_cells()),
            "d" | "diff" => {
                match tokens.get(2) {
                    None => cmd_err::print(
                                CmdErr::MissingValue(subcmd),
                                self.height
                            ),
                    Some(other) => self.col_cmp(
                                            csvs,
                                            &other,
                                            true,
                                            &tokens[3..]
                                        ),
                }
            },
            "si" | "sim" => {
                match tokens.get(2) {
                    None => cmd_err::print(
                                CmdErr::MissingValue(subcmd),
                                self.height
                            ),
                    Some(other) => self.col_cmp(
                                            csvs,
                                            &other,
                                            false,
                                            &tokens[3..]
                                        ),
                }
            },
            "pre" | "prepend" => {
                match tokens.get(2) {
                    None => cmd_err::print(
                                CmdErr::MissingValue(subcmd),
                                self.height
                            ),
                    Some(pre) => self.col_pre(csvs.get_cells(), &pre),
                }
            },
            "app" | "append" => {
                match tokens.get(2) {
                    None => cmd_err::print(
                                CmdErr::MissingValue(subcmd),
                                self.height
                            ),
                    Some(app) => self.col_app(csvs.get_cells(), &app),
                }
            },
            "dup" | "duplicate" => self.col_dup(csvs.get_cells()),
            "fb"  | "filterby"  => {
                match tokens.get(2) {
                    Some(col) => self.col_filterby(csvs.get_cells(), &col),
                    None => cmd_err::print(
                                CmdErr::MissingValue(subcmd),
                                self.height
                            ),
                }
            }
            "up"  | "upper" => self.col_up(csvs.get_cells()),
            "lo"  | "lower" => self.col_lo(csvs.get_cells()),
            _ => cmd_err::print(
                    CmdErr::InvalidSubCmd(subcmd),
                    self.height
                ),
        }
    }

    fn row_cmd(&mut self, tokens: Vec<&str>, csvs: &mut Csvs) {
        let mut tokens = tokens.into_iter();
        let tok = tokens.next().unwrap();
        let subcmd = match tokens.next() {
            None => {
                cmd_err::print(
                    CmdErr::MissingSubCmd(tok), 
                    self.height
                );
                return;
            }
            Some(sc) => sc,
        };
        
        match subcmd {
            "c"  | "count" => self.print_row_count(),
            "i"  | "insert" => {
                let count = match tokens.next() {
                    Some(num) => {
                        match num.parse::<usize>() {
                            Ok(u) => u,
                            Err(_) => {
                                cmd_err::print(
                                    CmdErr::InvalidDec(num),
                                    self.height
                                );
                                return;
                            }
                        }
                    }
                    None => 1usize,
                };
                self.insert_row(csvs.get_cells(), count);
            }
            "dup"  | "duplicate" => {
                let count = match tokens.next() {
                    Some(num) => {
                        match num.parse::<usize>() {
                            Ok(u) => u,
                            Err(_) => {
                                cmd_err::print(
                                    CmdErr::InvalidDec(num),
                                    self.height
                                );
                                return;
                            }
                        }
                    }
                    None => 1usize,
                };
                self.duplicate_row(csvs.get_cells(), count);
            }
            "d"  | "delete" => self.delete_row(csvs),
            "mv" | "move"   => {
                match tokens.next() {
                    None => cmd_err::print(
                                CmdErr::MissingRange(subcmd), 
                                self.height
                            ),
                    Some(range) => {
                        match tokens.next() {
                            None => cmd_err::print(
                                        CmdErr::MissingTarget(subcmd), 
                                        self.height
                                    ),
                            Some(target) => self.move_rows(
                                                csvs.get_cells(), 
                                                &range, 
                                                &target
                                            ),
                        }
                    }
                }
            }
            "g" | "goto" => {
                match tokens.next() {
                    None => self.goto_row(csvs.get_cells(), None),
                    Some(loc) => self.goto_row(csvs.get_cells(), Some(&loc)),
                }
            }
            "n" | "num" => self.show_row_num(),
            "a" | "add" => self.add_vals_in_row(csvs.get_cells()),
            _ =>  cmd_err::print(
                    CmdErr::InvalidSubCmd(tok), 
                    self.height
                ),
        }
    }
        
    fn sheet_cmd(&mut self, tokens: Vec<&str>, csvs: &mut Csvs) {
        let tok = tokens[0];
        let subcmd = match tokens.get(1) {
            None =>{
                cmd_err::print(
                    CmdErr::MissingSubCmd(tok), 
                    self.height
                );
                return;
            }
            Some(sc) => *sc,
        };
        
        match subcmd {
            "s" | "sortby" => {
                match tokens.get(2) {
                    None => cmd_err::print(
                                CmdErr::MissingName(tok),
                                self.height
                            ),
                    Some(col) => {
                        let s_dir = match tokens.get(3) {
                            None => Sort::AscAlph,
                            Some(dir) => {
                                match *dir {
                                    "a" => Sort::AscAlph,
                                    "r" | "ar" => Sort::DescAlph,
                                    "n" => Sort::AscNum,
                                    "nr" => Sort::DescNum,
                                    _ => {
                                        cmd_err::print(
                                            CmdErr::InvalidArg(dir), 
                                            self.height
                                        );
                                        return;
                                    }
                                }
                            }
                        };
                        self.sort_by(
                            csvs.get_cells(), 
                            &col, 
                            s_dir
                        );
                    }
                }
            }
            "sf" | "siftby" => {
                match tokens.get(2) {
                    None => cmd_err::print(
                                CmdErr::MissingName(tok),
                                self.height
                            ),
                    Some(col) => self.sift_by(csvs.get_cells(), col),
                }
            }
            "rv" | "revert" => self.revert_sheet(csvs.get_cells()),
            "sl" | "slice" => self.slice(tokens, csvs),
            "sp" | "splice" => self.splice(tokens, csvs),
            "r"  | "replace" => self.replace_in_sheet(tokens, csvs.get_cells()),
            "f"  | "find" => {
                match tokens.get(2) {
                    None => cmd_err::print(
                                CmdErr::MissingValue(tok),
                                self.height
                            ),
                    Some(term) => self.find_in_sheet(
                                    csvs.get_cells(),
                                    term,
                                    tokens
                                )
                }
            }
            "rn" | "rename" => {
                match tokens.get(2) {
                    None => cmd_err::print(
                                CmdErr::MissingName(tok),
                                self.height
                            ),
                    Some(name) => self.rename_sheet(csvs, name)
                }
            }
            "g"  | "goto" => self.goto_sheet(tokens, csvs),
            "o"  | "open" => self.open_sheet(tokens, csvs),
            "c"  | "close" => self.close_sheet(csvs),
            "split" => self.split_sheet(tokens, csvs),
            _ => cmd_err::print(
                    CmdErr::InvalidSubCmd(subcmd), 
                    self.height
                ),
        }
    }

    // finds a sheet by index, or by name: an exact match on the
    // filename, basename, or basename without extension wins, then
    // a unique prefix of the filename or basename
    fn resolve_sheet<'a>(csvs: &Csvs, target: &'a str) -> Result<usize, CmdErr<'a>> {
        let t = Self::trim_quotes(target);
        if !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()) {
            return match t.parse::<usize>() {
                Ok(id) if id < csvs.num_contexts() => Ok(id),
                _ => Err(CmdErr::InvalidConId(target)),
            };
        }

        let t = t.to_lowercase();
        let names: Vec<(String, String, String)> = csvs.contexts
            .iter()
            .map(|con| {
                let full = con.cells.filename.clone();
                let path = Path::new(&full);
                let base = path.file_name()
                    .map(|b| b.to_string_lossy().into_owned())
                    .unwrap_or_else(|| full.clone());
                let stem = path.file_stem()
                    .map(|b| b.to_string_lossy().into_owned())
                    .unwrap_or_else(|| base.clone());
                (full.to_lowercase(), base.to_lowercase(), stem.to_lowercase())
            })
            .collect();

        let tiers: [Box<dyn Fn(&(String, String, String)) -> bool>; 3] = [
            Box::new(|n| n.0 == t),
            Box::new(|n| n.1 == t || n.2 == t),
            Box::new(|n| n.0.starts_with(&t) || n.1.starts_with(&t)),
        ];
        for tier in &tiers {
            let hits: Vec<usize> = (0..names.len())
                .filter(|&i| tier(&names[i]))
                .collect();
            match hits.len() {
                0 => (),
                1 => return Ok(hits[0]),
                n => return Err(CmdErr::AmbiguousSheet(target, n)),
            }
        }
        Err(CmdErr::NoSheet(target))
    }

    // `sh g {index or name}` focuses another sheet
    fn goto_sheet(&mut self, tokens: Vec<&str>, csvs: &mut Csvs) {
        let tok = tokens[1];
        let target = match tokens.get(2) {
            Some(t) => *t,
            None => {
                cmd_err::print(CmdErr::MissingName(tok), self.height);
                return;
            }
        };
        if tokens.len() > 3 {
            cmd_err::print(CmdErr::TooManyArgs(tok), self.height);
            return;
        }
        let id = match Self::resolve_sheet(csvs, target) {
            Ok(id) => id,
            Err(e) => {
                cmd_err::print(e, self.height);
                return;
            }
        };
        if !self.focus_sheet(csvs, id) {
            return;
        }
        self.draw_context(csvs);
        self.changed = WinChange::Screen;
        self.show_csv(csvs.get_cells());
        self.flush();
    }

    fn split_sheet(&mut self, tokens: Vec<&str>, csvs: &mut Csvs) {
        let arg = match tokens.get(2) {
            Some(arg) => *arg,
            None => {
                cmd_err::print(CmdErr::MissingValue("split"), self.height);
                return;
            }
        };
        if arg == "off" {
            if tokens.len() > 3 {
                cmd_err::print(CmdErr::TooManyArgs("split"), self.height);
                return;
            }
            self.disable_split(csvs);
            return;
        }
        let direction = match arg {
            "h" | "horizontal" => SplitDirection::Horizontal,
            "v" | "vertical" => SplitDirection::Vertical,
            _ => {
                cmd_err::print(CmdErr::InvalidArg(arg), self.height);
                return;
            }
        };
        if tokens.len() > 4 {
            cmd_err::print(CmdErr::TooManyArgs("split"), self.height);
            return;
        }

        if let Some(mut split) = csvs.split {
            split.direction = direction;
            if let Some(target) = tokens.get(3) {
                let id = match Self::resolve_sheet(csvs, target) {
                    Ok(id) => id,
                    Err(e) => {
                        cmd_err::print(e, self.height);
                        return;
                    }
                };
                if id == split.first {
                    cmd_err::print(CmdErr::SamePane, self.height);
                    return;
                }
                split.second = id;
                if split.active == SplitPane::Second {
                    csvs.set_handle(id);
                    split.active = SplitPane::Second;
                }
            }
            csvs.split = Some(split);
        } else {
            let second = match tokens.get(3) {
                Some(target) => match Self::resolve_sheet(csvs, target) {
                    Ok(id) => id,
                    Err(e) => {
                        cmd_err::print(e, self.height);
                        return;
                    }
                },
                None if csvs.num_contexts() > 1 => (csvs.handle + 1) % csvs.num_contexts(),
                None => {
                    let id = csvs.num_contexts();
                    let delim = csvs.get_cells().delim;
                    let rows = self.height.saturating_sub(4);
                    csvs.push_context(Context::new(
                        id,
                        blank_csv("Untitled.csv".to_string(), delim, rows),
                    ));
                    id
                }
            };
            if second == csvs.handle {
                cmd_err::print(CmdErr::SamePane, self.height);
                return;
            }
            csvs.start_split(direction, second);
        }
        self.changed = WinChange::Screen;
    }

    fn disable_split(&mut self, csvs: &mut Csvs) {
        if csvs.split.is_none() {
            print_bottom!(self, "No split view");
            self.flush();
            return;
        }
        csvs.save_context(self);
        csvs.split = None;
        self.set_context(csvs.get_context());
        self.changed = WinChange::Screen;
    }

    fn trim_quotes(q: &str) -> String {
        match q.chars().next() {
            Some(first) if first == '\'' || first == '"' => {
                let inner = &q[first.len_utf8()..];
                inner.strip_suffix(first).unwrap_or(inner).to_string()
            }
            _ => q.to_string(),
        }
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
        cells.header[self.w_pointer].content = new_name;
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
        let mut next = self.w_pointer;
        for _ in 0..header.len() {
            if header[next].content.contains(&name) {
                let name = header[next].content.clone();
                self.set_w_pointer(next);
                self.changed = WinChange::Columns;
                self.show_csv(cells);
                print_bottom!(
                    self,
                    "{}",
                    name
                );
                return;
            }
            next = (next + 1) % header.len();
        }
        cmd_err::print(CmdErr::NoNameContains(&name), self.height);
    }

    // column functions
    //
    fn print_col_count(&mut self) {
        print_bottom!(
            self,
            "{} columns",
            self.num_cols
        );
    }

    fn move_columns(&mut self, cells: &mut Cells, range: &str, loc: &str) {
        let (tokens, delim) = Self::tokenize_range(range); 
        
        let ida = match tokens[0] {
            "" => 0,
            "_" => self.w_pointer,
            _ => {
                match cells.get_col_idx(tokens[0]) {
                    Ok(id) => id,
                    Err(e) => {
                        cmd_err::print(
                            e, self.height
                        );
                        return;
                    }
                }
            }
        };

        let idb = match tokens.get(1) {
            None => ida,
            Some(t) => {
                match delim {
                    None => ida,
                    Some(d) => {
                        match d {
                            '-' => {
                                match *t {
                                    "" => cells.num_cols() - 1,
                                    _ => {
                                        match cells.get_col_idx(t) {
                                            Ok(id) => id,
                                            Err(e) => {
                                                cmd_err::print(
                                                    e, self.height
                                                );
                                                return;
                                            }
                                        }
                                    }
                                }
                            }
                            '+' => {
                                match *t {
                                    "" => cells.num_cols() - 1,
                                    _ => {
                                        match Self::str_to_dec(t) {
                                            Ok(d) => {
                                                let i = ida + d;
                                                if i >= cells.num_cols() {
                                                    cmd_err::print(
                                                        CmdErr::InvalidIndex(ida + d),
                                                        self.height
                                                    );
                                                    return;
                                                }
                                                i
                                            }
                                            Err(e) => {
                                                cmd_err::print(
                                                    e, self.height
                                                );
                                                return;
                                            }
                                        }
                                    }
                                }
                            }
                            _ => ida,
                        }
                    }
                }
            }
        };

        let idx = match cells.get_col_idx(loc) {
            Ok(id) => id,
            Err(e) => {
                cmd_err::print(e, self.height);
                return;
            }
        };

        // accept ranges written in either order
        let (ida, idb) = (ida.min(idb), ida.max(idb));
        let diff = idb - ida;
        // a block moved ahead starts at idx, so it must fit before the end
        if idx > ida && idx + diff >= cells.num_cols() {
            cmd_err::print(CmdErr::InvalidIndex(idx + diff), self.height);
            return;
        }

        cells.w_cell().is_focused = false;
        let is_behind = (ida > idx) as usize;
        let is_ahead = (idx > ida) as usize;
        for i in 0..=diff {
            let a = ida + i * is_behind;
            let x = idx + diff * is_ahead + i * is_behind;
            let col = cells.columns.remove(a);
            let col_name = cells.header.remove(a);
            let w = col.width;
            // move column and header
            cells.columns.insert(x, col);
            cells.header.insert(x, col_name);
            // adjust col_id widths
            cells.col_ids[a].width = cells.columns[a].width;
            cells.col_ids[x].width = w;
        }
        cells.written = true;

        self.set_w_pointer(idx);
        self.changed = WinChange::Columns;
        self.show_csv(cells);
    }
    
    fn draw_col_find(&mut self, cells: &mut Cells, idx: usize, indices: &Vec<usize>, mess: &str) {
        self.set_h_pointer(indices[idx]);
        self.show_csv(cells);
        print_bottom!(
            self, 
            "{}/{} {}",
            idx + 1, 
            indices.len(),
            mess
        );
        self.flush();
    }
    
    fn draw_sheet_find(&mut self, cells: &mut Cells, idx: usize, indices: &Vec<(usize, usize)>, mess: &str) {
        self.set_w_pointer(indices[idx].0);
        self.set_h_pointer(indices[idx].1);
        self.draw_screen(cells);
        print_bottom!(
            self, 
            "{}/{} {}",
            idx + 1, 
            indices.len(),
            mess
        );
        self.flush();
    }

    fn matching_column_rows(col: &Column, val: &str, insensitive: bool) -> Vec<usize> {
        (0..col.visible_len())
            .filter(|&row| {
                let content = col.view_cell(row);
                if insensitive {
                    content.to_lowercase().contains(val)
                } else {
                    content.contains(val)
                }
            })
            .collect()
    }

    fn apply_column_filter(col: &mut Column, visible_rows: &[usize]) {
        // rows hidden by an earlier filter stay hidden, after the new ones
        let row_count = col.visible_len();
        let previous_indices = col.indices.clone();
        let mut is_visible = vec![false; row_count];
        for &row in visible_rows {
            if row < row_count {
                is_visible[row] = true;
            }
        }

        let hidden_rows = (0..row_count)
            .filter(|&row| !is_visible[row])
            .collect::<Vec<usize>>();
        let padding = hidden_rows.len();
        let mut new_indices = Vec::with_capacity(previous_indices.len() + padding);

        new_indices.extend(
            visible_rows
                .iter()
                .filter_map(|&row| previous_indices.get(row).copied()),
        );

        let padding_start = col.cells.len();
        for offset in 0..padding {
            new_indices.push(padding_start + offset);
            col.cells.push(Cell::new(""));
        }

        new_indices.extend(
            hidden_rows
                .iter()
                .filter_map(|&row| previous_indices.get(row).copied()),
        );
        new_indices.extend(previous_indices.into_iter().skip(row_count));

        col.padding += padding;
        col.indices = new_indices;
    }

    fn find_value_in_col(&mut self, csvs: &mut Csvs, val: &str, options: Vec<&str>) {
        if val == "" {
            print_bottom!(self, "Specify a value for 'col f'");
            return;
        }

        let mut val = Self::trim_quotes(val);
        let mut filter = false;
        let mut insensitive = false;

        for i in 3..options.len() {
            match options[i] {
                "-f" | "--filter" => filter = true,
                "-i" | "--case-insensitive" => {
                    val = val.to_lowercase();
                    insensitive = true;
                }
                _ => {
                    cmd_err::print(
                        CmdErr::InvalidOption(options[i]),
                        self.height
                    );
                    return;
                }
            }
        }

        let cells = csvs.get_cells();
        let col = &mut cells.columns[self.w_pointer];

        let mut indices = Self::matching_column_rows(col, &val, insensitive);

        if indices.len() == 0 {
            print_bottom!(
                self,
                "No instance of '{}' in '{}'",
                val, cells.header[self.w_pointer].content
            );
            return;
        }

        if filter {
            Self::apply_column_filter(col, &indices);
            indices = (0..indices.len()).collect();
            // the sheet shrinks if this was its longest column
            self.num_rows = cells.visible_rows();

            cells.set_w_cell(self.w_pointer, 0);
            cells.written = true;
            self.draw_screen(cells);
        }

        // hide cursor
        self.push_str_to_frame("\x1b[?25l");

        // - read from stdin: 'n', 'b', or 'esc'
        // - print nth/n at bottom or "No rows in {col_name} contain {val}"
        // - shift self.h_pointer with 'n' or 'b'
        // - return to normal functionality with 'esc'
        let mut idx = {
            // start with the closest example of the value
            let mut i = 0usize;
            let idx = indices[i];
            let mut diff = self.h_pointer.max(idx) - self.h_pointer.min(idx);
            for _ in 1..indices.len() {
                let idx = indices[i + 1];
                let cur_diff = self.h_pointer.max(idx) - self.h_pointer.min(idx);
                if cur_diff < diff {
                    diff = cur_diff;
                    i += 1;
                } else {
                    break;
                }
            } 

            i
        };
        self.draw_col_find(cells, idx, &indices, "");

        let mut buf = [0u8; 1];
        loop {
            match poll_stdin(&mut buf) {
                Ok(PollEvent::Sig) => {
                    match check_flags() {
                        SigFlag::Winch => self.set_w_h(csvs),
                        SigFlag::Int | SigFlag::Quit => continue, // will never catch
                        SigFlag::Non => break, // must've been quit/int
                    }
                }
                Ok(PollEvent::Data(0)) => continue,
                Ok(PollEvent::Data(_)) => {
                    match buf {
                        [b'n'] => {
                            idx = (idx + 1) % indices.len();
                            self.draw_col_find(
                                csvs.get_cells(), 
                                idx, 
                                &indices,
                                ""
                            );
                            buf[0] = 0u8;
                        }
                        [b'b'] => {
                            if idx == 0 {
                                idx = indices.len() - 1;
                            } else {
                                idx -= 1;
                            }
                            self.draw_col_find(
                                csvs.get_cells(), 
                                idx, 
                                &indices,
                                ""
                            );
                            buf[0] = 0u8;
                        }
                        _   => break,
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

        self.draw_focused_content();
        self.flush();
    }

    fn new_column(&mut self, cells: &mut Cells, name: &str) {
        let name = Self::trim_quotes(name);
       
        let loc = self.w_pointer + 1;

        // mirror the focused column's layout, so hidden and padding
        // rows (from a filter, sift or unique) line up with the new column
        let mut new_col = Column::new();
        let focused = &cells.columns[self.w_pointer];
        new_col.cells = (0..focused.cells.len()).map(|_| Cell::new("")).collect();
        new_col.indices = focused.indices.clone();
        new_col.padding = focused.padding;
        cells.insert_column(loc, new_col);
        
        let col_name = Cell::new(&name);
        cells.insert_col_name(loc, col_name);
        
        // update col ids
        cells.increment_col_ids();

        // make sure each col_id's width
        // matches the width of the column below
        for i in loc..cells.col_ids.len() {
            cells.col_ids[i].width = cells.header[i].width;
        }

        cells.set_w_cell(loc, self.h_pointer);
        cells.written = true;
        
        self.num_cols += 1;
        self.set_w_pointer(loc);
        self.draw_screen(cells);
        self.set_focused("");
        self.draw_focused_content();
        self.flush();
    }

    fn remove_column(&mut self, csvs: &mut Csvs) {
        if self.num_cols == 1 {
            print_bottom!(
                self,
                "\x1b[?25lCannot remove the only column."
            );
            return;
        }
        // confirm remove
        let col_name = csvs
            .get_cells()
            .header[self.w_pointer]
            .content
            .clone();
        print_bottom!(
            self,
            "\x1b[?25lConfirm remove column '{}' with 'y': ",
            col_name
        );
        self.flush();
        let mut buf = [0u8; 1];
        loop {
            match poll_stdin(&mut buf) {
                Ok(PollEvent::Sig) => {
                    match check_flags() {
                        SigFlag::Winch => self.set_w_h(csvs),
                        SigFlag::Int | SigFlag::Quit => continue, // will never catch
                        SigFlag::Non => break, // must've been quit/int
                    }
                }
                Ok(PollEvent::Data(0)) => continue,

                Ok(PollEvent::Data(n)) => {
                    match &buf[..n] {
                        [b'y'] | [b'Y']  => {
                            let cells = csvs.get_cells();
                            cells.columns.remove(self.w_pointer);
                            cells.header.remove(self.w_pointer);
                            cells.col_ids.pop();
                            for i in self.w_pointer..cells.col_ids.len() {
                                cells.col_ids[i].width = cells.header[i].width;
                            }

                            cells.written = true;
                            self.num_cols -= 1;
                            self.w_pointer -= self.w_pointer.saturating_sub(self.num_cols - 1);
                            cells.w_cell = (self.w_pointer, self.h_pointer);
                            self.draw_screen(cells);
                            
                            print_bottom!(
                                self,
                                "Removed column '{}'",
                                col_name
                            );

                            break;
                        }
                        _ => {
                            print_bottom!(
                                self,
                                "Column '{}' was not removed.",
                                col_name
                            );
                            break;
                        }
                    }
                }
                _ => {
                    cmd_err::print(CmdErr::StdinErr("col remove"), self.height);
                    break;
                }
            }
        }
    }

    fn group_columns(&mut self, cells: &mut Cells, list: &str) {
        let mut columns = Self::tokenize(list, ',').into_iter();
        // columns.next() for sure has a value here
        let p_col_name = columns.next().unwrap();

        let p_col_idx = match cells.get_col_idx(p_col_name) {
            Ok(idx) => idx,
            Err(e)      => {
                cmd_err::print(
                    e, self.height
                );
                return;
            }
        };

        // collect ids before removing the columns,
        // in case of an error
        let mut ids = Vec::<usize>::with_capacity(columns.len());

        while let Some(name) = columns.next() {
            match cells.get_col_idx(name) {
                Ok(idx) => ids.push(idx),
                Err(e) => {
                    cmd_err::print(
                        e, self.height
                    );
                    return;
                }
            }
        }
            
        // a column can only be grouped once, and not with itself
        let mut seen = Vec::<usize>::with_capacity(ids.len());
        for idx in ids {
            if idx != p_col_idx && !seen.contains(&idx) {
                seen.push(idx);
            }
        }
        let ids = seen;

        cells.w_cell().is_focused = false;

        // pull the grouped columns out, back to front so indices stay valid
        let mut sorted = ids.clone();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        let mut pulled = Vec::<(usize, Column, Cell)>::with_capacity(ids.len());
        for idx in sorted {
            let col = cells.columns.remove(idx);
            let col_name = cells.header.remove(idx);
            pulled.push((idx, col, col_name));
        }

        // then put them back after the primary column, in the listed order
        let dec = ids.iter().filter(|&&idx| idx < p_col_idx).count();
        let p_col_idx = p_col_idx - dec;
        for (i, idx) in ids.iter().enumerate() {
            let pos = pulled.iter().position(|(j, _, _)| j == idx).unwrap();
            let (_, col, col_name) = pulled.swap_remove(pos);
            cells.columns.insert(p_col_idx + 1 + i, col);
            cells.header.insert(p_col_idx + 1 + i, col_name);
        }
        for i in 0..cells.col_ids.len() {
            cells.col_ids[i].width = cells.columns[i].width;
        }

        // columns left of the focus changed if any grouped column came
        // from before the primary, so draw_from_column won't cover them
        if self.w_pointer != p_col_idx || dec > 0 {
            self.set_w_pointer(p_col_idx);
            self.draw_screen(cells);
        } else {
            self.draw_from_column(cells);
        }

        self.draw_focused_content();
        self.flush();
        cells.written = true;
    }

    fn show_unique_column_values(&mut self, csvs: &mut Csvs) {
        let cells = csvs.get_cells();
        cells.w_cell().is_focused = false;
        let orig = &mut cells.columns[self.w_pointer];
        let uq_len = orig.make_unique(); 
        // the sheet shrinks if this was its longest column
        self.num_rows = cells.visible_rows();
        self.clamp_view();
        
        let values = "values";
        print_bottom!(
            self,
            "\x1b[?25l{} unique {} in '{}'",
            uq_len, 
            &values[0..6-1usize.saturating_sub((uq_len != 1) as usize)], 
            cells.header[self.w_pointer].content
        );
        
        cells.set_w_cell(self.w_pointer, self.h_pointer);
        self.draw_from_column(cells);
        self.flush();
        cells.written = true;
    }

    fn sort_indices(col: &mut Column, dir: Sort) {
        let visible_len = col.len().saturating_sub(col.padding).min(col.indices.len());
        match dir {
            Sort::AscAlph => 
                col.indices[..visible_len].sort_by(|&i, &j| {
                    let a = {
                        match col.cells[i].view() {
                            "" => &format!("{}a", col.cells[j].view()),
                            _ => col.cells[i].view(),
                        }
                    };
                    let b = {
                        match col.cells[j].view() {
                            "" => &format!("{}a", col.cells[i].view()),
                            _ => col.cells[j].view(),
                        }
                    };
                    a.cmp(&b)
                }),
            Sort::DescAlph => 
                col.indices[..visible_len].sort_by(|&i, &j| {
                    let a = col.cells[i].view();
                    let b = col.cells[j].view();
                    b.cmp(&a)
                }),
            Sort::AscNum => {
                col.indices[..visible_len].sort_by(|&i, &j| {
                    let a = match col.cells[i].content.parse::<f32>() {
                        Ok(num) => num,
                        Err(_)  => Self::extract_embedded_float(col.cells[i].view(), f32::MAX),
                    };
                    let b = match col.cells[j].content.parse::<f32>() {
                        Ok(num) => num,
                        Err(_)  => Self::extract_embedded_float(&col.cells[j].view(), f32::MAX),
                    };
                    a.total_cmp(&b)
                })
            }
            Sort::DescNum => {
                col.indices[..visible_len].sort_by(|&i, &j| {
                    let a = match col.cells[i].content.parse::<f32>() {
                        Ok(num) => num,
                        Err(_)  => Self::extract_embedded_float(&col.cells[i].view(), f32::MIN),
                    };
                    let b = match col.cells[j].content.parse::<f32>() {
                        Ok(num) => num,
                        Err(_)  => Self::extract_embedded_float(&col.cells[j].view(), f32::MIN),
                    };
                    b.total_cmp(&a)
                })
            }
        }
    }

    fn extract_embedded_float(content: &str, _default: f32) -> f32 {
        let mut num = String::new();
        let mut found = false;
        let mut dec = false;
        let mut e = false;
        let mut eneg = false;
        for c in content.chars() {
            match c {
                '.' => {
                    if !found {
                        found = true;
                        dec = true;
                        num.push(c);
                    } else {
                        if !dec {
                            dec = true;
                            num.push(c);
                        } else {
                            break;
                        }
                    }
                }
                '+' => {
                    if !found {
                        found = true;
                        num.push(c);
                    } else {
                        break;
                    }
                }
                '-' => {
                    if !found {
                        found = true;
                        num.push(c);
                    } else {
                        if e && !eneg {
                            eneg = true;
                            num.push(c);
                        } else {
                            break;
                        }
                    }
                }
                'e' => {
                    if found {
                        if !e {
                            e = true;
                            num.push(c);
                        } else {
                            break;
                        }
                    }
                }
                _ => {
                    if c.is_ascii_digit() {
                        num.push(c);
                        if !found { found = true; }
                    } else {
                        if found { break; }
                    }
                }
            }
        }

        // sanity check
        if found {
            match num.parse::<f32>() {
                Ok(f) => return f,
                Err(_) => return _default,
            }
        } else {
            return _default;
        }
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
        let col = &mut cells.columns[self.w_pointer];
        col.revert();
        cells.set_w_cell(self.w_pointer, self.h_pointer);
        cells.written = true;
        self.draw_from_column(cells);
        self.draw_focused_content();
        self.flush();
    }

    fn col_fillna(&mut self, cells: &mut Cells, val: &str) {
        let val = Self::trim_quotes(val);
        let col = &mut cells.columns[self.w_pointer];
        for cell in &mut col.cells {
            if &cell.content == "" {
                cell.content.push_str(&val);
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

        let col = &mut cells.columns[self.w_pointer];
        for cell in &mut col.cells {
            // Check if the cell content contains the target substring
            if cell.content.contains(&t) {
                // Replace the substring and reassign to cell.content
                cell.content = cell.content.replace(&t, &n);
            }
        }
        cells.written = true;
        self.draw_from_column(cells);
        self.draw_focused_content();
        self.flush();
    }

    fn add_vals_in_col(&mut self, cells: &mut Cells) {
        let col = cells.get_column(self.w_pointer);
        let mut valid = 0;
        let mut sum: f64 = 0.0;
        for cell in &mut col.cells {
            match cell.content.parse::<f64>() {
                Ok(f) => {
                    sum += f;
                    valid += 1;
                }
                Err(_) => continue,
            }
        }
        print_bottom!(
            self,
            "{} (parsed {} out of {} cells)",
            sum,
            valid,
            col.len()
        );
    }

    fn col_cmp(&mut self, csvs: &mut Csvs, other: &str, diff: bool, options: &[&str]) {
        let mut filter = false;
        for opt in options {
            match *opt {
                "-f" | "--filter" => filter = true,
                _ => {
                    cmd_err::print(
                        CmdErr::InvalidOption(opt),
                        self.height
                    );
                    return;
                }
            }
        }

        let cells = csvs.get_cells();
        let other_col_idx = match cells.get_col_idx(other) {
            Ok(idx) => idx,
            Err(e) => {
                cmd_err::print(
                    e, self.height
                );
                return;
            }
        };

        let other_col = cells.get_column(other_col_idx);
        let mut other_values = HashSet::<String>::new();
        for i in 0..other_col.len() {
            let content = &other_col.get_cell(i).content;
            if content != "" {
                other_values.insert(content.clone());
            }
        }

        // every row that matches, and the first row of each matching value
        let cur_col = cells.get_column(self.w_pointer);
        let mut matching_rows = Vec::<usize>::new();
        let mut cmp_values = Vec::<usize>::new();
        let mut seen = HashSet::<&str>::new();
        for i in 0..cur_col.visible_len() {
            let cell = cur_col.view_cell(i);
            if cell == "" || other_values.contains(cell) == diff {
                continue;
            }
            matching_rows.push(i);
            if seen.insert(cell) {
                cmp_values.push(i);
            }
        }

        if filter && !matching_rows.is_empty() {
            let col = cells.get_column(self.w_pointer);
            Self::apply_column_filter(col, &matching_rows);
            for v in cmp_values.iter_mut() {
                *v = matching_rows.binary_search(v).unwrap();
            }
            // the sheet shrinks if this was its longest column
            self.num_rows = cells.visible_rows();

            cells.set_w_cell(self.w_pointer, 0);
            cells.written = true;
            self.draw_screen(cells);
        }

        // get value closest to focus
        let mut diff_idx = {
            let mut i = 0usize;
            let mut dv = match cmp_values.get(i) {
                Some(value) => *value,
                None => {
                    print_bottom!(
                        self,
                        "No matching cells"
                    );
                    self.flush();
                    return;
                }
            };

            let mut diff = dv.max(self.h_pointer) - dv.min(self.h_pointer);
            for _ in 1..cmp_values.len() {
                dv = cmp_values[i + 1];
                let cur_diff = dv.max(self.h_pointer) - dv.min(self.h_pointer);
                if cur_diff < diff {
                    diff = cur_diff;
                    i += 1;
                } else {
                    break;
                }
            }

            i
        };
        self.push_str_to_frame("\x1b[?25l");
        let mess = {
            if diff {
                format!("values not in {}", other)
            } else {
                format!("values in {}", other)
            }
        };
        self.draw_col_find(
            csvs.get_cells(),
            diff_idx,
            &cmp_values,
            &mess
        );
        let mut buf = [0u8; 1];
        loop {
            match poll_stdin(&mut buf) {
                Ok(PollEvent::Sig) => {
                    match check_flags() {
                        SigFlag::Winch => self.set_w_h(csvs),
                        SigFlag::Int | SigFlag::Quit => continue, // will never catch
                        SigFlag::Non => break, // must've been quit/int
                    }
                }

                Ok(PollEvent::Data(0)) => continue,

                Ok(PollEvent::Data(n)) => {
                    match &buf[..n] {
                        [b'n'] => {
                            diff_idx = (diff_idx + 1) % cmp_values.len();
                            self.draw_col_find(
                                csvs.get_cells(),
                                diff_idx,
                                &cmp_values,
                                &mess
                            );
                            buf[0] = 0u8;
                        }
                        [b'b'] => {
                            if diff_idx == 0 {
                                diff_idx = cmp_values.len() - 1;
                            } else {
                                diff_idx -= 1;
                            }
                            self.draw_col_find(
                                csvs.get_cells(),
                                diff_idx,
                                &cmp_values,
                                &mess
                            );
                            buf[0] = 0u8;
                        }
                        _ => break,
                    }
                }
                _ => {
                    if diff {
                        cmd_err::print(CmdErr::StdinErr("col diff"), self.height);
                    } else {
                        cmd_err::print(CmdErr::StdinErr("col sim"), self.height);
                    } 
                    break;
                }
            }
        }
        self.draw_focused_content();
        self.flush();
    }

    // blank cells stay blank
    fn col_pre(&mut self, cells: &mut Cells, pre: &str) {
        let pre = Self::trim_quotes(pre);
        let col = cells.get_column(self.w_pointer);
        for i in 0..col.len() {
            let cell = col.get_cell(i);
            if cell.content.is_empty() {
                continue;
            }
            cell.content = format!("{}{}", pre, cell.content);
        }
        cells.written = true;
        self.draw_from_column(cells);
        self.draw_focused_content();
        self.flush();
    }

    // blank cells stay blank
    fn col_app(&mut self, cells: &mut Cells, app: &str) {
        let app = Self::trim_quotes(app);
        let col = cells.get_column(self.w_pointer);
        for i in 0..col.len() {
            let cell = col.get_cell(i);
            if cell.content.is_empty() {
                continue;
            }
            cell.content = format!("{}{}", cell.content, app);
        }
        cells.written = true;
        self.draw_from_column(cells);
        self.draw_focused_content();
        self.flush();
    }

    fn col_dup(&mut self, cells: &mut Cells) {
        let col_name = cells.header[self.w_pointer].clone();
        let col = cells.get_column(self.w_pointer);
        let id_width = col.width;
        let new_col = col.clone();
        let next = self.w_pointer + 1;
        
        cells.insert_column(next, new_col);
        cells.insert_col_name(next, col_name);
        cells.increment_col_ids();
        cells.col_ids[next].width = id_width;
        cells.written = true;

        self.num_cols += 1;
        self.draw_from_column(cells);
        print_bottom!(
            self,
            "Duplicated column '{}'",
            self.num_rows
        );
        self.flush();
    }

    fn col_filterby(&mut self, cells: &mut Cells, cols: &str) {
        let col_idx = match cells.get_col_idx(cols) {
            Err(e) => {
                cmd_err::print(
                    e, self.height
                );
                return;
            }
            Ok(i) => i,
        };
        let col = cells.get_column(col_idx);
        let mut u_vals = Vec::<String>::new();
        for i in 0..col.len() {
            let cell = col.get_cell(i);
            let mut u = true;
            for uv in &u_vals {
                if uv == &cell.content {
                    u = false;
                    break;
                }
            }
            if u {
                u_vals.push(cell.content.clone());
            }
        }
        
        let mut num_filt = 0;
        let cur = cells.get_column(self.w_pointer);
        for i in 0..cur.len() {
            let cell = cur.get_cell(i);
            for uv in &u_vals {
                if uv == &cell.content {
                    cell.content.clear();
                    num_filt += 1;
                }
            }
        }

        cells.written = true;
        self.draw_from_column(cells);
        print_bottom!(
            self,
            "Filtered {} rows",
            num_filt
        );
        self.flush();
    }

    fn col_up(&mut self, cells: &mut Cells) {
        let col = cells.get_column(self.w_pointer);
        for i in 0..col.len() {
            let cell = col.get_cell(i);
            cell.content = cell.content.to_uppercase();
        }
        cells.written = true;
        self.draw_from_column(cells);
        self.draw_focused_content();
        self.flush();
    }
    
    fn col_lo(&mut self, cells: &mut Cells) {
        let col = cells.get_column(self.w_pointer);
        for i in 0..col.len() {
            let cell = col.get_cell(i);
            cell.content = cell.content.to_lowercase();
        }
        cells.written = true;
        self.draw_from_column(cells);
        self.draw_focused_content();
        self.flush();
    }
    // row functions
    //
    fn print_row_count(&mut self) {
        print_bottom!(
            self,
            "{} rows",
            self.num_rows
        );
    }

    fn insert_row(&mut self, cells: &mut Cells, count: usize) {
        let start = self.h_pointer + 1;

        let new_row_len = self.num_rows + count;
        
        for col in &mut cells.columns {
            let new_cells = (0..count).map(|_| Cell::new("")).collect();
            col.insert_new_cells(start, new_cells);
        }
        cells.written = true;
        self.num_rows = new_row_len;
        self.changed = WinChange::Rows;
        self.show_csv(cells);
    }

    fn duplicate_row(&mut self, cells: &mut Cells, count: usize) {
        let start = self.h_pointer + 1;
        let end = start + count;

        for col in &mut cells.columns {
            let mut clone = col.get_cell(self.h_pointer).clone();
            clone.is_focused = false;
            let new_cells = (start..end).map(|_| clone.clone()).collect();
            col.insert_new_cells(start, new_cells);
        }

        cells.written = true;
        self.num_rows += count;
        self.changed = WinChange::Rows;

        self.show_csv(cells);
    }

    fn delete_row(&mut self, csvs: &mut Csvs) {
        if self.num_rows == 1 {
            print_bottom!(
                self,
                "\x1b[?25lCannot delete the only row."
            );
            return;
        }
        print_bottom!(
            self,
            "\x1b[?25lConfirm remove row {:04X} with 'y': ",
            self.h_pointer
        );
        self.flush();
        let mut buf = [0u8; 1];
        loop {
            match poll_stdin(&mut buf) {
                Ok(PollEvent::Sig) => {
                    match check_flags() {
                        SigFlag::Winch => self.set_w_h(csvs),
                        SigFlag::Int | SigFlag::Quit => continue, // will never catch
                        SigFlag::Non => break, // must've been quit/int
                    }
                }
                Ok(PollEvent::Data(0)) => continue,

                Ok(PollEvent::Data(n)) => {
                    match &buf[..n] {
                        [b'y'] | [b'Y']  => {
                            let cells = csvs.get_cells();
                            cells.set_w_cell(self.w_pointer, self.h_pointer.saturating_sub(1));
                            for col in &mut cells.columns {
                                col.remove_cell(self.h_pointer);
                            }
                            cells.written = true;
                           
                            self.num_rows -= 1;
                           
                            self.draw_screen(cells);
                            
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
                    cmd_err::print(CmdErr::StdinErr("row delete"), self.height);
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

    fn compute_valid_hex(c: char) -> Result<usize, CmdErr<'static>> {
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
        Err(CmdErr::InvalidHex(c))
    }

    fn hex_to_dec<'a>(hex: &'a str) -> Result<usize, CmdErr<'a>> {
        let digits = Self::trim_quotes(hex);
        let mut num = 0usize;
        for c in digits.chars() {
            let d = Self::compute_valid_hex(c)?;
            num = num
                .checked_mul(16)
                .and_then(|n| n.checked_add(d))
                .ok_or(CmdErr::InvalidDec(hex))?;
        }
        Ok(num)
    }

    fn str_to_dec<'a>(s: &'a str) -> Result<usize, CmdErr<'a>> {
        match s.chars().next() {
            Some('\'') | Some('"') => Self::hex_to_dec(s),
            _       => {
                match s.parse::<usize>() {
                    Ok(val) => Ok(val),
                    Err(_)  => return Err(CmdErr::InvalidDec(s)),
                }
            }
        }
    }

    fn parse_row_range<'a>(&mut self, range: &'a str) -> Result<(usize, usize), CmdErr<'a>> {
        // range can be expressed as:
        // - a single index (e.g. '63' or 99)
        // - two indices separated by a hyphen (e.g. '104D'-'105F')
        // - an index, a '+', and how many rows to include (e.g. 'A7'+10)
        // quotes refer to printed indices
        let (tokens, delim) = Self::tokenize_range(range);

        // if token is empty str, start at first row;
        // if token is _, start at focused row
        let lo = match tokens[0] {
            "" => 0usize,
            "_" => self.h_pointer,
            _ => Self::str_to_dec(tokens[0])?,
        };

        // if token is empty str, end at last row
        let hi = match tokens.get(1) {
            Some(val) => {
                match *val {
                    "" => self.num_rows.saturating_sub(1),
                    "_" => self.h_pointer,
                    _ => {
                        match Self::str_to_dec(val) {
                            Ok(v) => {
                                match delim {
                                    Some(d) => {
                                        match d {
                                            '-' => v,
                                            '+' => lo + v,
                                            _ => lo,
                                        }
                                    }
                                    None => lo,
                                }
                            }
                            Err(e) => return Err(e),
                        }
                    }
                }
            }
            None => lo,
        };
        
        Ok((lo, hi))
    }

    fn move_rows(&mut self, cells: &mut Cells, range: &str, target: &str) {
        let (t_lo, t_hi) = match self.parse_row_range(range) {
            Ok((l, h)) => (l, h),
            Err(e) => {
                cmd_err::print(e, self.height);
                return;
            }
        };

        let lo = t_lo.min(t_hi);
        let hi = t_hi.max(t_lo);

        if lo >= self.num_rows {
            cmd_err::print(CmdErr::InvalidIndex(lo), self.height);
            return;
        }

        if hi >= self.num_rows {
            cmd_err::print(CmdErr::InvalidIndex(hi), self.height);
            return;
        }

        let target = match Self::str_to_dec(target) {
            Ok(d) => d,
            Err(e) => {
                cmd_err::print(e, self.height);
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

    fn goto_row(&mut self, cells: &mut Cells, loc: Option<&str>) {
        let i = {
            match loc {
                Some(l) => {
                    match Self::str_to_dec(l) {
                        Ok(index) => {
                            if index >= self.num_rows {
                                cmd_err::print(
                                    CmdErr::InvalidIndex(index),
                                    self.height
                                );
                                return;
                            }
                            index
                        }
                        Err(e) => {
                            cmd_err::print(e, self.height);
                            return;
                        }
                    }
                }
                None => self.num_rows.saturating_sub(1),
            } 
        };
        self.set_h_pointer(i);
        self.draw_screen(cells);
        self.draw_focused_content();
        self.flush();
    }

    fn show_row_num(&mut self) {
        self.set_focused(&format!("{}", self.h_pointer));
        self.draw_focused_content();
        self.flush();
    }

    fn add_vals_in_row(&mut self, cells: &mut Cells) {
        let mut sum: f64 = 0.0;
        let mut valid = 0;
        for col in &mut cells.columns {
            match col.get_cell(self.h_pointer).content.parse::<f64>() {
                Ok(n) => {
                    sum += n;
                    valid += 1;
                }
                Err(_) => continue,
            }
        }
        print_bottom!(
            self,
            "{} (parsed {} out of {} cells)",
            sum,
            valid,
            self.num_cols
        );
    }

    // sheet functions
    //
    fn sort_by(&mut self, cells: &mut Cells, col_name: &str, sort_dir: Sort) {
        match cells.get_col_idx(col_name) {
            Err(e) => {
                cmd_err::print(
                    e, self.height
                );
                return;
            }
            Ok(col_idx) => {
                cells.w_cell().is_focused = false;
                let old_indices = cells.columns[col_idx].indices.clone();
                {
                    let mut col = &mut cells.columns[col_idx];
                    Self::sort_indices(&mut col, sort_dir);
                }

                // columns can differ in how their cells are laid out
                // (e.g. one column was reverted after a filter),
                // so apply the sorted row order rather than copying indices
                let order = Self::row_order(&old_indices, &cells.columns[col_idx].indices);
                for i in (0..cells.columns.len()).filter(|&i| i != col_idx) {
                    Self::reorder_rows(&mut cells.columns[i], &order);
                }
                cells.written = true;
                cells.set_w_cell(self.w_pointer, self.h_pointer);
                self.draw_screen(cells);
                self.draw_focused_content();
                self.flush();
            }
        }
    }

    // for each display row after reordering a column,
    // the display row it came from
    fn row_order(old_indices: &[usize], new_indices: &[usize]) -> Vec<Option<usize>> {
        let mut old_pos = std::collections::HashMap::<usize, usize>::new();
        for (pos, &real) in old_indices.iter().enumerate() {
            old_pos.insert(real, pos);
        }
        new_indices
            .iter()
            .map(|real| old_pos.get(real).copied())
            .collect()
    }

    // rearrange a column's rows to match a reordered column;
    // rows that didn't exist before (None) become padding
    fn reorder_rows(col: &mut Column, order: &[Option<usize>]) {
        let old_indices = col.indices.clone();
        let mut new_indices = Vec::<usize>::with_capacity(order.len());
        let mut used = vec![false; old_indices.len()];
        for from in order {
            match from.and_then(|f| old_indices.get(f).map(|&real| (f, real))) {
                Some((f, real)) => {
                    used[f] = true;
                    new_indices.push(real);
                }
                None => {
                    new_indices.push(col.cells.len());
                    col.cells.push(Cell::new(""));
                    col.padding += 1;
                }
            }
        }
        // keep any rows the other column didn't have
        for (f, &real) in old_indices.iter().enumerate() {
            if !used[f] {
                new_indices.push(real);
            }
        }
        col.indices = new_indices;
    }

    fn sift_by(&mut self, cells: &mut Cells, col_name: &str) {
         match cells.get_col_idx(col_name) {
            Err(e) => {
                cmd_err::print(e, self.height);
                return;
            }
            Ok(col_idx) => {
                let col = &cells.columns[col_idx];
                if (0..col.len().saturating_sub(col.padding)).all(|row| col.view_cell(row) == "") {
                    print_bottom!(self, "No values to sift by in '{}'", cells.header[col_idx].content);
                    return;
                }
                cells.w_cell().is_focused = false;
                let orig_len = self.num_rows;
                let old_indices = cells.columns[col_idx].indices.clone();
                {
                    let col = &mut cells.columns[col_idx];
                    self.num_rows = col.make_unique();
                }

                let order = Self::row_order(&old_indices, &cells.columns[col_idx].indices);
                for i in (0..cells.columns.len()).filter(|&i| i != col_idx) {
                    Self::reorder_rows(&mut cells.columns[i], &order);
                }
                cells.written = true;
                self.set_h_pointer(self.h_pointer);
                cells.set_w_cell(self.w_pointer, self.h_pointer);
                self.draw_screen(cells);
                print_bottom!(
                    self,
                    "Sifted from {} to {} rows",
                    orig_len,
                    self.num_rows
                );
                self.flush();
            }
        }
    }

    fn revert_sheet(&mut self, cells: &mut Cells) {
        cells.w_cell().is_focused = false;
        for col in &mut cells.columns {
            col.revert();
        }
        cells.written = true;
        self.num_rows = cells.num_rows();
        cells.set_w_cell(self.w_pointer, self.h_pointer);
        self.draw_screen(cells);
        self.draw_focused_content();
        self.flush();
    }

    fn slice(&mut self, tokens: Vec<&str>, csvs: &mut Csvs) {
        let mut tokens = tokens.into_iter();
        for _ in 0..2 {
            let _ = tokens.next();
        }
        let or = match tokens.next() {
            Some(o) => o,
            None => {
                cmd_err::print(
                    CmdErr::MissingValue("slice {'col' or 'row'}"),
                    self.height
                );
                return;
            }
        };

       let range = match tokens.next() {
           Some(r) => r,
           None => {
               cmd_err::print(
                   CmdErr::MissingRange("slice"),
                   self.height
                );
               return;
            }
        };

        match or {
            "c" | "cols" => self.slice_cols(&range, csvs),
            "r" | "rows" => self.slice_rows(&range, csvs),
            _ => cmd_err::print(
                    CmdErr::InvalidArg(or),
                    self.height
                ),
        }
    }

    fn slice_cols(&mut self, range: &str, csvs: &mut Csvs) {
        let (cols, delim) = Self::tokenize_range(range);

        // -remove columns
        // -push them into new Cells
        // -create new Context
        // -add Context to Csvs and change handle to it
        //
        let cells = csvs.get_cells();
        cells.w_cell().is_focused = false;
        let ida = match cols[0] {
            "" => 0,
            "_" => self.w_pointer,
            _ => {
                match cells.get_col_idx(cols[0]) {
                    Ok(id) => id,
                    Err(e) => {
                        cmd_err::print(
                            e, self.height
                        );
                        return;
                    }
                }
            }
        };
        let idb = match cols.get(1) {
            Some(c) => {
                let c = match delim {
                    Some(d) => {
                        match d {
                            '-' => *c,
                            '+' => {
                                match Self::str_to_dec(*c) {
                                    Ok(dec) => {
                                        let num = ida + dec;
                                        if num >= cells.header.len() {
                                            cmd_err::print(
                                                CmdErr::InvalidIndex(num),
                                                self.height
                                            );
                                            return;
                                        }
                                        &cells.col_ids[num].content
                                    }
                                    Err(e) => {
                                        cmd_err::print(
                                            e, self.height
                                        );
                                        return;
                                    }
                                }
                            }
                            _ => cols[0],
                        }
                    }
                    None => cols[0],
                };
                match c {
                    "" => self.num_cols - 1,
                    "_" => self.w_pointer,
                    _ => {
                        match cells.get_col_idx(c) {
                            Ok(id) => id,
                            Err(e) => {
                                cmd_err::print(
                                    e, self.height
                                );
                                return;
                            }
                        }
                    }
                }
            }
            None => ida,
        };

        let (ida, idb) = (ida.min(idb), ida.max(idb));
        if idb >= cells.num_cols() {
            cmd_err::print(CmdErr::InvalidIndex(idb), self.height);
            return;
        }
        if idb - ida + 1 == cells.num_cols() {
            print_bottom!(self, "Cannot slice out every column");
            return;
        }

        cells.slices += 1;
        let mut fn_split = Self::tokenize(&cells.filename, '.');
        let first = format!(
                        "{}_{}",
                        fn_split[0],
                        cells.slices
                    );
        fn_split[0] = &first;
        let slice_name = fn_split.join(".");

        let col_len = idb - ida + 1;
        let col_ids: Vec<Cell> = make_col_ids(col_len);
        let mut new_cells = Cells::new(
            slice_name,
            cells.delim,
            Vec::<Cell>::new(),
            col_ids,
            col_len
        );

        // the slice is a copy of the rows on screen
        let shown = cells.visible_rows();
        let mut new_header = Vec::<Cell>::new();
        for _ in ida..=idb {
            let col = cells.columns.remove(ida);
            let mut new_col = Column::new();
            new_col.width = col.width;
            if shown > 0 {
                new_col.cells = col.copy_shown(0, shown - 1);
            }
            new_col.indices = (0..new_col.cells.len()).collect();
            new_cells.push_column(new_col);
            
            let name = cells.header.remove(ida);
            new_header.push(name);
            cells.col_ids.pop();
        }
        new_cells.header = new_header;
        // realign original cells
        for i in ida..cells.col_ids.len() {
            cells.col_ids[i].width = cells.header[i].width;
        }
        cells.written = true;
        new_cells.written = true;

        // keep the original sheet's focus inside its remaining columns
        self.num_cols = cells.num_cols();
        self.clamp_view();
        cells.w_cell = (self.w_pointer, self.h_pointer);
        csvs.save_context(self);
        
        let new_id = csvs.num_contexts();
        let new_context = Context::new(
                            new_id.clone(),
                            new_cells
                        );
        csvs.push_context(new_context);
        csvs.set_handle(new_id);

        self.set_context(csvs.get_context());
        self.draw_context(csvs);
        self.changed = WinChange::Screen;
        self.show_csv(csvs.get_cells());
    }

    fn slice_rows(&mut self, range: &str, csvs: &mut Csvs) {
        // ranges index the rows on screen
        self.num_rows = csvs.get_cells().visible_rows();
        let (st, end) = match self.parse_row_range(range) {
            Ok((s,t)) => (s, t),
            Err(e) => {
                cmd_err::print(
                        e,
                        self.height
                );
                return;
            }
        };

        // -remove rows
        // -push them into new Columns with old col names
        // -push new Columns into new Cells
        // -create new Context
        // -add Context to Csvs and change handle to it
        //
        let (st, end) = (st.min(end), st.max(end));
        if end >= self.num_rows {
            cmd_err::print(CmdErr::InvalidIndex(end), self.height);
            return;
        }
        if end - st + 1 == self.num_rows {
            print_bottom!(self, "Cannot slice out every row");
            return;
        }

        let cells = csvs.get_cells();

        cells.w_cell().is_focused = false;
        cells.slices += 1;
        let mut fn_split = Self::tokenize(&cells.filename, '.');
        let first = format!(
                        "{}_{}",
                        fn_split[0],
                        cells.slices
                    );
        fn_split[0] = &first;
        let slice_name = fn_split.join(".");

        let mut new_cells = Cells::new(
            slice_name,
            cells.delim,
            Cells::clone_cell_row(&cells.header),
            Cells::clone_cell_row(&cells.col_ids),
            self.num_cols
        );

        // the slice is a copy of the rows on screen
        for col in &mut cells.columns {
            let mut new_col = Column::new();
            new_col.width = col.width;
            new_col.cells = col.copy_shown(st, end);
            new_col.indices = (0..new_col.cells.len()).collect();
            col.remove_shown(st, end, self.num_rows);
            new_cells.push_column(new_col);
        }
        cells.written = true;
        new_cells.written = true;

        // keep the original sheet's focus inside its remaining rows
        self.num_rows = cells.visible_rows();
        self.clamp_view();
        cells.w_cell = (self.w_pointer, self.h_pointer);
        csvs.save_context(self);
        
        let new_id = csvs.num_contexts();
        let new_context = Context::new(
                            new_id.clone(),
                            new_cells
                        );
        csvs.push_context(new_context);
        csvs.set_handle(new_id);

        self.set_context(csvs.get_context());
        self.draw_context(csvs);
        self.changed = WinChange::Screen;
        self.show_csv(csvs.get_cells());
    }

    fn splice(&mut self, tokens: Vec<&str>, csvs: &mut Csvs) {
        let mut tokens = tokens.into_iter();
        for _ in 0..2 {
            let _ = tokens.next();
        }
        let or = match tokens.next() {
            Some(o) => o,
            None => {
                cmd_err::print(
                    CmdErr::MissingValue("splice {'col' or 'row'}"),
                    self.height
                );
                return;
            }
        };

        let con_id = match tokens.next() {
            Some(r) if r != "" => {
                match Self::resolve_sheet(csvs, r) {
                    Ok(num) => {
                        if csvs.handle == num {
                            cmd_err::print(
                                CmdErr::SameCon,
                                self.height
                            );
                            return;
                        }
                        num
                    }
                    Err(e) => {
                        cmd_err::print(
                            e, self.height
                        );
                        return;
                    }
                }
            }
            _ => {
                cmd_err::print(
                    CmdErr::MissingConId("splice"),
                    self.height
                );
                return;
            }
        };
    
        match or {
            "c" | "cols" => self.splice_cols(
                                con_id, 
                                self.w_pointer + 1, 
                                csvs
                            ),
            "r" | "rows" => self.splice_rows(
                                con_id, 
                                self.h_pointer,
                                csvs
                            ),
            _ => cmd_err::print(
                    CmdErr::InvalidArg(or),
                    self.height
                ),
        }
    }

    fn splice_cols(&mut self, con_id: usize, at_col: usize, csvs: &mut Csvs) {
        // insert columns from csvs.context[at_col]
        // after the focused column
        let mut src_con = csvs.remove_context(con_id);
        let src_cells = src_con.cells();
        src_cells.w_cell().is_focused = false;
        let dest_cells = csvs.get_cells();
        
        let num_cols = src_cells.num_cols();
        let max_rows = dest_cells
            .num_rows()
            .max(src_cells.num_rows());
        
        let st = at_col;
        let end = st + num_cols;
        for i in st..end {
            let mut col = src_cells
                .columns
                .remove(0);
            col.reindex();
            let col_name = src_cells
                .header
                .remove(0);

            dest_cells.insert_column(i, col);
            dest_cells.insert_col_name(i, col_name);
            dest_cells.increment_col_ids();
        }
        // realign cell widths
        for i in st..dest_cells.num_cols() {
            dest_cells.col_ids[i].width = dest_cells.header[i].width;
        }

        for col in &mut dest_cells.columns {
            while col.len() < max_rows {
                col.push_cell(Cell::new(""));
            }
        }
        
        dest_cells.written = true;
        
        self.num_cols += num_cols;
        self.num_rows = max_rows;
        self.draw_screen(dest_cells);

        self.draw_context(csvs);
        self.draw_focused_content();
        self.flush();
    }

    fn splice_rows_into(dest_cells: &mut Cells, src_cells: &mut Cells, at_row: usize) {
        let num_rows = src_cells.num_rows();
        let st = at_row + 1;
        let dest_num_rows = dest_cells.num_rows();
        for i in 0..dest_cells.num_cols() {
            let col = &mut dest_cells.columns[i];
            col.reindex();
            let dest_name = dest_cells.header[i].content.clone();
            let mut j = 0usize;
            let mut matched = false;
            while j < src_cells.num_cols() {
                if src_cells.header[j].content == dest_name {
                    let mut src_col = src_cells.columns.remove(j);
                    src_col.reindex();
                    let _ = src_cells.header.remove(j);
                    if st >= dest_num_rows {
                        col.cells.append(&mut src_col.cells);
                    } else {
                        let mut end_of_col = col.cells.split_off(st);
                        col.cells.append(&mut src_col.cells);
                        col.cells.append(&mut end_of_col);
                    }
                    col.indices = (0..col.len()).collect();
                    matched = true;
                    break;
                }
                j += 1;
            }

            // if no matching src_col, then we still need to pad dest_col
            if !matched {
                if st >= dest_num_rows {
                    for _ in 0..num_rows {
                        col.push_cell(Cell::new(""));
                    }
                } else {
                    let mut end_of_col = col.cells.split_off(st);
                    for _ in 0..num_rows {
                        col.push_cell(Cell::new(""));
                    }
                    col.cells.append(&mut end_of_col);
                }
                col.indices = (0..col.len()).collect();
            }
        }

        // should only be the dissimilar ones left
        while src_cells.num_cols() > 0 {
            let mut dest_col = Column::new();
            let mut src_col = src_cells.columns.remove(0);
            src_col.reindex();
            let src_name = src_cells.header.remove(0);
            dest_cells.header.push(src_name);
            dest_cells.increment_col_ids();
            for _ in 0..st {
                dest_col.push_cell(Cell::new(""));
            }
            dest_col.cells.append(&mut src_col.cells);
            while dest_col.len() < dest_cells.num_rows() {
                dest_col.push_cell(Cell::new(""));
            }
            dest_col.indices = (0..dest_col.len()).collect();
            dest_cells.push_column(dest_col);
        }

        Self::pad_columns_to_same_row_count(dest_cells);
    }

    fn pad_columns_to_same_row_count(cells: &mut Cells) {
        let row_count = cells
            .columns
            .iter()
            .map(|col| col.len())
            .max()
            .unwrap_or(0);

        for col in &mut cells.columns {
            while col.len() < row_count {
                col.push_cell(Cell::new(""));
            }
            col.indices = (0..col.len()).collect();
        }
    }

    fn splice_rows(&mut self, con_id: usize, at_row: usize, csvs: &mut Csvs) {
        // insert rows from csvs.context[[con_id]
        // after the focused column;
        // if the column names are different,
        // create new columns
        let mut src_con = csvs.remove_context(con_id);
        let src_cells = src_con.cells();
        src_cells.w_cell().is_focused = false;
        let dest_cells = csvs.get_cells();
       
        Self::splice_rows_into(dest_cells, src_cells, at_row);
        
        dest_cells.written = true;
        
        self.num_rows = dest_cells.num_rows();
        self.num_cols = dest_cells.num_cols();
        self.draw_screen(dest_cells);

        self.draw_context(csvs);
        self.draw_focused_content();
        self.flush();
    }

    fn replace_in_sheet(&mut self, tokens: Vec<&str>, cells: &mut Cells) {
        let replace = match tokens.get(2) {
            Some(r) => Self::trim_quotes(r),
            None => {
                cmd_err::print(
                    CmdErr::MissingValue("sh replace"),
                    self.height
                );
                return;
            }
        };

        let with = match tokens.get(3) {
            Some(w) => Self::trim_quotes(w),
            None => {
                cmd_err::print(
                    CmdErr::MissingValue("sh replace"),
                    self.height
                );
                return;
            }
        };

        for i in 0..cells.num_cols() {
            let col = cells.get_column(i);
            for j in 0..col.len() {
                let cell = col.get_cell(j);
                // Check if the cell content contains the target substring
                if cell.content.contains(&replace) {
                    // Replace the substring and reassign to cell.content
                    cell.content = cell.content.replace(&replace, &with);
                }
            }
        }

        cells.written = true;
        self.draw_from_column(cells);
        self.draw_focused_content();
        self.flush();
    }

    fn apply_sheet_filter(cells: &mut Cells, visible_rows: &[usize]) {
        // rows hidden by an earlier filter stay hidden, after the new ones
        let row_count = cells.visible_rows();
        let mut is_visible = vec![false; row_count];
        for &row in visible_rows {
            if row < row_count {
                is_visible[row] = true;
            }
        }

        let hidden_rows = (0..row_count)
            .filter(|&row| !is_visible[row])
            .collect::<Vec<usize>>();
        let padding = hidden_rows.len();

        for col in &mut cells.columns {
            let previous_indices = col.indices.clone();
            let mut new_indices = Vec::with_capacity(row_count + padding);

            new_indices.extend(
                visible_rows
                    .iter()
                    .filter_map(|&row| previous_indices.get(row).copied()),
            );

            let padding_start = col.cells.len();
            for offset in 0..padding {
                new_indices.push(padding_start + offset);
                col.cells.push(Cell::new(""));
            }

            new_indices.extend(
                hidden_rows
                    .iter()
                    .filter_map(|&row| previous_indices.get(row).copied()),
            );
            new_indices.extend(previous_indices.iter().skip(row_count).copied());

            col.padding += padding;
            col.indices = new_indices;
        }
    }

    fn matching_sheet_cells(cells: &Cells, term: &str, insensitive: bool) -> Vec<(usize, usize)> {
        let mut matches = Vec::new();
        let row_count = cells.visible_rows();
        for (col_idx, col) in cells.columns.iter().enumerate() {
            for row in 0..row_count.min(col.indices.len()) {
                let content = col.view_cell(row);
                let found = if insensitive {
                    content.to_lowercase().contains(term)
                } else {
                    content.contains(term)
                };
                if found {
                    matches.push((col_idx, row));
                }
            }
        }
        matches
    }

    fn remap_sheet_matches(matches: &mut [(usize, usize)], visible_rows: &[usize]) {
        for (_, row) in matches {
            if let Ok(filtered_row) = visible_rows.binary_search(row) {
                *row = filtered_row;
            }
        }
    }

    fn find_in_sheet(&mut self, cells: &mut Cells, term: &str, options: Vec<&str>) {
        let mut term = Self::trim_quotes(term);
        let mut filter = false;
        let mut insensitive = false;
      
        for i in 3..options.len() {
            match options[i] {
                "-f" | "--filter" => filter = true,
                "-i" | "--case-insensitive" => {
                    term = term.to_lowercase();
                    insensitive = true;
                }
                _ => {
                    cmd_err::print(
                        CmdErr::InvalidOption(options[i]),
                        self.height
                    );
                    return;
                }
            }
        }

        let mut cols_rows = Self::matching_sheet_cells(cells, &term, insensitive);

        if cols_rows.len() == 0 {
            print_bottom!(
                self,
                "'{}' not found in sheet",
                term
            );
            return;
        }

        if filter {
            let mut new_indices = Vec::with_capacity(cols_rows.len());
            for i in 0..cols_rows.len() {
                new_indices.push(cols_rows[i].1);
            }

            new_indices.sort();
            new_indices.dedup();

            Self::remap_sheet_matches(&mut cols_rows, &new_indices);

            cells.w_cell().is_focused = false;
            self.num_rows = new_indices.len();
            Self::apply_sheet_filter(cells, &new_indices);
            cells.set_w_cell(cols_rows[0].0, cols_rows[0].1);
            cells.written = true;
        }
                
        // hide cursor
        self.push_str_to_frame("\x1b[?25l");

        let mut idx = 0;
        self.draw_sheet_find(
            cells, 
            idx, 
            &cols_rows,
            ""
        );
        let mut buf = [0u8; 1];
        loop {
            match poll_stdin(&mut buf) {
                Ok(PollEvent::Sig) => {
                    match check_flags() {
                        SigFlag::Winch => {
                            unsafe {
                                let mut ws: winsize = mem::zeroed();
                                if ioctl(STDOUT_FILENO, TIOCGWINSZ.into(), &mut ws) == 0 {
                                    self.width = ws.ws_col as usize;
                                    self.height = ws.ws_row as usize;
                                }
                            }
                        }
                        SigFlag::Int | SigFlag::Quit => continue, // will never catch
                        SigFlag::Non => break, // must've been quit/int
                    }
                }
                Ok(PollEvent::Data(0)) => continue,
                Ok(PollEvent::Data(n)) => {
                    match &buf[..n] {
                        [b'n'] => {
                            idx = (idx + 1) % cols_rows.len();
                            self.draw_sheet_find(
                                cells, 
                                idx, 
                                &cols_rows,
                                ""
                            );
                            buf = [0u8];
                        }
                        [b'b'] => {
                            if idx == 0 {
                                idx = cols_rows.len() - 1;
                            } else {
                                idx -= 1;
                            }
                            self.draw_sheet_find(
                                cells, 
                                idx, 
                                &cols_rows,
                                ""
                            );
                            buf = [0u8];
                        }
                        _ => break,
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
        self.draw_focused_content();
        self.flush();
    }

    // `sh o {path} [-i] [-d {delim}]` loads a file into a new sheet,
    // or in place of the focused sheet with -i
    fn open_sheet(&mut self, tokens: Vec<&str>, csvs: &mut Csvs) {
        let tok = tokens[1];
        let mut path: Option<String> = None;
        let mut in_place = false;
        let mut delim = csvs.get_cells().delim;

        let mut args = tokens.into_iter().skip(2);
        while let Some(arg) = args.next() {
            match arg {
                "-i" | "--in-place" => in_place = true,
                "-d" | "--delimiter" => match args.next() {
                    Some("t") => delim = '\t',
                    Some(d) if d.chars().count() == 1 => delim = d.chars().next().unwrap(),
                    Some(d) => {
                        cmd_err::print(CmdErr::InvalidArg(d), self.height);
                        return;
                    }
                    None => {
                        cmd_err::print(CmdErr::MissingValue(arg), self.height);
                        return;
                    }
                },
                _ if path.is_none() => path = Some(Self::expand_home(&Self::trim_quotes(arg))),
                _ => {
                    cmd_err::print(CmdErr::TooManyArgs(tok), self.height);
                    return;
                }
            }
        }

        let path = match path {
            Some(p) => p,
            None => {
                cmd_err::print(CmdErr::MissingName(tok), self.height);
                return;
            }
        };

        // load first so a bad path doesn't prompt to save
        let new_cells = match load_csv(path.clone(), delim) {
            Ok(c) => c,
            Err(e) => {
                print_bottom!(self, "Could not open {}: {}", path, e);
                self.flush();
                return;
            }
        };

        if in_place {
            if csvs.get_cells().written
            && !self.save_before(csvs.get_cells(), &format!("opening {path}")) {
                return;
            }
            let id = csvs.handle;
            csvs.contexts[id] = Context::new(id, new_cells);
            csvs.replaced = true;
        } else {
            csvs.save_context(self);
            let id = csvs.num_contexts();
            csvs.push_context(Context::new(id, new_cells));
            csvs.set_handle(id);
        }

        self.set_context(csvs.get_context());
        self.draw_context(csvs);
        self.changed = WinChange::Screen;
        self.show_csv(csvs.get_cells());
        print_bottom!(self, "Opened {}", path);
        self.flush();
    }

    fn expand_home(path: &str) -> String {
        match (path.strip_prefix("~/"), std::env::home_dir()) {
            (Some(rest), Some(home)) => home.join(rest).to_string_lossy().into_owned(),
            _ => path.to_string(),
        }
    }

    // closes the focused sheet and focuses the next one
    // (process_command quits instead when it's the only sheet)
    fn close_sheet(&mut self, csvs: &mut Csvs) {
        if csvs.get_cells().written && !self.save_before(csvs.get_cells(), "closing") {
            return;
        }
        let closed = csvs.remove_context(csvs.handle);
        if csvs.handle >= csvs.num_contexts() {
            // the closed sheet isn't a sheet to go back to,
            // so keep the one backtick pointed at before
            let prev = csvs.prev;
            csvs.set_handle(csvs.num_contexts() - 1);
            csvs.prev = prev.filter(|&p| p != csvs.handle);
        }

        self.set_context(csvs.get_context());
        self.draw_context(csvs);
        self.changed = WinChange::Screen;
        self.show_csv(csvs.get_cells());
        print_bottom!(self, "Closed {}", closed.cells.filename);
        self.flush();
    }

    // asks whether to save a sheet with edits before it's replaced or closed;
    // false cancels
    fn save_before(&mut self, cells: &mut Cells, action: &str) -> bool {
        print_bottom!(
            self,
            "Save {} before {}? [y/n/c(ancel)]",
            cells.filename,
            action
        );
        self.flush();
        match self.read_key() {
            Some(b'y') | Some(b'Y') => {
                let _ = save_backup(&cells.filename);
                match write_to_file(cells) {
                    Ok(_) => {
                        cells.written = false;
                        true
                    }
                    Err(e) => {
                        print_bottom!(self, "Could not save {}: {}", cells.filename, e);
                        self.flush();
                        false
                    }
                }
            }
            Some(b'n') | Some(b'N') => true,
            _ => {
                print_bottom!(self, "Cancelled {}", action);
                self.flush();
                false
            }
        }
    }

    // block for a single keypress; escape sequences and signals give None
    fn read_key(&mut self) -> Option<u8> {
        let mut buf = [0u8; 16];
        loop {
            match poll_stdin(&mut buf) {
                Ok(PollEvent::Data(0)) => continue,
                Ok(PollEvent::Data(1)) => return Some(buf[0]),
                Ok(PollEvent::Data(_)) => return None,
                Ok(PollEvent::Sig) => match check_flags() {
                    SigFlag::Winch => {
                        unsafe {
                            let mut ws: winsize = mem::zeroed();
                            if ioctl(STDOUT_FILENO, TIOCGWINSZ.into(), &mut ws) == 0 {
                                self.width = ws.ws_col as usize;
                                self.height = ws.ws_row as usize;
                            }
                        }
                    }
                    _ => return None,
                },
                Err(_) => return None,
            }
        }
    }

    fn rename_sheet(&mut self, csvs: &mut Csvs, new_name: &str) {
        let new_name = Self::trim_quotes(new_name);
        let cells = csvs.get_cells();
        let old_name = cells.filename.clone();
        cells.filename = new_name.clone();
        cells.written = true;
        
        self.draw_context(csvs);
        print_bottom!(
            self,
            "Renamed {} to {}",
            old_name,
            new_name
        );
        self.flush();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csv_io::write_to_file;
    use std::{
        env, fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn cells_with_rows(filename: &str, headers: Vec<&str>, rows: Vec<Vec<&str>>) -> Cells {
        let num_cols = headers.len();
        let header = headers
            .into_iter()
            .map(Cell::new)
            .collect::<Vec<Cell>>();
        let col_ids = make_col_ids(num_cols);
        let mut cells = Cells::new(filename.to_string(), ',', header, col_ids, num_cols);

        for _ in 0..num_cols {
            cells.push_column(Column::new());
        }

        for row in rows {
            for col_idx in 0..num_cols {
                let content = row.get(col_idx).copied().unwrap_or("");
                cells.push_to_col(col_idx, Cell::new(content));
            }
        }

        cells.set_w_cell(0, 0);
        cells
    }

    fn cells_with_generated_rows(filename: &str, headers: Vec<&str>, row_count: usize) -> Cells {
        let num_cols = headers.len();
        let header = headers
            .iter()
            .map(|name| Cell::new(name))
            .collect::<Vec<Cell>>();
        let col_ids = make_col_ids(num_cols);
        let mut cells = Cells::new(filename.to_string(), ',', header, col_ids, num_cols);

        for _ in 0..num_cols {
            cells.push_column(Column::new());
        }

        for row_idx in 0..row_count {
            for col_idx in 0..num_cols {
                let content = format!("{}-{row_idx}", headers[col_idx]);
                cells.push_to_col(col_idx, Cell::new(&content));
            }
        }

        cells.set_w_cell(0, 0);
        cells
    }

    fn column_values(cells: &Cells, idx: usize) -> Vec<&str> {
        (0..cells.columns[idx].len())
            .map(|row| cells.columns[idx].view_cell(row))
            .collect()
    }

    fn test_path(name: &str) -> String {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir()
            .join(format!("csview_{name}_{}_{}.csv", std::process::id(), nonce))
            .to_string_lossy()
            .into_owned()
    }

    fn command_win() -> WinInfo {
        let mut win = WinInfo::new();
        win.width = 80;
        win.height = 10;
        win.cursor.limit = 79;
        win
    }

    fn type_command(win: &mut WinInfo, cmd: &str) {
        win.write_buffer.reset();
        win.cursor.offset = 0;
        win.add_to_write_buffer(cmd);
    }

    fn win_for(cells: &mut Cells) -> WinInfo {
        let mut win = command_win();
        win.num_cols = cells.num_cols();
        win.num_rows = cells.num_rows();
        win
    }

    fn open_fixture() -> (Csvs, WinInfo, String) {
        let path = test_path("open_target");
        fs::write(&path, "x,y\n1,2\n3,4\n").unwrap();
        let cells = cells_with_rows("first.csv", vec!["a"], vec![vec!["one"]]);
        let mut csvs = Csvs::new(vec![Context::new(0, cells)]);
        let mut win = command_win();
        win.set_context(csvs.get_context());
        (csvs, win, path)
    }

    #[test]
    fn open_adds_a_sheet_and_undo_removes_it() {
        let (mut csvs, mut win, path) = open_fixture();

        type_command(&mut win, &format!("sh o \"{path}\""));
        win.process_command(&mut csvs);
        assert_eq!(csvs.num_contexts(), 2);
        assert_eq!(csvs.handle, 1);
        assert_eq!(csvs.get_cells().filename, path);
        assert_eq!(csvs.get_cells().num_cols(), 2);
        assert!(!csvs.get_cells().written);

        assert!(crate::history::undo(&mut csvs, &mut win));
        assert_eq!(csvs.num_contexts(), 1);
        assert_eq!(csvs.get_cells().filename, "first.csv");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn open_in_place_replaces_the_sheet_and_undo_restores_it() {
        let (mut csvs, mut win, path) = open_fixture();

        type_command(&mut win, &format!("sheet open {path} -i"));
        win.process_command(&mut csvs);
        assert_eq!(csvs.num_contexts(), 1);
        assert_eq!(csvs.get_cells().filename, path);
        assert_eq!(win.num_cols, 2);

        assert!(crate::history::undo(&mut csvs, &mut win));
        assert_eq!(csvs.num_contexts(), 1);
        assert_eq!(csvs.get_cells().filename, "first.csv");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn close_removes_the_sheet_and_undo_restores_it() {
        let (mut csvs, mut win, path) = open_fixture();
        type_command(&mut win, &format!("sh o {path}"));
        win.process_command(&mut csvs);
        csvs.set_handle(0);
        win.set_context(csvs.get_context());

        type_command(&mut win, "sh c");
        assert!(matches!(win.process_command(&mut csvs), SigFlag::Non));
        assert_eq!(csvs.num_contexts(), 1);
        assert_eq!(csvs.handle, 0);
        assert_eq!(csvs.contexts[0].id, 0);
        assert_eq!(csvs.get_cells().filename, path);

        assert!(crate::history::undo(&mut csvs, &mut win));
        assert_eq!(csvs.num_contexts(), 2);
        assert_eq!(csvs.contexts[0].cells.filename, "first.csv");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn closing_the_last_sheet_focuses_the_one_before() {
        let (mut csvs, mut win, path) = open_fixture();
        type_command(&mut win, &format!("sh o {path}"));
        win.process_command(&mut csvs);

        type_command(&mut win, "sheet close");
        win.process_command(&mut csvs);
        assert_eq!(csvs.num_contexts(), 1);
        assert_eq!(csvs.handle, 0);
        assert_eq!(csvs.get_cells().filename, "first.csv");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn closing_the_only_sheet_quits() {
        let (mut csvs, mut win, _) = open_fixture();
        type_command(&mut win, "sh close");
        assert!(matches!(win.process_command(&mut csvs), SigFlag::Quit));
        assert_eq!(csvs.num_contexts(), 1);
    }

    #[test]
    fn open_missing_file_changes_nothing() {
        let (mut csvs, mut win, _) = open_fixture();

        type_command(&mut win, "sh o /no/such/file.csv -i");
        win.process_command(&mut csvs);
        assert_eq!(csvs.num_contexts(), 1);
        assert_eq!(csvs.get_cells().filename, "first.csv");
        assert!(!crate::history::undo(&mut csvs, &mut win));
    }

    #[test]
    fn tokenize_handles_multibyte_chars() {
        assert_eq!(WinInfo::tokenize("cn to 日本", ' '), vec!["cn", "to", "日本"]);
        assert_eq!(WinInfo::tokenize("col f 'é名 x' y", ' '), vec!["col", "f", "'é名 x'", "y"]);
        assert_eq!(WinInfo::tokenize("é-日", '-'), vec!["é", "日"]);
    }

    #[test]
    fn trim_quotes_handles_empty_and_multibyte() {
        assert_eq!(WinInfo::trim_quotes(""), "");
        assert_eq!(WinInfo::trim_quotes("'"), "");
        assert_eq!(WinInfo::trim_quotes("'é名'"), "é名");
        assert_eq!(WinInfo::trim_quotes("plain"), "plain");
    }

    #[test]
    fn row_numbers_reject_bad_input_without_panicking() {
        assert!(WinInfo::str_to_dec("").is_err());
        assert!(WinInfo::str_to_dec("'é'").is_err());
        assert!(WinInfo::str_to_dec("'FFFFFFFFFFFFFFFFFFFF'").is_err());
        assert_eq!(WinInfo::str_to_dec("'1F'").ok(), Some(31));
    }

    #[test]
    fn col_idx_rejects_empty_and_lone_quote() {
        let cells = cells_with_rows("ids.csv", vec!["é名", "b"], vec![vec!["1", "2"]]);
        assert!(cells.get_col_idx("").is_err());
        assert!(cells.get_col_idx("'").is_err());
        assert_eq!(cells.get_col_idx("'é名'").ok(), Some(0));
    }

    #[test]
    fn write_buffer_grows_with_text_after_cursor() {
        let mut buf = WriteBuf::new(4);
        for c in "abcd".chars() {
            buf.insert(c);
        }
        buf.move_gap(1);
        for c in "XYZ".chars() {
            buf.insert(c);
        }
        assert_eq!(buf.as_string().trim_end(), "aXYZbcd");
        buf.move_gap(100);
        assert_eq!(buf.gap_start, buf.content_len);
    }

    #[test]
    fn tab_jumps_over_delimiters_and_home_end_reach_buffer_edges() {
        let mut win = command_win();
        type_command(&mut win, "foo/bar\\baz, qux");
        win.move_cursor_home();
        assert_eq!(win.write_buffer.gap_start, 0);
        assert_eq!(win.cursor.offset, 0);

        win.jump_word_right();
        assert_eq!(win.write_buffer.gap_start, 4); // bar
        win.jump_word_right();
        assert_eq!(win.write_buffer.gap_start, 8); // baz
        win.jump_word_right();
        assert_eq!(win.write_buffer.gap_start, 13); // qux
        win.jump_word_right();
        assert_eq!(win.write_buffer.gap_start, 16); // end

        win.jump_word_left();
        assert_eq!(win.write_buffer.gap_start, 13);
        win.jump_word_left();
        assert_eq!(win.write_buffer.gap_start, 8);

        win.move_cursor_end();
        assert_eq!(win.write_buffer.gap_start, 16);
        assert_eq!(win.cursor.offset, 16);
        assert_eq!(win.write_buffer.as_string().trim_end(), "foo/bar\\baz, qux");

        type_command(&mut win, "snake_case");
        win.move_cursor_home();
        win.jump_word_right();
        assert_eq!(win.write_buffer.gap_start, 6); // case
    }

    #[test]
    fn home_and_end_scroll_long_buffers() {
        let mut win = command_win();
        win.cursor.limit = 10;
        // write mode sizes both the cursor limit and the scroll window to the cell
        win.write_buffer.window = 10;
        for _ in 0..50 { win.add_to_write_buffer("w"); }
        win.move_cursor_home();
        assert_eq!(win.write_buffer.gap_start, 0);
        assert_eq!(win.write_buffer.offset, 0);
        win.move_cursor_end();
        assert_eq!(win.write_buffer.gap_start, 50);
    }

    #[test]
    fn paste_longer_than_window_keeps_cursor_in_sync() {
        let mut win = command_win();
        win.cursor.limit = 10;
        win.write_buffer.window = 10;
        win.set_yanked(&Cell::new(&"abcdefghij".repeat(3)));
        win.paste_yanked_to_write_buffer();
        let (buf, cur) = (&win.write_buffer, &win.cursor);
        assert_eq!(buf.gap_start, 30);
        assert_eq!(buf.offset + cur.offset, buf.gap_start);

        for _ in 0..30 { win.move_cursor_left(); }
        assert_eq!(win.write_buffer.gap_start, 0);
        assert_eq!(win.write_buffer.offset, 0);
        assert_eq!(win.cursor.offset, 0);
        win.move_cursor_end();
        assert_eq!(win.write_buffer.gap_start, 30);
        assert_eq!(win.write_buffer.offset + win.cursor.offset, 30);
    }

    #[test]
    fn long_command_then_right_arrow_does_not_panic() {
        let mut win = command_win();
        win.cursor.limit = 10;
        type_command(&mut win, &"w".repeat(2000));
        win.move_cursor_right();
        win.move_cursor_right();
        assert_eq!(win.write_buffer.as_string().trim_end(), "w".repeat(2000));
    }

    #[test]
    fn sheet_sort_after_reverting_one_filtered_column() {
        let mut cells = cells_with_rows(
            "mixed.csv",
            vec!["A", "B"],
            vec![vec!["z", "1"], vec!["m", "2"], vec!["a", "3"]],
        );
        WinInfo::apply_sheet_filter(&mut cells, &[0, 2]);
        cells.columns[1].revert();
        let mut win = win_for(&mut cells);

        win.sort_by(&mut cells, "A", Sort::AscAlph);

        for col in &cells.columns {
            assert!(col.indices.iter().all(|&idx| idx < col.cells.len()));
        }
        assert_eq!(cells.columns[0].view_cell(0), "a");
        assert_eq!(cells.columns[0].view_cell(1), "z");
    }

    #[test]
    fn inserted_rows_survive_revert_after_unique() {
        let mut cells = cells_with_rows(
            "unique.csv",
            vec!["A"],
            vec![vec!["x"], vec!["x"], vec!["y"]],
        );
        cells.columns[0].make_unique();
        let mut win = win_for(&mut cells);

        win.insert_row(&mut cells, 2);
        cells.columns[0].revert();

        // reverted order is storage order, with the new rows kept at the end
        assert_eq!(column_values(&cells, 0), vec!["x", "x", "y", "", ""]);
    }

    fn dup_rows() -> Cells {
        cells_with_rows(
            "dup.csv",
            vec!["A", "B"],
            vec![vec!["x", "1"], vec!["x", "2"], vec!["y", "3"], vec!["z", "4"], vec!["y", "5"]],
        )
    }

    fn shown_values(cells: &Cells, idx: usize) -> Vec<&str> {
        (0..cells.visible_rows().min(cells.columns[idx].indices.len()))
            .map(|row| cells.columns[idx].view_cell(row))
            .collect()
    }

    #[test]
    fn row_slice_after_sift_copies_shown_rows() {
        let mut csvs = Csvs::new(vec![Context::new(0, dup_rows())]);
        let mut win = win_for(csvs.get_cells());
        win.sift_by(csvs.get_cells(), "A");

        win.slice_rows("0-1", &mut csvs);

        let slice = &csvs.contexts[1].cells;
        assert_eq!(shown_values(slice, 0), vec!["x", "y"]);
        assert_eq!(shown_values(slice, 1), vec!["1", "3"]);
        // the sifted-out duplicates stay hidden in the original
        let orig = &mut csvs.contexts[0].cells;
        assert_eq!(shown_values(orig, 0), vec!["z"]);
        assert_eq!(shown_values(orig, 1), vec!["4"]);
        for col in &mut orig.columns {
            col.revert();
        }
        assert_eq!(column_values(orig, 0), vec!["x", "z", "y"]);
        assert_eq!(column_values(orig, 1), vec!["2", "4", "5"]);
    }

    #[test]
    fn row_slice_after_col_unique_copies_shown_rows() {
        let mut csvs = Csvs::new(vec![Context::new(0, dup_rows())]);
        csvs.get_cells().columns[0].make_unique();
        let mut win = win_for(csvs.get_cells());

        win.slice_rows("2-4", &mut csvs);

        let slice = &csvs.contexts[1].cells;
        assert_eq!(shown_values(slice, 0), vec!["z", "", ""]);
        assert_eq!(shown_values(slice, 1), vec!["3", "4", "5"]);
        let orig = &csvs.contexts[0].cells;
        assert_eq!(shown_values(orig, 0), vec!["x", "y"]);
        assert_eq!(shown_values(orig, 1), vec!["1", "2"]);
    }

    #[test]
    fn col_slice_after_sift_copies_shown_rows() {
        let mut csvs = Csvs::new(vec![Context::new(0, dup_rows())]);
        let mut win = win_for(csvs.get_cells());
        win.sift_by(csvs.get_cells(), "A");

        win.slice_cols("B", &mut csvs);

        let slice = &csvs.contexts[1].cells;
        assert_eq!(column_values(slice, 0), vec!["1", "3", "4"]);
        assert_eq!(shown_values(&csvs.contexts[0].cells, 0), vec!["x", "y", "z"]);
    }

    #[test]
    fn slicing_every_row_or_column_is_refused() {
        let cells = cells_with_rows("all.csv", vec!["A", "B"], vec![vec!["1", "2"], vec!["3", "4"]]);
        let mut csvs = Csvs::new(vec![Context::new(0, cells)]);
        let mut win = win_for(csvs.get_cells());

        win.slice_rows("0-1", &mut csvs);
        win.slice_cols("A-B", &mut csvs);
        win.slice_rows("5", &mut csvs);
        win.slice_cols("B-A", &mut csvs);
        assert_eq!(csvs.num_contexts(), 1);

        win.slice_cols("B", &mut csvs);
        assert_eq!(csvs.num_contexts(), 2);
        assert_eq!(csvs.contexts[0].cells.num_cols(), 1);
    }

    #[test]
    fn view_is_clamped_after_sheet_shrinks() {
        let mut cells = cells_with_generated_rows("shrink.csv", vec!["A", "B"], 40);
        let mut win = win_for(&mut cells);
        win.h_pointer = 39;
        win.h_offset = 35;
        win.w_pointer = 1;
        win.num_rows = 2;
        win.num_cols = 1;

        win.draw_screen(&mut cells);
        win.draw_rows(&mut cells);
        win.draw_from_column(&mut cells);

        assert_eq!((win.w_pointer, win.h_pointer), (0, 1));
        assert!(win.h_offset <= win.h_pointer);
    }

    #[test]
    fn new_column_on_filtered_sheet_matches_layout() {
        let mut cells = cells_with_rows(
            "newcol.csv",
            vec!["A"],
            vec![vec!["q"], vec!["x"], vec!["q"]],
        );
        WinInfo::apply_sheet_filter(&mut cells, &[0, 2]);
        let mut win = win_for(&mut cells);
        win.num_rows = 2;

        win.new_column(&mut cells, "B");

        let (a, b) = (&cells.columns[0], &cells.columns[1]);
        assert_eq!(a.indices, b.indices);
        assert_eq!(a.cells.len(), b.cells.len());
        assert_eq!(a.padding, b.padding);
        // undo/redo and sheet switching reload the visible count
        assert_eq!(cells.visible_rows(), 2);
    }

    #[test]
    fn filtering_an_already_filtered_sheet_keeps_counts() {
        let mut cells = cells_with_rows(
            "refilter.csv",
            vec!["A", "B"],
            vec![
                vec!["zz", "1"], vec!["a", "2"], vec!["zz", "3"],
                vec!["b", "4"], vec!["zz", "5"], vec!["c", "6"],
            ],
        );
        let rows = WinInfo::matching_sheet_cells(&cells, "zz", false)
            .iter().map(|m| m.1).collect::<Vec<_>>();
        assert_eq!(rows, vec![0, 2, 4]);
        WinInfo::apply_sheet_filter(&mut cells, &rows);
        assert_eq!(cells.visible_rows(), 3);

        // only the shown rows are searched the second time
        let rows = WinInfo::matching_sheet_cells(&cells, "3", false)
            .iter().map(|m| m.1).collect::<Vec<_>>();
        assert_eq!(rows, vec![1]);
        WinInfo::apply_sheet_filter(&mut cells, &rows);
        assert_eq!(cells.visible_rows(), 1);
        assert_eq!(cells.columns[1].view_cell(0), "3");

        for col in &cells.columns {
            assert_eq!(col.indices.len(), col.cells.len());
            assert!(col.indices.iter().all(|&idx| idx < col.cells.len()));
        }
        // every original value is still stored
        cells.columns[1].revert();
        assert_eq!(column_values(&cells, 1), vec!["1", "2", "3", "4", "5", "6"]);
    }

    #[test]
    fn grouping_focused_first_column_behind_another_redraws_it() {
        let mut cells = cells_with_rows(
            "groupfirst.csv",
            vec!["A", "B", "C"],
            vec![vec!["aaaaaaaa", "b", "c"]],
        );
        let mut win = win_for(&mut cells);
        win.draw_screen(&mut cells);
        let b_start = cells.columns[1].start;
        assert!(b_start > cells.columns[0].start);

        // focus stays on index 0, but A moves after B
        win.group_columns(&mut cells, "B,A");

        assert_eq!(cells.header[0].content, "B");
        assert_eq!(cells.header[1].content, "A");
        // B is redrawn in the first slot, not left at its old position
        assert_eq!(cells.columns[0].start, 6);
        assert!(cells.columns[1].start > cells.columns[0].start);
    }

    #[test]
    fn unique_on_first_column_keeps_sheet_row_count() {
        let mut cells = cells_with_rows(
            "firstcol.csv",
            vec!["A", "B"],
            vec![vec!["x", "1"], vec!["x", "2"], vec!["x", "3"]],
        );
        cells.columns[0].make_unique();

        assert_eq!(cells.columns[0].visible_len(), 1);
        assert_eq!(cells.visible_rows(), 3);
    }

    #[test]
    fn unique_on_only_column_shrinks_sheet() {
        let cells = cells_with_rows(
            "onecol.csv",
            vec!["A"],
            vec![vec!["N/A"], vec!["0x1F"], vec![""]],
        );
        let mut csvs = Csvs::new(vec![Context::new(0, cells)]);
        let mut win = win_for(csvs.get_cells());
        win.h_pointer = 2;

        win.show_unique_column_values(&mut csvs);

        assert_eq!(win.num_rows, 2);
        assert_eq!(win.h_pointer, 1);
    }

    #[test]
    fn command_history_browses_older_and_newer() {
        let mut win = command_win();
        win.push_cmd_history("row c");
        win.push_cmd_history("col c");

        type_command(&mut win, "sh");
        win.browse_cmd_history(true);
        assert_eq!(win.write_buffer.as_string(), "col c");
        win.browse_cmd_history(true);
        assert_eq!(win.write_buffer.as_string(), "row c");
        // stops at the oldest
        win.browse_cmd_history(true);
        assert_eq!(win.write_buffer.as_string(), "row c");

        win.browse_cmd_history(false);
        assert_eq!(win.write_buffer.as_string(), "col c");
        // past the newest restores what was being typed
        win.browse_cmd_history(false);
        assert_eq!(win.write_buffer.as_string(), "sh");
        win.browse_cmd_history(false);
        assert_eq!(win.write_buffer.as_string(), "sh");
    }

    #[test]
    fn command_history_skips_blank_and_repeated_commands() {
        let mut win = command_win();
        win.push_cmd_history("row c");
        win.push_cmd_history("row c");
        win.push_cmd_history("  ");

        assert_eq!(win.cmd_history, vec!["row c"]);
    }

    #[test]
    fn command_history_cursor_ends_after_recalled_command() {
        let mut win = command_win();
        win.push_cmd_history("row c");

        type_command(&mut win, "");
        win.browse_cmd_history(true);
        win.add_to_write_buffer("ount");
        assert_eq!(win.write_buffer.as_string(), "row count");
    }

    #[test]
    fn command_history_persists_across_sessions() {
        let path = PathBuf::from(test_path("cmd_history"));

        let mut win = command_win();
        win.set_cmd_history_file(path.clone());
        win.push_cmd_history("row c");
        win.push_cmd_history("col c");

        let mut next = command_win();
        next.set_cmd_history_file(path.clone());
        assert_eq!(next.cmd_history, vec!["row c", "col c"]);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn command_history_file_is_trimmed_to_max() {
        let path = PathBuf::from(test_path("cmd_history_trim"));
        let lines: Vec<String> = (0..MAX_CMD_HISTORY + 5)
            .map(|i| format!("row g {i}"))
            .collect();
        fs::write(&path, lines.join("\n")).unwrap();

        let mut win = command_win();
        win.set_cmd_history_file(path.clone());
        assert_eq!(win.cmd_history.len(), MAX_CMD_HISTORY);
        assert_eq!(win.cmd_history[0], "row g 5");
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), MAX_CMD_HISTORY);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn focused_preview_renders_tabs_as_spaces() {
        let mut win = WinInfo::new();
        win.width = 20;

        win.set_focused("a\tb");

        assert_eq!(win.focused_content, "a b");
    }

    #[test]
    fn table_rendering_does_not_write_raw_tabs_to_frame() {
        let mut cells = cells_with_rows("tabbed.csv", vec!["report"], vec![vec!["a\tb"]]);
        let mut win = WinInfo::new();
        win.width = 80;
        win.height = 10;
        win.num_cols = cells.num_cols();
        win.num_rows = cells.num_rows();

        win.draw_screen(&mut cells);

        assert!(!win.frame.contains('\t'));
        assert!(win.frame.contains("a b"));
    }

    #[test]
    fn filtered_sheet_with_late_match_can_be_saved() {
        let path = test_path("filtered_sheet_save");
        let mut cells = cells_with_rows(
            &path,
            vec!["A", "B"],
            vec![
                vec!["a0", "b0"],
                vec!["a1", "b1"],
                vec!["match", "b2"],
                vec!["a3", "b3"],
            ],
        );

        WinInfo::apply_sheet_filter(&mut cells, &[2]);
        write_to_file(&mut cells).unwrap();
        let saved = fs::read_to_string(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(saved, "A,B\nmatch,b2\n");
        for col in &cells.columns {
            assert!(col.indices.iter().all(|&idx| idx < col.cells.len()));
        }
    }

    #[test]
    fn filtered_sheet_partitions_noncontiguous_matches() {
        let mut cells = cells_with_rows(
            "filtered.csv",
            vec!["A", "B"],
            vec![
                vec!["a0", "b0"],
                vec!["a1", "b1"],
                vec!["a2", "b2"],
                vec!["a3", "b3"],
                vec!["a4", "b4"],
            ],
        );

        WinInfo::apply_sheet_filter(&mut cells, &[1, 3]);

        assert_eq!(cells.columns[0].view_cell(0), "a1");
        assert_eq!(cells.columns[0].view_cell(1), "a3");
        assert_eq!(cells.columns[1].view_cell(0), "b1");
        assert_eq!(cells.columns[1].view_cell(1), "b3");
        for col in &cells.columns {
            assert_eq!(col.len(), 5);
            assert!(col.indices.iter().all(|&idx| idx < col.cells.len()));
        }

        for col in &mut cells.columns {
            col.revert();
        }
        assert_eq!(column_values(&cells, 0), vec!["a0", "a1", "a2", "a3", "a4"]);
        assert_eq!(column_values(&cells, 1), vec!["b0", "b1", "b2", "b3", "b4"]);
    }

    #[test]
    fn column_find_uses_display_rows_after_sort() {
        let mut cells = cells_with_rows(
            "sorted_column.csv",
            vec!["A"],
            vec![vec!["match-z"], vec!["other"], vec!["match-a"]],
        );

        WinInfo::sort_indices(&mut cells.columns[0], Sort::AscAlph);

        assert_eq!(column_values(&cells, 0), vec!["match-a", "match-z", "other"]);
        assert_eq!(
            WinInfo::matching_column_rows(&cells.columns[0], "match", false),
            vec![0, 1]
        );
    }

    #[test]
    fn filtered_column_preserves_sorted_match_order() {
        let mut cells = cells_with_rows(
            "filtered_sorted_column.csv",
            vec!["A"],
            vec![
                vec!["match-z"],
                vec!["other-b"],
                vec!["match-a"],
                vec!["other-a"],
            ],
        );
        let col = &mut cells.columns[0];
        WinInfo::sort_indices(col, Sort::AscAlph);
        let matches = WinInfo::matching_column_rows(col, "match", false);

        WinInfo::apply_column_filter(col, &matches);

        assert_eq!(matches, vec![0, 1]);
        assert_eq!(col.view_cell(0), "match-a");
        assert_eq!(col.view_cell(1), "match-z");
        assert!(col.indices.iter().all(|&idx| idx < col.cells.len()));

        col.revert();
        assert_eq!(column_values(&cells, 0), vec!["match-z", "other-b", "match-a", "other-a"]);
    }

    #[test]
    fn sheet_find_uses_display_rows_after_sort() {
        let mut cells = cells_with_rows(
            "sorted_sheet.csv",
            vec!["A", "B"],
            vec![
                vec!["z", "match-z"],
                vec!["m", "other"],
                vec!["a", "match-a"],
            ],
        );

        WinInfo::sort_indices(&mut cells.columns[0], Sort::AscAlph);
        let sorted_indices = cells.columns[0].indices.clone();
        cells.columns[1].indices = sorted_indices;

        assert_eq!(column_values(&cells, 1), vec!["match-a", "other", "match-z"]);
        assert_eq!(
            WinInfo::matching_sheet_cells(&cells, "match", false),
            vec![(1, 0), (1, 2)]
        );
    }

    #[test]
    fn filtered_sheet_preserves_sorted_rows_and_column_alignment() {
        let mut cells = cells_with_rows(
            "filtered_sorted_sheet.csv",
            vec!["A", "B"],
            vec![
                vec!["z", "match-z"],
                vec!["m", "other"],
                vec!["a", "match-a"],
            ],
        );
        WinInfo::sort_indices(&mut cells.columns[0], Sort::AscAlph);
        let sorted_indices = cells.columns[0].indices.clone();
        cells.columns[1].indices = sorted_indices;
        let mut matches = WinInfo::matching_sheet_cells(&cells, "match", false);
        let visible_rows = matches.iter().map(|(_, row)| *row).collect::<Vec<_>>();

        WinInfo::apply_sheet_filter(&mut cells, &visible_rows);
        WinInfo::remap_sheet_matches(&mut matches, &visible_rows);

        assert_eq!(matches, vec![(1, 0), (1, 1)]);
        assert_eq!(cells.columns[0].view_cell(0), "a");
        assert_eq!(cells.columns[1].view_cell(0), "match-a");
        assert_eq!(cells.columns[0].view_cell(1), "z");
        assert_eq!(cells.columns[1].view_cell(1), "match-z");
        for col in &cells.columns {
            assert!(col.indices.iter().all(|&idx| idx < col.cells.len()));
        }
    }

    #[test]
    fn paste_uses_latest_yanked_content() {
        let mut cells = cells_with_rows("paste.csv", vec!["report"], vec![vec![""]]);
        let mut win = WinInfo::new();
        win.width = 80;
        win.height = 10;
        win.num_cols = cells.num_cols();
        win.num_rows = cells.num_rows();

        win.set_yanked(&Cell::new("first"));
        win.paste(&mut cells);
        assert_eq!(cells.columns[0].view_cell(0), "first");

        win.set_yanked(&Cell::new("second\tvalue"));
        win.paste(&mut cells);
        assert_eq!(cells.columns[0].view_cell(0), "second\tvalue");
    }

    #[test]
    fn paste_content_does_not_replace_yanked_content() {
        let mut cells = cells_with_rows("paste.csv", vec!["report"], vec![vec![""]]);
        let mut win = WinInfo::new();
        win.width = 80;
        win.height = 10;
        win.num_cols = cells.num_cols();
        win.num_rows = cells.num_rows();
        win.set_yanked(&Cell::new("yanked"));

        win.paste_content(&mut cells, "clipboard");

        assert_eq!(cells.columns[0].view_cell(0), "clipboard");
        assert_eq!(win.yanked, "yanked");
    }

    #[test]
    fn splice_rows_pads_columns_when_headers_do_not_match() {
        let mut dest = cells_with_rows(
            "dest.csv",
            vec!["A", "B"],
            vec![vec!["a0", "b0"], vec!["a1", "b1"]],
        );
        let mut src = cells_with_rows(
            "src.csv",
            vec!["A", "C"],
            vec![vec!["a2", "c2"], vec!["a3", "c3"]],
        );

        WinInfo::splice_rows_into(&mut dest, &mut src, 0);

        assert_eq!(dest.num_rows(), 4);
        assert_eq!(dest.num_cols(), 3);
        assert!(dest.columns.iter().all(|col| col.len() == 4));
        assert_eq!(column_values(&dest, 0), vec!["a0", "a2", "a3", "a1"]);
        assert_eq!(column_values(&dest, 1), vec!["b0", "", "", "b1"]);
        assert_eq!(column_values(&dest, 2), vec!["", "c2", "c3", ""]);
    }

    #[test]
    fn splice_rows_handles_wider_source_when_match_is_not_first_column() {
        let mut dest = cells_with_rows(
            "dest.csv",
            vec!["B"],
            vec![vec!["b0"], vec!["b1"]],
        );
        let mut src = cells_with_rows(
            "src.csv",
            vec!["A", "B", "C"],
            vec![vec!["a2", "b2", "c2"], vec!["a3", "b3", "c3"]],
        );

        WinInfo::splice_rows_into(&mut dest, &mut src, 0);

        assert_eq!(dest.num_rows(), 4);
        assert_eq!(dest.num_cols(), 3);
        assert!(dest.columns.iter().all(|col| col.len() == 4));
        assert_eq!(column_values(&dest, 0), vec!["b0", "b2", "b3", "b1"]);
        assert_eq!(column_values(&dest, 1), vec!["", "a2", "a3", ""]);
        assert_eq!(column_values(&dest, 2), vec!["", "c2", "c3", ""]);
    }

    #[test]
    fn splice_rows_can_draw_after_wider_source_with_many_rows() {
        let mut dest = cells_with_generated_rows(
            "dest.csv",
            vec!["PID", "MRN", "StudyDateTime"],
            65,
        );
        let mut src = cells_with_generated_rows(
            "src.csv",
            vec!["pid", "timepoint", "accession_date", "report", "summary"],
            381,
        );

        WinInfo::splice_rows_into(&mut dest, &mut src, 0);

        let rows = dest.num_rows();
        assert_eq!(rows, 446);
        assert_eq!(dest.num_cols(), 8);
        for (idx, col) in dest.columns.iter().enumerate() {
            assert_eq!(col.len(), rows, "column {idx} has mismatched length");
            assert_eq!(col.indices.len(), rows, "column {idx} has mismatched indices");
        }

        let mut win = WinInfo::new();
        win.width = 200;
        win.height = 24;
        win.num_cols = dest.num_cols();
        win.num_rows = rows;
        win.set_h_pointer(378);
        win.draw_screen(&mut dest);
    }

    fn sheets_fixture(names: &[&str]) -> (Csvs, WinInfo) {
        let contexts = names
            .iter()
            .enumerate()
            .map(|(i, n)| Context::new(i, cells_with_rows(n, vec!["a"], vec![vec!["x"]])))
            .collect::<Vec<Context>>();
        let mut csvs = Csvs::new(contexts);
        let mut win = command_win();
        win.set_context(csvs.get_context());
        (csvs, win)
    }

    fn goto(csvs: &mut Csvs, win: &mut WinInfo, target: &str) {
        type_command(win, &format!("sh g {target}"));
        win.process_command(csvs);
    }

    #[test]
    fn goto_by_index_prefix_and_basename() {
        let (mut csvs, mut win) = sheets_fixture(&[
            "/d/alpha.csv", "/d/beta.csv", "/e/beta2.csv", "gamma.csv",
        ]);

        goto(&mut csvs, &mut win, "1");
        assert_eq!(csvs.handle, 1);
        goto(&mut csvs, &mut win, "gam");
        assert_eq!(csvs.handle, 3);
        goto(&mut csvs, &mut win, "ALPHA");
        assert_eq!(csvs.handle, 0);
        goto(&mut csvs, &mut win, "beta.csv");
        assert_eq!(csvs.handle, 1);
        goto(&mut csvs, &mut win, "beta2");
        assert_eq!(csvs.handle, 2);
        goto(&mut csvs, &mut win, "'/d/alpha.csv'");
        assert_eq!(csvs.handle, 0);
        goto(&mut csvs, &mut win, "/e");
        assert_eq!(csvs.handle, 2);
        goto(&mut csvs, &mut win, "goto");
        assert_eq!(csvs.handle, 2);
        type_command(&mut win, "sheet goto 3");
        win.process_command(&mut csvs);
        assert_eq!(csvs.handle, 3);
    }

    #[test]
    fn goto_bad_targets_do_not_switch() {
        let (mut csvs, mut win) = sheets_fixture(&[
            "/d/alpha.csv", "/d/beta.csv", "/e/beta2.csv",
        ]);

        goto(&mut csvs, &mut win, "be");
        assert_eq!(csvs.handle, 0);
        goto(&mut csvs, &mut win, "nope");
        assert_eq!(csvs.handle, 0);
        goto(&mut csvs, &mut win, "3");
        assert_eq!(csvs.handle, 0);
        goto(&mut csvs, &mut win, "99999999999999999999999");
        assert_eq!(csvs.handle, 0);
        type_command(&mut win, "sh g");
        win.process_command(&mut csvs);
        assert_eq!(csvs.handle, 0);
        assert!(WinInfo::resolve_sheet(&csvs, "be").is_err());
        assert_eq!(WinInfo::resolve_sheet(&csvs, "beta").ok(), Some(1));
    }

    #[test]
    fn goto_leaves_undo_history_alone() {
        let (mut csvs, mut win) = sheets_fixture(&["a.csv", "b.csv"]);
        let before = crate::history::undo_len(&csvs);
        goto(&mut csvs, &mut win, "b");
        goto(&mut csvs, &mut win, "0");
        assert_eq!(csvs.handle, 0);
        assert_eq!(crate::history::undo_len(&csvs), before);
    }

    #[test]
    fn prev_follows_the_handle_and_close() {
        let (mut csvs, _) = sheets_fixture(&["a", "b", "c", "d"]);
        assert_eq!(csvs.prev, None);
        csvs.set_handle(2);
        assert_eq!(csvs.prev, Some(0));
        csvs.set_handle(2);
        assert_eq!(csvs.prev, Some(0));
        csvs.set_handle(3);
        assert_eq!(csvs.prev, Some(2));

        // closing a sheet before prev shifts it down
        csvs.remove_context(0);
        assert_eq!((csvs.handle, csvs.prev), (2, Some(1)));
        // closing prev itself clears it
        csvs.remove_context(1);
        assert_eq!((csvs.handle, csvs.prev), (1, None));
    }

    #[test]
    fn tab_shift_tab_and_backtick_switch_sheets() {
        use crate::input::process_input;
        let (mut csvs, mut win) = sheets_fixture(&["a", "b", "c"]);

        process_input(&[9], &mut win, &mut csvs);
        assert_eq!(csvs.handle, 1);
        process_input(&[9], &mut win, &mut csvs);
        process_input(&[9], &mut win, &mut csvs);
        assert_eq!(csvs.handle, 0);
        process_input(&[27, 91, 90], &mut win, &mut csvs);
        assert_eq!(csvs.handle, 2);
        process_input(&[96], &mut win, &mut csvs);
        assert_eq!(csvs.handle, 0);
        process_input(&[96], &mut win, &mut csvs);
        assert_eq!(csvs.handle, 2);
        process_input(&[49], &mut win, &mut csvs);
        assert_eq!(csvs.handle, 1);

        // a stale prev is ignored
        csvs.prev = Some(7);
        process_input(&[96], &mut win, &mut csvs);
        assert_eq!(csvs.handle, 1);
        csvs.prev = Some(1);
        process_input(&[96], &mut win, &mut csvs);
        assert_eq!(csvs.handle, 1);
    }

    #[test]
    fn single_sheet_ignores_tab_and_backtick() {
        use crate::input::process_input;
        let (mut csvs, mut win) = sheets_fixture(&["a"]);
        process_input(&[9], &mut win, &mut csvs);
        process_input(&[27, 91, 90], &mut win, &mut csvs);
        process_input(&[96], &mut win, &mut csvs);
        assert_eq!(csvs.handle, 0);
    }

    #[test]
    fn split_commands_create_reorient_and_disable_two_panes() {
        let (mut csvs, mut win) = sheets_fixture(&["a.csv", "b.csv", "c.csv"]);

        type_command(&mut win, "sh split v b.csv");
        win.process_command(&mut csvs);
        assert_eq!(csvs.split, Some(SplitView {
            direction: SplitDirection::Vertical,
            first: 0,
            second: 1,
            active: SplitPane::First,
        }));

        type_command(&mut win, "sh split h c.csv");
        win.process_command(&mut csvs);
        assert_eq!(csvs.split.unwrap().direction, SplitDirection::Horizontal);
        assert_eq!(csvs.split.unwrap().second, 2);

        csvs.set_handle(2);
        win.set_context(csvs.get_context());
        type_command(&mut win, "sh split off");
        win.process_command(&mut csvs);
        assert_eq!(csvs.num_contexts(), 3);
        assert_eq!(csvs.handle, 2);
        assert_eq!(csvs.split, None);
        assert_eq!(csvs.contexts[1].cells.filename, "b.csv");
        assert_eq!(csvs.contexts[2].cells.filename, "c.csv");
        assert!(!crate::history::undo(&mut csvs, &mut win));
    }

    #[test]
    fn split_without_target_uses_next_or_creates_blank_sheet() {
        let (mut csvs, mut win) = sheets_fixture(&["a.csv", "b.csv"]);
        type_command(&mut win, "sh split v a.csv");
        win.process_command(&mut csvs);
        assert_eq!(csvs.split, None);

        type_command(&mut win, "sh split v");
        win.process_command(&mut csvs);
        assert_eq!(csvs.split.unwrap().second, 1);

        let (mut csvs, mut win) = sheets_fixture(&["only.csv"]);
        type_command(&mut win, "sh split h");
        win.process_command(&mut csvs);
        assert_eq!(csvs.num_contexts(), 2);
        assert_eq!(csvs.contexts[1].cells.filename, "Untitled.csv");
        assert_eq!(csvs.split.unwrap().direction, SplitDirection::Horizontal);

        csvs.contexts[1].cells.written = true;
        type_command(&mut win, "sh split off");
        win.process_command(&mut csvs);
        assert_eq!(csvs.num_contexts(), 2);
        assert_eq!(csvs.split, None);
        assert!(csvs.contexts[1].cells.written);

        crate::input::process_input(&[9], &mut win, &mut csvs);
        assert_eq!(csvs.handle, 1);

        assert!(crate::history::undo(&mut csvs, &mut win));
        assert_eq!(csvs.num_contexts(), 1);
        assert_eq!(csvs.split, None);
    }

    #[test]
    fn ctrl_o_and_sheet_switching_target_the_active_pane() {
        use crate::input::process_input;
        let (mut csvs, mut win) = sheets_fixture(&["a", "b", "c"]);
        csvs.start_split(SplitDirection::Vertical, 1);

        process_input(&[15], &mut win, &mut csvs);
        assert_eq!(csvs.handle, 1);
        assert_eq!(csvs.split.unwrap().active, SplitPane::Second);

        process_input(&[50], &mut win, &mut csvs);
        assert_eq!(csvs.handle, 2);
        assert_eq!(csvs.split.unwrap().second, 2);

        process_input(&[48], &mut win, &mut csvs);
        assert_eq!(csvs.handle, 0);
        assert_eq!(csvs.split.unwrap().active, SplitPane::First);
        assert_eq!(csvs.split.unwrap().second, 2);
    }

    #[test]
    fn split_panes_restore_independent_scroll_positions() {
        let contexts = ["a", "b"]
            .iter()
            .enumerate()
            .map(|(id, name)| {
                Context::new(id, cells_with_generated_rows(name, vec!["a"], 20))
            })
            .collect();
        let mut csvs = Csvs::new(contexts);
        let mut win = command_win();
        win.height = 20;
        win.set_context(csvs.get_context());
        csvs.start_split(SplitDirection::Vertical, 1);

        win.set_h_pointer(5);
        win.switch_pane(&mut csvs);
        assert_eq!(csvs.contexts[0].h_pointer, 5);

        win.set_h_pointer(7);
        win.switch_pane(&mut csvs);
        assert_eq!(csvs.contexts[1].h_pointer, 7);
        assert_eq!(win.h_pointer, 5);
    }

    #[test]
    fn removing_other_sheet_repairs_split_indices() {
        let (mut csvs, _) = sheets_fixture(&["a", "unused", "b"]);
        csvs.start_split(SplitDirection::Vertical, 2);

        csvs.remove_context(1);
        assert_eq!(csvs.split.unwrap().first, 0);
        assert_eq!(csvs.split.unwrap().second, 1);

        csvs.remove_context(1);
        assert_eq!(csvs.split, None);
        assert_eq!(csvs.handle, 0);
    }

    #[test]
    fn split_layouts_are_equal_and_reject_tiny_terminals() {
        let mut win = command_win();
        win.width = 81;
        win.height = 20;

        let (left, right) = win.split_rects(SplitDirection::Vertical).unwrap();
        assert_eq!(left.width + right.width + 1, 81);
        assert_eq!(right.left, left.width + 2);

        let (top, bottom) = win.split_rects(SplitDirection::Horizontal).unwrap();
        assert_eq!(top.height + bottom.height + 1, 18);
        assert_eq!(bottom.top, top.height + 2);

        win.width = MIN_PANE_WIDTH * 2;
        assert!(win.split_rects(SplitDirection::Vertical).is_none());
        win.width = 80;
        win.height = MIN_PANE_HEIGHT * 2 + 2;
        assert!(win.split_rects(SplitDirection::Horizontal).is_none());
    }

    #[test]
    fn split_renderer_draws_both_sheet_titles() {
        let (mut csvs, mut win) = sheets_fixture(&["left.csv", "right.csv"]);
        win.width = 80;
        win.height = 20;
        csvs.start_split(SplitDirection::Vertical, 1);

        assert!(win.draw_split(&mut csvs));
        assert!(win.frame.contains("0: left.csv"));
        assert!(win.frame.contains("1: right.csv"));
        assert!(win.frame.contains("X"));
        assert!(!win.frame.contains("\x1b[2J"));
        assert_eq!(csvs.handle, 0);
    }

    #[test]
    fn split_mode_suppresses_legacy_full_screen_draws() {
        let (mut csvs, mut win) = sheets_fixture(&["left.csv", "right.csv"]);
        csvs.start_split(SplitDirection::Vertical, 1);
        win.split_mode = true;

        win.draw_screen(csvs.get_cells());
        win.draw_focus(csvs.get_cells());
        win.show_csv(csvs.get_cells());
        assert!(win.frame.is_empty());
    }

    #[test]
    fn collapsing_split_forces_full_screen_redraw() {
        let (mut csvs, mut win) = sheets_fixture(&["left.csv", "right.csv"]);
        csvs.start_split(SplitDirection::Vertical, 1);
        win.changed = WinChange::Screen;
        win.show_workspace(&mut csvs);
        assert!(win.split_mode);

        csvs.split = None;
        win.changed = WinChange::Non;
        win.show_workspace(&mut csvs);
        assert!(!win.split_mode);
        assert_eq!(win.changed, WinChange::Non);
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn bar_width(entries: &[String]) -> usize {
        entries.iter().map(|e| e.chars().count()).sum()
    }

    #[test]
    fn sheet_bar_shows_everything_that_fits() {
        let n = names(&["a.csv", "b.csv"]);
        let (start, entries, left, right) = WinInfo::layout_sheet_bar(&n, 0, 0, 40);
        assert_eq!(start, 0);
        assert_eq!(entries, vec!["0: a.csv ", "1: b.csv "]);
        assert!(!left && !right);
    }

    #[test]
    fn sheet_bar_scrolls_to_the_focused_sheet_and_back() {
        let n = names(&["aaaa", "bbbb", "cccc", "dddd", "eeee"]);
        // each entry is 8 wide; 20 columns leave room for 2
        let (start, entries, left, right) = WinInfo::layout_sheet_bar(&n, 4, 0, 20);
        assert_eq!(start, 3);
        assert_eq!(entries, vec!["3: dddd ", "4: eeee "]);
        assert!(left && !right);

        // moving focus back left pulls the bar back
        let (start, entries, left, right) = WinInfo::layout_sheet_bar(&n, 1, start, 20);
        assert_eq!(start, 1);
        assert_eq!(entries[0], "1: bbbb ");
        assert!(left && right);

        // focus still inside the window leaves the start alone
        let (start, _, _, _) = WinInfo::layout_sheet_bar(&n, 3, 2, 20);
        assert_eq!(start, 2);

        // start is clamped when sheets were closed, and refills the bar
        let (start, entries, _, _) = WinInfo::layout_sheet_bar(&n[..2], 1, 4, 20);
        assert_eq!(start, 0);
        assert_eq!(entries, vec!["0: aaaa ", "1: bbbb "]);
    }

    #[test]
    fn workspace_redraw_tracks_focused_sheet_in_bar() {
        let (mut csvs, mut win) = sheets_fixture(&[
            "aaaa", "bbbb", "cccc", "dddd", "eeee",
        ]);
        win.width = 20;
        csvs.set_handle(4);
        win.set_context(csvs.get_context());
        win.changed = WinChange::Screen;

        win.show_workspace(&mut csvs);

        assert_eq!(win.bar_start, 3);
        assert_eq!(csvs.handle, 4);
    }

    #[test]
    fn sheet_bar_truncates_the_last_entry_and_huge_names() {
        let n = names(&["aaaa", "bbbbbbbbbb"]);
        // 14 usable: "0: aaaa " leaves 6 for "1: bb…"
        let (_, entries, _, right) = WinInfo::layout_sheet_bar(&n, 0, 0, 16);
        assert_eq!(entries, vec!["0: aaaa ", "1: bb…"]);
        assert!(!right);

        // too little room for even one char of the name
        let (_, entries, _, right) = WinInfo::layout_sheet_bar(&n, 0, 0, 12);
        assert_eq!(entries, vec!["0: aaaa "]);
        assert!(right);

        // a single name wider than the bar
        let huge = names(&["x".repeat(100).as_str()]);
        let (start, entries, left, right) = WinInfo::layout_sheet_bar(&huge, 0, 0, 20);
        assert_eq!(start, 0);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].chars().count(), 18);
        assert!(entries[0].starts_with("0: x") && entries[0].ends_with('…'));
        assert!(!left && !right);
    }

    #[test]
    fn sheet_bar_never_exceeds_the_width() {
        let n = names(&["日本語のファイル.csv", "é.csv", "plain.csv", "名前", "z"]);
        for width in 0..60 {
            for handle in 0..n.len() {
                let (start, entries, _, _) = WinInfo::layout_sheet_bar(&n, handle, 0, width);
                assert!(bar_width(&entries) + 2 <= width.max(2), "width {width}");
                assert!(start <= handle);
                if width >= 2 + format!("{handle}: ").len() + 2 {
                    assert!(start + entries.len() > handle, "focus hidden at {width}");
                }
            }
        }
        let (_, entries, _, _) = WinInfo::layout_sheet_bar(&n, 0, 0, 14);
        assert_eq!(entries, vec!["0: 日本語のファイル…"]);
    }

    #[test]
    fn sheet_bar_pulls_earlier_sheets_back_in_when_room_frees_up() {
        let n = names(&["aaaa", "bbbb", "cccc", "dddd", "eeee"]);
        // bar was scrolled to sheet 3 at 20 wide; at 50 wide everything fits
        let (start, entries, left, right) = WinInfo::layout_sheet_bar(&n, 4, 3, 50);
        assert_eq!(start, 0);
        assert_eq!(entries.len(), 5);
        assert!(!left && !right);

        // after closing down to three sheets, the start comes back too
        let (start, entries, _, _) = WinInfo::layout_sheet_bar(&n[..3], 2, 2, 30);
        assert_eq!(start, 0);
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn closing_the_last_sheet_keeps_the_backtick_target() {
        let (mut csvs, mut win) = sheets_fixture(&["a", "b", "c"]);
        csvs.set_handle(2);
        win.set_context(csvs.get_context());
        assert_eq!(csvs.prev, Some(0));

        type_command(&mut win, "sh c");
        win.process_command(&mut csvs);
        assert_eq!((csvs.handle, csvs.prev), (1, Some(0)));
    }

    #[test]
    fn prepend_and_append_skip_blank_cells() {
        let mut cells = cells_with_rows(
            "affix.csv",
            vec!["a", "b"],
            vec![vec!["x", "1"], vec!["", "2"], vec!["y", ""]],
        );
        let mut win = win_for(&mut cells);

        win.col_pre(&mut cells, "<");
        win.col_app(&mut cells, "'>'");
        let col: Vec<String> = (0..3)
            .map(|i| cells.get_column(0).get_cell(i).content.clone())
            .collect();
        assert_eq!(col, vec!["<x>", "", "<y>"]);
    }

    #[test]
    fn splice_accepts_a_sheet_name() {
        let first = cells_with_rows("first.csv", vec!["a"], vec![vec!["one"]]);
        let second = cells_with_rows("second.csv", vec!["a"], vec![vec!["two"]]);
        let mut csvs = Csvs::new(vec![Context::new(0, first), Context::new(1, second)]);
        let mut win = command_win();
        win.set_context(csvs.get_context());

        type_command(&mut win, "sh sp r second 0");
        win.process_command(&mut csvs);
        assert_eq!(csvs.get_cells().num_rows(), 2);

        // the focused sheet itself is refused
        type_command(&mut win, "sh sp r first 0");
        win.process_command(&mut csvs);
        assert_eq!(csvs.get_cells().num_rows(), 2);
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
        sa_winch.sa_sigaction = sig_winch as *const () as usize;
        libc::sigemptyset(&mut sa_winch.sa_mask);
        sa_winch.sa_flags = 0;
        libc::sigaction(libc::SIGWINCH, &sa_winch, std::ptr::null_mut());

        //SIGINT
        let mut sa_int: libc::sigaction = std::mem::zeroed();
        sa_int.sa_sigaction = sig_int as *const () as usize;
        libc::sigemptyset(&mut sa_int.sa_mask);
        sa_int.sa_flags = 0;
        libc::sigaction(libc::SIGINT, &sa_int, std::ptr::null_mut());
        
        //SIGQUIT
        let mut sa_quit: libc::sigaction = std::mem::zeroed();
        sa_quit.sa_sigaction = sig_quit as *const () as usize;
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
