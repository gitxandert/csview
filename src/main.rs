use std::{
    env,
    io::{self, Read, Write, stdin},
};

mod cells;
mod cmd_err;
mod csv_io;
mod input;
mod terminal;

use crate::{
    cells::{Cells, show_csv},
    input::process_input,
    terminal::{check_flags, WinInfo},
    csv_io::{load_csv, save_backup, write_to_file},
};

fn main() {
    let mut args = env::args();
    if args.len() < 2 {
        eprintln!("csview expects at least a file name");
        return;
    }

    let mut filename = String::new();
    let mut delimiter: char = ',';

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-d" | "--delimiter" => {
                match args.next() {
                    Some(d) => {
                        if d.len() > 1 {
                            eprintln!("Argument to -d/--delimiter must be a single character t for tab)");
                            return;
                        }
                        if d == "t" {
                            delimiter = '\t';
                        } else {
                            delimiter = d.chars().nth(0).unwrap();
                        }
                    }
                    None => {
                        eprintln!("No argument provided for -d/--delimiter");
                        return;
                    }
                }
            }
            _ => filename = arg,
        }
    }

    let mut cells: Cells = match load_csv(filename.clone(), delimiter.clone()) {
        Ok(file) => file,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };

    let max_w = cells.num_cols();
    let max_h = cells.num_rows();
    let mut w_info = WinInfo::new(max_w, max_h);

    terminal::install_panic_hook();
    terminal::install_sig_handlers();
    terminal::raw_mode(true);

    let mut buffer = [0u8; 16];

    // main loop
    loop {
        if check_flags(&mut w_info) {
            break;
        }
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

        show_csv(&mut cells, &mut w_info);
    }

    // restore terminal
    terminal::raw_mode(false);
    if cells.written {
        println!("Write to file? [y/N]: ");
        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .expect("Failed to read line");
        match input.trim_end() {
            "y" | "yes" | "Y" | "Yes" | "YES" => {
                    // save backup to 
                    // /home/user/.csview/backups/;
                    // if fails, don't write
                    match save_backup(filename.clone()) {
                        Ok(()) => write_to_file(cells, filename, delimiter),
                        Err(e) => {
                            eprintln!("WARNING -- could not create back up due to the following:");
                            eprintln!("\n\t{e}");
                            eprintln!("Exiting without writing");
                    }
                }
            }
            _ => println!("Exiting without writing"),
        }
    }
}
