use std::{
    env,
    io::{self, Read, Write},
};

mod cells;
mod cmd_err;
mod csv_io;
mod history;
mod input;
mod terminal;

use crate::{
    cells::{Context, Csvs},
    input::process_input,
    terminal::{check_flags, is_delimiter, SigFlag, WinInfo},
    csv_io::{
        cmd_history_path, load_csv, 
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
    if let Some(path) = cmd_history_path() {
        w_info.set_cmd_history_file(path);
    }
    w_info.set_context(csvs.get_context());
    w_info.draw_context(&mut csvs);
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
            println!();
            let input = edit_line(&format!("Write {} to file? [y/N/s(ave as)]: ", cells.filename));
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

// read one line in raw mode so arrows, home/end and tab jumps work while typing;
// ctrl + c cancels with an empty line
fn edit_line(prompt: &str) -> String {
    use libc::{cfmakeraw, tcgetattr, tcsetattr, termios, STDIN_FILENO, TCSANOW};

    let mut orig: termios = unsafe { std::mem::zeroed() };
    if unsafe { tcgetattr(STDIN_FILENO, &mut orig) } != 0 {
        // not a terminal; fall back to a plain read
        print!("{prompt}");
        io::stdout().flush().unwrap();
        let mut input = String::new();
        let _ = io::stdin().read_line(&mut input);
        return input;
    }
    let mut raw = orig;
    unsafe {
        cfmakeraw(&mut raw);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        tcsetattr(STDIN_FILENO, TCSANOW, &raw);
    }

    let mut line = Vec::<char>::new();
    let mut pos = 0usize;
    let mut buf = [0u8; 16];
    let mut out = io::stdout();
    loop {
        let shown: String = line.iter().collect();
        let col = prompt.chars().count() + pos + 1;
        let _ = write!(out, "\r\x1b[2K{prompt}{shown}\x1b[{col}G");
        let _ = out.flush();

        let n = match io::stdin().read(&mut buf) {
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
            [9] => { // tab (jump to next word)
                while pos < line.len() && !is_delimiter(line[pos]) { pos += 1; }
                while pos < line.len() && is_delimiter(line[pos]) { pos += 1; }
            }
            [27, 91, 90] => { // shift-tab (jump to previous word)
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

    unsafe { tcsetattr(STDIN_FILENO, TCSANOW, &orig); }
    println!();
    line.into_iter().collect()
}
