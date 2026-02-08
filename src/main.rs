use std::{
    env,
    ptr,
    io::{self, Read, Write, stdin},
};

mod cells;
mod cmd_err;
mod csv_io;
mod input;
mod terminal;

use crate::{
    cells::{Cells, Context, Csvs},
    input::process_input,
    terminal::{check_flags, SigFlag, WinInfo},
    csv_io::{
        load_csv, 
        poll_stdin, PollEvent, 
        save_backup, write_to_file
    },
};

fn main() {
    let mut args = env::args();
    if args.len() < 2 {
        println!("csview expects at least a file name");
        return;
    }

    // skip the program name
    let _ = args.next();

    let mut filenames = Vec::<String>::new();
    let mut delimiter: char = ',';

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-d" | "--delimiter" => {
                match args.next() {
                    Some(d) => {
                        if d.len() > 1 {
                            println!("Argument to -d/--delimiter must be a single character t for tab)");
                            return;
                        }
                        if d == "t" {
                            delimiter = '\t';
                        } else {
                            delimiter = d.chars().nth(0).unwrap();
                        }
                    }
                    None => {
                        println!("No argument provided for -d/--delimiter");
                        return;
                    }
                }
            }
            _ => filenames.push(arg),
        }
    }

    let mut contexts = Vec::<Context>::new();
    let mut id = 0usize;
    for f in filenames {
        match load_csv(f.clone(), delimiter.clone()) {
            Ok(csv) => {
                contexts.push(Context::new(id, csv));
                id += 1;
            }
            Err(e) => {
                println!("Error loading csv: {e}");
                return;
            }
        };
    }

    terminal::install_panic_hook();
    terminal::install_sig_handlers();
    terminal::raw_mode(true);

    let mut csvs = Csvs::new(contexts);
    let mut w_info = WinInfo::new();
    w_info.set_context(csvs.get_context());
    w_info.show_csv(csvs.get_cells());
    
    // main loop
    let mut buffer = [0u8; 16];
    loop {
        match poll_stdin(&mut buffer) {
            Ok(PollEvent::Data(0)) => continue,
            Ok(PollEvent::Data(n)) => {
                let input = &buffer[..n];
                if !input.is_empty() {

                    process_input(input, &mut w_info, &mut csvs);
                    w_info.show_csv(csvs.get_cells());
                }
            }
            Ok(PollEvent::Sig) => {
                match check_flags() {
                    SigFlag::Winch => {
                        w_info.set_w_h(csvs.get_cells());
                    }
                    SigFlag::Int | SigFlag::Quit => break,
                    SigFlag::Non => continue,
                }
            }
            Err(e) => {
                w_info.push_str_to_frame(
                    &format!(
                        "\x1b[{};1H\x1b[2K\x1b[0mERR: {}",
                        w_info.height, e
                    )
                );
                w_info.flush();
            }
        }
    }

    // restore terminal
    terminal::raw_mode(false);
    for con in csvs.contexts {
        let mut cells = con.cells;
        if cells.written {
            println!("Write {} to file? [y/N]: ", cells.filename);
            let mut input = String::new();
            io::stdin()
                .read_line(&mut input)
                .expect("Failed to read line");
            match input.trim_end() {
                "y" | "yes" | "Y" | "Yes" | "YES" => {
                    // save backup to 
                    // /home/user/.csview/backups/;
                    // if fails, don't write
                    match save_backup(&cells.filename) {
                        Ok(b) => {
                            println!("\t{b}");
                            match write_to_file(&mut cells) {
                                Ok(s) => println!("\t{s}"),
                                Err(e) => println!("\t{e}"),
                            }
                        }
                        Err(e) => {
                            println!("\tWARNING -- could not create back up due to the following:");
                            println!("\t{e}");
                        }
                    }
                }
                _ => (),
            }
        }
    }
}
