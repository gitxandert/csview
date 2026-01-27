use std::io::{self, Read, Write};

pub enum CmdErr {
    MissingName,
    UnknownSpec,
}

pub fn print(err: CmdErr, token: &str, line: usize) {
    let mut out = io::stdout();
    let mut outstring = String::new();
    let outstring = {
        match err {
            CmdErr::MissingName => format!(
                    "\x1b[{};1H\x1b[2KERR: missing name for '{}'",
                    line, token
                ),

            CmdErr::UnknownSpec => format!(
                    "\x1b[{};1H\x1b[2KERR: unknown specifier '{}'",
                    line, token
                ),
        }
    };

    write!(out, "{outstring}");
    out.flush().unwrap();
}
