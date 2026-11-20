use std::{
    env,
    io::{self, Write},
};

mod cells;
mod cmd_err;
mod csv_io;
mod history;
mod input;
mod platform;
mod terminal;

use crate::{
    cells::{Context, Csvs},
    input::{process_input, split_keys},
    terminal::{check_flags, is_delimiter, SigFlag, WinInfo},
    csv_io::{blank_csv, cmd_history_path, load_csv, save_backup, write_to_file},
    platform::{poll_stdin, PollEvent, RawMode},
};

fn main() {
    let mut args = env::args();

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

    let mut w_info = WinInfo::new();
    // no file given: start on an empty sheet that fills the screen
    if contexts.is_empty() {
        let rows = w_info.height.saturating_sub(4);
        contexts.push(Context::new(0, blank_csv("Untitled.csv".to_string(), delimiter, rows)));
    }

    terminal::install_panic_hook();
    terminal::install_sig_handlers();
    terminal::raw_mode(true);

    let mut csvs = Csvs::new(contexts);
    if let Some(path) = cmd_history_path() {
        w_info.set_cmd_history_file(path);
    }
    w_info.set_context(csvs.get_context());
    w_info.show_workspace(&mut csvs);
    
    // main loop
    let mut buffer = [0u8; 1024];
    // bytes read so far that don't make up a whole key yet
    let mut pending = Vec::<u8>::new();
    'main: loop {
        match poll_stdin(&mut buffer) {
            Ok(PollEvent::Data(0)) => continue,
            Ok(PollEvent::Data(n)) => {
                pending.extend_from_slice(&buffer[..n]);
                let (keys, rest) = split_keys(&pending);
                for key in keys {
                    match process_input(key, &mut w_info, &mut csvs) {
                        SigFlag::Quit => break 'main,
                        _ => w_info.show_workspace(&mut csvs),
                    }
                }
                pending.drain(..pending.len() - rest);
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
            println!();
            let input = edit_line(&format!("Write {} to file? [y/N/s(ave as)]: ", cells.filename));
            cells.filename = match input.trim_end().to_lowercase().as_str() {
                "y" | "yes" => {
                    // save backup to /home/user/.csview/backups/
                    // only if retaining filename and there's a file to back up
                    if std::path::Path::new(&cells.filename).exists() {
                        match save_backup(&cells.filename) {
                            Ok(b) => {
                                println!("\t{b}");
                            }
                            Err(e) => {
                                println!("\tWARNING -- could not create backup due to the following:");
                                println!("\t\t{e}");
                            }
                        }
                    }
                    cells.filename
                },
                "s" | "save as" => {
                    let name = edit_line("Enter new filename: ");
                    match name.trim_end() {
                        "" => continue,
                        name => name.to_string(),
                    }
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

// read one line in raw mode so arrows, home/end and ctrl + arrow word jumps work while typing;
// ctrl + c cancels with an empty line
fn edit_line(prompt: &str) -> String {
    let Some(orig) = platform::enter_raw(RawMode::Line) else {
        // not a terminal; fall back to a plain read
        print!("{prompt}");
        io::stdout().flush().unwrap();
        let mut input = String::new();
        let _ = io::stdin().read_line(&mut input);
        return input;
    };

    let mut line = Vec::<char>::new();
    let mut pos = 0usize;
    let mut buf = [0u8; 16];
    let mut out = io::stdout();
    loop {
        let shown: String = line.iter().collect();
        let col = prompt.chars().count() + pos + 1;
        let _ = write!(out, "\r\x1b[2K{prompt}{shown}\x1b[{col}G");
        let _ = out.flush();

        let n = match platform::read_stdin(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        match &buf[..n] {
            [13] | [10] | [13, 10] => break,
            [3] | [4] => { line.clear(); break; } // ctrl + c / ctrl + d
            [27, 91, 67] => pos = (pos + 1).min(line.len()), // right
            [27, 91, 68] => pos = pos.saturating_sub(1), // left
            [27, 91, 72] | [27, 79, 72] | [27, 91, 49, 126] | [27, 91, 55, 126] => pos = 0, // home
            [27, 91, 70] | [27, 79, 70] | [27, 91, 52, 126] | [27, 91, 56, 126] => pos = line.len(), // end
            [27, 91, 49, 59, 53, 67] => { // ctrl + right (jump to next word)
                while pos < line.len() && !is_delimiter(line[pos]) { pos += 1; }
                while pos < line.len() && is_delimiter(line[pos]) { pos += 1; }
            }
            [27, 91, 49, 59, 53, 68] => { // ctrl + left (jump to previous word)
                while pos > 0 && is_delimiter(line[pos - 1]) { pos -= 1; }
                while pos > 0 && !is_delimiter(line[pos - 1]) { pos -= 1; }
            }
            [127] | [8] => { // backspace
                if pos > 0 { pos -= 1; line.remove(pos); }
            }
            [27, 91, 51, 126] => { // delete
                if pos < line.len() { line.remove(pos); }
            }
            [27, ..] | [0..=31] => (), // other escapes and control chars
            bytes => {
                if let Ok(s) = std::str::from_utf8(bytes) {
                    for c in s.chars() { line.insert(pos, c); pos += 1; }
                }
            }
        }
    }

    platform::restore(&orig);
    println!();
    line.into_iter().collect()
}
