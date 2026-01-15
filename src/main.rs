use std::env;
use std::io::{self, Read, Write, stdin};

mod csv;
mod input;
mod terminal;

use crate::csv::{Cells, load_csv, show_csv, write_to_file};
use crate::input::process_input;
use crate::terminal::{check_flags, WinInfo};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("csview expects one filename argument (e.g. file.csv)");
        return;
    }

    let filename = &args[1];

    let mut cells: Cells = match load_csv(filename.clone()) {
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
        if check_flags(&mut w_info) > 0 {
            break;
        }
        show_csv(&mut cells, &mut w_info);
        match std::io::stdin().read(&mut buffer) {
            Ok(n) => {
                let input = &buffer[..n];
                if !input.is_empty() {
                    process_input(input, &mut w_info, &mut cells);
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
            _ => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }

    // restore terminal
    terminal::raw_mode(false);
    write_to_file(cells, filename.to_string());
}
