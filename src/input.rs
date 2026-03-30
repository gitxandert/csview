use crate::{
    cells::{Cells, Context, Csvs},
    csv_io::{save_backup, write_to_file},
    terminal::{InputMode, ScrollMode, SigFlag, WinChange, WinInfo},
};

pub fn process_input(
    input: &[u8], 
    w_info: &mut WinInfo, 
    csvs: &mut Csvs, 
) -> SigFlag {
    match w_info.input_mode {
        InputMode::Scroll => {
            match input {
                [48..=57] => {
                    let id = input[0] as usize - 48;
                    if csvs.handle != id
                    && id < csvs.num_contexts() {
                        csvs.save_context(w_info);
                        csvs.set_handle(id);
                        w_info.set_context(
                            csvs.get_context()
                        );
                        w_info.draw_screen(csvs.get_cells());
                        w_info.print_context(csvs);
                        w_info.draw_focused_content();
                        w_info.flush();
                    }
                }
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
                        50 => w_info.scroll_mode = ScrollMode::Axis,
                        51 => w_info.scroll_mode = ScrollMode::Text,
                        53 => w_info.scroll_mode = ScrollMode::Page,
                        54 => winch = true,
                        _ => (),
                    }
                    match d {
                        65 => { // up
                            match w_info.scroll_mode {
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
                            match w_info.scroll_mode {
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
                                let cells = csvs.get_cells();
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
                                match w_info.scroll_mode {
                                    ScrollMode::Text => {
                                        let cells = csvs.get_cells();
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
                                let cells = csvs.get_cells();
                                let col = cells.get_column(
                                    cells.w_cell.0
                                );
                                let width = col.width;
                                cells.set_column_width(width.saturating_sub(1));
                                w_info.changed = WinChange::ColWidth;
                            } else {
                                match w_info.scroll_mode {
                                    ScrollMode::Text => {
                                        let cells = csvs.get_cells();
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
                // ctrl + s (save)
                [19] => {
                    let cells = csvs.get_cells();
                    match save_backup(&cells.filename) {
                        Ok(b) => {
                            match write_to_file(cells) {
                                Ok(s) => {
                                    cells.written = false;
                                    w_info.push_str_to_frame(
                                        &format!(
                                            "\x1b[{};1H\x1b[2K\x1b[0m{}", 
                                            w_info.height, s.chars().take(w_info.width).collect::<String>()
                                        )
                                    );
                                    w_info.flush();
                                }
                                Err(e) => {
                                    let es = e.to_string();
                                    w_info.push_str_to_frame(
                                        &format!(
                                            "\x1b[{};1H\x1b[2K\x1b[0m{}",
                                            w_info.height, es.chars().take(w_info.width).collect::<String>()
                                        )
                                    );
                                    w_info.flush();
                                }
                            }
                        }
                        Err(e) => {
                            let es = e.to_string();
                            w_info.push_str_to_frame(
                                &format!(
                                    "\x1b[{};1H\x1b[2K\x1b[0m{}",
                                    w_info.height, es.chars().take(w_info.width).collect::<String>()
                                )
                            );
                            w_info.flush();
                        }
                    }
                }
                // ctrl/alt + w (write)
                [23] | [27, 119] => {
                    let cells = csvs.get_cells();
                    w_info.set_write_buffer_w_cell(&cells.w_cell());
                    w_info.set_write_mode(true);
                    w_info.changed = WinChange::Write;
                }
                // y (yank)
                [121] => {
                    let cell = csvs.get_cells().w_cell();
                    w_info.set_yanked(cell);
                    w_info.push_str_to_frame(
                        &format!(
                            "\x1b[{};1H\x1b[2K\x1b[0myanked '{}'",
                            w_info.height, cell.content
                        )
                    );
                    w_info.flush();
                }
                // p (paste)
                [112] => {
                    if w_info.yanked == "" {
                        w_info.push_str_to_frame(
                            &format!(
                                "\x1b[{};1H\x1b[2K\x1b[0mNothing has been yanked",
                                w_info.height
                            )
                        );
                    } else {
                        w_info.paste(csvs.get_cells());
                        w_info.push_str_to_frame(
                            &format!(
                                "\x1b[{};1H\x1b[2K\x1b[0mpasted '{}' to cell",
                                w_info.height, csvs.get_cells().w_cell().content
                            )
                        );
                    }
                    w_info.flush();
                }
                // : (command)
                [58] => {
                    let cells = csvs.get_cells();
                    let col = cells.get_column(w_info.w_pointer);
                    w_info.set_command_mode(true, &col);
                }
                _ => (),
            }
        } 
        InputMode::Write => {
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
                    w_info.changed = WinChange::Write;
                }
                [27, 91, 68] => { // left
                    // sim. but left
                    w_info.move_cursor_left();
                    w_info.changed = WinChange::Write;
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
                // ctrl/alt + w (write)
                [23] | [27, 119] => {
                    let cells = csvs.get_cells();
                    w_info.set_write_mode(false);
                    let mut w_cell = cells.w_cell();
                    w_info.write_to_cell(w_cell);
                }
                [1..=22] | [24..=26] | [31] => {
                    // ignore other control characters
                }
                [8] | [127] => { /* backspace */ 
                    let cells = csvs.get_cells();
                    w_info.delete_from_write_buffer();
                    w_info.changed = WinChange::Write;
                    cells.written = true;
                }
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
                            w_info.push_str_to_frame(
                                &format!(
                                    "\x1b[{};1HInvalid input {:?}", 
                                    w_info.height,
                                    input
                                )
                            );
                            w_info.flush();
                            return SigFlag::Non;
                        }
                    };
                    w_info.add_to_write_buffer(c);
                    w_info.changed = WinChange::Write;
                    csvs.get_cells().written = true;
                }
            }
        }
        InputMode::Command => {
            match input {
                // normal arrows
                [27, 91, 65] => { // up
                }
                [27, 91, 66] => { // down
                }
                [27, 91, 67] => { // right
                    // scrolls cursor right
                    w_info.move_cursor_right();
                    w_info.changed = WinChange::Command;
                }
                [27, 91, 68] => { // left
                    // sim. but left
                    w_info.move_cursor_left();
                    w_info.changed = WinChange::Command;
                }
                [27] => { //escape 
                    // causes issues; ignore
                }
                [16] => { // ctrl + p (paste)
                    w_info.paste_yanked_to_write_buffer();
                    w_info.changed = WinChange::Command;
                }
                [17] => { // ctrl + q (quit)
                    w_info.push_str_to_frame(
                        &format!(
                            "\x1b[{};1H\x1b[2K\x1b[0m",
                            w_info.height
                        )
                    );
                    w_info.draw_focused_content();
                    w_info.flush();

                    let cells = csvs.get_cells();
                    let col = cells.get_column(w_info.w_pointer);
                    w_info.set_command_mode(false, &col);
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
                [1..10] | [11..13] | [14..26] | [31] => {
                    // ignore other control characters
                }
                [8] | [127] => { /* backspace */ 
                    w_info.delete_from_write_buffer();
                    w_info.changed = WinChange::Command;
                }
                [10] | [13] => { /* enter (\n, \r) */
                    match w_info.process_command(csvs) {
                        SigFlag::Quit => return SigFlag::Quit,
                        _ => {
                            let cells = csvs.get_cells();
                            let col = cells.get_column(w_info.w_pointer);
                            w_info.set_command_mode(false, &col);
                        }
                    }
                }
                [13, 10] => { /* enter (\r\n) */
                    match w_info.process_command(csvs) {
                        SigFlag::Quit => return SigFlag::Quit,
                        _ => {
                            let cells = csvs.get_cells();
                            let col = cells.get_column(w_info.w_pointer);
                            w_info.set_command_mode(false, &col);
                        }
                    }
                }
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
                            w_info.push_str_to_frame(
                                &format!(
                                    "\x1b[{};1HInvalid input {:?}", 
                                    w_info.height,
                                    input
                                )
                            );
                            w_info.flush();
                            return SigFlag::Non;
                        }
                    };
                    w_info.add_to_write_buffer(c);
                    w_info.changed = WinChange::Command;
                }
            }
        }
    }
    SigFlag::Non
}
