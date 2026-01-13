use std::io::{self, Read, Write, stdin};

mod csv;
mod input;
mod terminal;

use crate::csv::{Cells, load_csv, show_csv};
use crate::input::process_input;
use crate::terminal::{check_flags, WinInfo};

fn main() {
    let mut cells: Cells = match load_csv() {
        Ok(file) => file,
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

    let mut buffer = [0u8; 16];

    // main loop
    loop {
        show_csv(&mut cells, &mut w_info);
        check_flags(&mut w_info);
        match std::io::stdin().read(&mut buffer) {
            Ok(n) => {
                let input = &buffer[..n];
                process_input(input, &mut w_info, &mut cells);
            }
            _ => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }

    // restore terminal
    terminal::raw_mode(false);
}
