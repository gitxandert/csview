use crate::csv::Cells;
use crate::terminal::{ScrollMode, WinInfo};

pub fn process_input(input: &[u8], w_info: &mut WinInfo, cells: &mut Cells) {
    match input {
        // normal arrows
        [27, 91, 65] => { // up
            if w_info.mode() != ScrollMode::Cursor {
                w_info.set_mode(ScrollMode::Cell);
            }
            w_info.h_offset_up();
        }
        [27, 91, 66] => { // down
            if w_info.mode() != ScrollMode::Cursor {
                w_info.set_mode(ScrollMode::Cell);
            }
            w_info.h_offset_down();
        }
        [27, 91, 67] => { // right
            if w_info.mode() != ScrollMode::Cursor {
                w_info.set_mode(ScrollMode::Cell);
            }
            w_info.w_offset_right(cells);
        }
        [27, 91, 68] => { // left
            if w_info.mode() != ScrollMode::Cursor {
                w_info.set_mode(ScrollMode::Cell);
            }
            w_info.w_offset_left(cells);
        }
        // modified arrows
        [27, 91, 49, 59, m, d] => {
            match m {
                50 => w_info.set_mode(ScrollMode::Axis),
                51 => w_info.set_mode(ScrollMode::Text),
                53 => w_info.set_mode(ScrollMode::Page),
                _ => w_info.set_mode(ScrollMode::Cell),
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
            // also toggles ScrollMode::Cursor and ScrollMode::Cell
            w_info.toggle_cursor();
        }
        _ => (),

    }
}
