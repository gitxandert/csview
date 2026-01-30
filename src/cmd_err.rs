use std::io::{self, Read, Write};

pub enum CmdErr {
    InvalidCommand,
    MissingList,
    MissingLocation,
    MissingName,
    MissingValue,
    MissingSubCmd,
    NoId,
    NoName,
    StdinErr,
    TooManyArgs,
    UnknownSpec,
    UnmatchedQuote
}

pub fn print(err: CmdErr, token: &str, line: usize) {
    let mut out = io::stdout();
    let mut outstring = String::new();
    let outstring = {
        match err {
            CmdErr::InvalidCommand => format!(
                    "\x1b[{};1H\x1b[2KERR: invalid command '{}'",
                    line, token
                ),

            CmdErr::MissingList => format!(
                    "\x1b[{};1H\x1b[2KERR: missing list for '{}'",
                    line, token
                ),

            CmdErr::MissingLocation => format!(
                    "\x1b[{};1H\x1b[2KERR: missing location for '{}'",
                    line, token
                ),

            CmdErr::MissingName => format!(
                    "\x1b[{};1H\x1b[2KERR: missing name for '{}'",
                    line, token
                ),

            CmdErr::MissingValue => format!(
                    "\x1b[{};1H\x1b[2KERR: missing value for '{}'",
                    line, token
                ),

            CmdErr::MissingSubCmd => format!(
                    "\x1b[{};1H\x1b[2KERR: missing sub-command for '{}'",
                    line, token
                ),

            CmdErr::NoId => format!(
                    "\x1b[{};1H\x1b[2KERR: no column id '{}'",
                    line, token
                ),

            CmdErr::NoName => format!(
                    "\x1b[{};1H\x1b[2KERR: no column called '{}'",
                    line, token
                ),

            CmdErr::StdinErr => format!(
                    "x1b[{};1H\x1b[2KERR: stdin error while processing '{}'",
                    line, token
                ),

            CmdErr::TooManyArgs => format!(
                    "x1b[{};1H\x1b[2KERR: too many argumentss for '{}'",
                    line, token
                ),


            CmdErr::UnknownSpec => format!(
                    "\x1b[{};1H\x1b[2KERR: unknown specifier '{}'",
                    line, token
                ),

            CmdErr::UnmatchedQuote => format!(
                    "\x1b[{};1H\x1b[2KERR: unmatched quote in {}",
                    line, token
                ),
        }
    };

    write!(out, "{outstring}");
    out.flush().unwrap();
}
