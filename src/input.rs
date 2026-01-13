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
        match input {
            // normal arrows
            [27, 91, 65] => { // up
                // can maybe use to shift back by the cell's width
            }
            [27, 91, 66] => { // down
                // opposite of above
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
                    65 => (), // beginning of cell
                    66 => (), // end of cell
                    67 => (), // forward by a word
                    68 => (), // back by a word
                    _ => (),
                }
            }
            // ctrl + w (write)
            [23] => {
                w_info.set_writing(false);
            }
            _ => (),
        }
    }
}
