use crate::csv::Cells;
use crate::terminal::{ScrollMode, WinInfo};

pub fn process_input(input: &[u8], w_info: &mut WinInfo, cells: &mut Cells) {
    if !w_info.writing() {
        match input {
            // normal arrows
            [27, 91, 65] => { // up
                w_info.set_mode(ScrollMode::Cell);
                w_info.h_offset_up();
            }
            [27, 91, 66] => { // down
                w_info.set_mode(ScrollMode::Cell);
                w_info.h_offset_down();
            }
            [27, 91, 67] => { // right
                w_info.set_mode(ScrollMode::Cell);
                w_info.w_offset_right(cells);
            }
            [27, 91, 68] => { // left
                w_info.set_mode(ScrollMode::Cell);
                w_info.w_offset_left(cells);
            }
            // modified arrows
            [27, 91, 49, 59, m, d] => {
                match m {
                    50 => w_info.set_mode(ScrollMode::Axis),
                    51 => w_info.set_mode(ScrollMode::Text),
                    53 => w_info.set_mode(ScrollMode::Page),
                    _ => (),
                }
                match d {
                    65 => w_info.h_offset_up(),
                    66 => w_info.h_offset_down(),
                    67 => w_info.w_offset_right(cells),
                    68 => w_info.w_offset_left(cells),
                    _ => (),
                }
            }
            // ctrl + w (write)
            [23] => {
                w_info.set_writing(true);
            }
            _ => (),
        }
    } else {
        // writing to cell
        match input {
            // normal arrows
            [27, 91, 65] => { // up
                // if cell has lines separated by \n,
                // this scrolls through them; otherwise, it can
                // maybe be used to shift back by the cell's width
            }
            [27, 91, 66] => { // down
                // sim.
            }
            [27, 91, 67] => { // right
                // scrolls cursor right within a cell
            }
            [27, 91, 68] => { // left
                // scrolls cursor left within a cell
            }
            // modified arrows
            [27, 91, 49, 59, m, d] => {
                match m {
                    // affects scroll speed
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
            }
            [8] | [127] => {
                let cur_pos = w_info.get_cursor_offset();
                cells.delete_from_cell(cur_pos);
                w_info.set_cursor_offset(cur_pos.saturating_sub(1));
            }
            _ => {
                let c = match str::from_utf8(input) {
                    Ok(valid) => valid,
                    Err(_) => {
                        eprintln!("Invalid input {:?}", input);
                        return;
                    }
                };
                let cur_pos = w_info.get_cursor_offset();
                cells.write_to_cell(c.to_string(), cur_pos);
                w_info.set_cursor_offset(cur_pos + 1);
            }
        }
    }
}
