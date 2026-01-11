mod csv;
mod input;
mod terminal;

use crate::csv::{Cells, load_csv, show_csv};
use crate::input::{find_kbd, process_input};
use crate::terminal::{check_flags, WinInfo};

fn main() {
    let mut cells: Cells = match load_csv() {
        Ok(file) => file,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };

    let mut kbd = match find_kbd() {
        Ok(dev) => dev,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };

    let (max_w, max_h) = cells.xy();
    let mut w_info = WinInfo::new(max_w, max_h);

    terminal::install_panic_hook();
    terminal::install_sig_handlers();
    terminal::raw_mode(true);
    terminal::set_w_h();

    // main loop
    loop {
        show_csv(&mut cells, &mut w_info);
        check_flags(&mut w_info);
        process_input(&mut w_info, &mut kbd, &mut cells);
    }

    // restore terminal
    terminal::raw_mode(false);
}
