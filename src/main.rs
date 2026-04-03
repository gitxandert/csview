use std::{
    env,
    io::{self, Write},
};

mod cells;
mod cmd_err;
mod csv_io;
mod input;
mod terminal;

use crate::{
    cells::{Context, Csvs},
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
    w_info.print_context(&mut csvs);
    w_info.show_csv(csvs.get_cells());
    
    // main loop
    let mut buffer = [0u8; 16];
    loop {
        match poll_stdin(&mut buffer) {
            Ok(PollEvent::Data(0)) => continue,
            Ok(PollEvent::Data(n)) => {
                let input = &buffer[..n];
                if !input.is_empty() {
                    match process_input(input, &mut w_info, &mut csvs) {
                        SigFlag::Quit => break,
                        _ => w_info.show_csv(csvs.get_cells()),
                    }
                }
            }
            Ok(PollEvent::Sig) => {
                match check_flags() {
                    SigFlag::Winch => {
                        w_info.set_w_h(&mut csvs);
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
            print!("\nWrite {} to file? [y/N/s(ave as)]: ", cells.filename);
            io::stdout().flush().unwrap();
            let mut input = String::new();
            io::stdin()
                .read_line(&mut input)
                .expect("Failed to read line");
            cells.filename = match input.trim_end().to_lowercase().as_str() {
                "y" | "yes" => {
                    // save backup to /home/user/.csview/backups/
                    // only if retaining filename
                    match save_backup(&cells.filename) {
                        Ok(b) => {
                            println!("\t{b}");
                        }
                        Err(e) => {
                            println!("\tWARNING -- could not create backup due to the following:");
                            println!("\t\t{e}");
                        }
                    }
                    cells.filename
                },
                "s" | "save as" => {
                    print!("Enter new filename: ");
                    io::stdout().flush().unwrap();
                    input = String::new();
                    io::stdin()
                        .read_line(&mut input)
                        .expect("Failed to read line");
                    input.trim_end().to_string()
                },
                _ => continue,
            };
            match write_to_file(&mut cells) {
                Ok(s) => println!("\t{s}"),
                Err(e) => println!("\t{e}"),
            }
        }
    }
}
