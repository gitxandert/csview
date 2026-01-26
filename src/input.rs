use crate::cells::Cells;
use crate::terminal::{ScrollMode, WinChange, WinInfo};

pub fn process_input(input: &[u8], w_info: &mut WinInfo, cells: &mut Cells) {
    if !w_info.writing {
        match input {
            // normal arrows
            [27, 91, 65] => { // up
                w_info.set_h_pointer(w_info.h_pointer.saturating_sub(1));
            }
            [27, 91, 66] => { // down
                w_info.set_h_pointer(w_info.h_pointer + 1);
            }
            [27, 91, 67] => { // right
                w_info.set_w_pointer(w_info.w_pointer + 1);
            }
            [27, 91, 68] => { // left
                w_info.set_w_pointer(w_info.w_pointer.saturating_sub(1));
            }
            // modified arrows
            [27, 91, 49, 59, m, d] => {
                let mut winch = false;
                match m {
                    50 => w_info.set_mode(ScrollMode::Axis),
                    51 => w_info.set_mode(ScrollMode::Text),
                    53 => w_info.set_mode(ScrollMode::Page),
                    54 => winch = true,
                    _ => (),
                }
                match d {
                    65 => { // up
                        match w_info.mode {
                            ScrollMode::Axis => {
                                w_info.set_h_offset(
                                    w_info.h_offset.saturating_sub(1)
                                );
                            }
                            ScrollMode::Page => {
                                w_info.set_h_pointer(
                                    w_info.h_pointer.saturating_sub(w_info.h_page)
                                );
                            }
                            _ => (),
                        }
                    }
                    66 => { // down
                        match w_info.mode {
                            ScrollMode::Axis => {
                                w_info.set_h_offset(
                                    w_info.h_offset + 1
                                );
                            }
                            ScrollMode::Page => {
                                w_info.set_h_pointer(
                                    w_info.h_pointer + w_info.h_page
                                );
                            }
                            _ => (),
                        }
                    }
                    67 => { // right
                        if winch {
                            let col = cells.get_column(
                                cells.w_cell.0
                            );
                            let new_width = col.width + 1;
                            // only change col_width if the current cell doesn't go out of bounds
                            if col.start + new_width + 3 < w_info.width {
                            
                                cells.set_column_width(new_width);
                                w_info.changed = WinChange::ColWidth;

                            }
                        } else {
                            match w_info.mode {
                                ScrollMode::Text => {
                                    let mut w_cell = cells.w_cell();
                                    w_cell.set_text_offset(
                                        w_cell.text_offset + 1
                                    );
                                    w_info.changed = WinChange::Cell;
                                }
                                ScrollMode::Axis => {
                                    w_info.set_w_offset(
                                        w_info.w_offset + 1
                                    );
                                }
                                ScrollMode::Page => {
                                    w_info.set_w_pointer(
                                        w_info.w_pointer + w_info.w_page
                                    );
                                }
                                _ => (),
                            }
                        }
                    }
                    68 => { // left
                        if winch {
                            let col = cells.get_column(
                                cells.w_cell.0
                            );
                            let width = col.width;
                            cells.set_column_width(width.saturating_sub(1));
                            w_info.changed = WinChange::ColWidth;
                        } else {
                            match w_info.mode {
                                ScrollMode::Text => {
                                    let mut w_cell = cells.w_cell();
                                    w_cell.set_text_offset(
                                        w_cell.text_offset.saturating_sub(1)
                                    );
                                    w_info.changed = WinChange::Cell;
                                }
                                ScrollMode::Axis => {
                                    w_info.set_w_offset(
                                        w_info.w_offset.saturating_sub(1)
                                    );
                                }
                                ScrollMode::Page => {
                                    w_info.set_w_pointer(
                                        w_info.w_pointer.saturating_sub(w_info.w_page)
                                    );
                                }
                                _ => (),
                            }
                        }
                    }
                    _ => (),
                }
            }
            // ctrl + w (write)
            [23] => {
                w_info.set_write_buffer_w_cell(&cells.w_cell());
                w_info.set_writing(true);
                w_info.changed = WinChange::Write;
            }
            _ => (),
        }
    } else {
        // writing to cell
        match input {
            // normal arrows
            [27, 91, 65] => { // up
                // maybe used to shift back by the cell's width
            }
            [27, 91, 66] => { // down
                // sim. but forward
            }
            [27, 91, 67] => { // right
                // scrolls cursor right within a cell
                w_info.move_cursor_right();
            }
            [27, 91, 68] => { // left
                // sim. but left
                w_info.move_cursor_left();
            }
            [27] => { // escape by itself
                // ignore for now
            }
            [27, 91, 90] => { // shift-tab
                // ignore for now
            }
            // modified arrows
            [27, 91, 49, 59, m, d] => {
                match m {
                    // affect scroll speed
                    _ => (),
                }
                match d {
                    65 => (),
                    66 => (),
                    67 => (),
                    68 => (),
                    _ => (),
                }
            }
            // ctrl + w (write)
            [23] => {
                w_info.set_writing(false);
                let mut w_cell = cells.w_cell();
                w_info.write_to_cell(w_cell);
            }
            [1..=22] | [24..=26] | [31] => {
                // ignore other control characters
            }
            [8] | [127] => { /* backspace */ }
            [27, 91, 50, 126] => { /* insert */ }
            [27, 91, 51, 126] => { /* delete */ }
            [27, 91, 70] => { /* end */ }
            [27, 91, 72] => { /* home */ }
            [27, 91, 50, 59, 53, 126] => { /* ctrl + insert */ }
            [27, 91, 51, 59, 53, 126] => { /* ctrl + delete */ }
            [27, 91, 49, 59, 53, 70] => { /* ctrl + end */ }
            [27, 91, 49, 59, 53, 72] => { /* ctrl + home */ }
            _ => {
                let c = match str::from_utf8(input) {
                    Ok(valid) => valid,
                    Err(_) => {
                        eprintln!("Invalid input {:?}", input);
                        return;
                    }
                };
                w_info.to_write_buffer(c);
                cells.written = true;
            }
        }
    }
}
