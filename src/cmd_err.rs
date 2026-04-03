use std::io::{self, Write};

pub enum CmdErr<'a> {
    InvalidArg(&'a str),
    InvalidCommand(&'a str),
    InvalidConId(&'a str),
    InvalidDec(&'a str),
    InvalidHex(char),
    InvalidIndex(usize),
    // InvalidRange(&'a str),
    InvalidSubCmd(&'a str),
    MissingConId(&'a str),
    MissingList(&'a str),
    MissingLocation(&'a str),
    MissingName(&'a str),
    MissingRange(&'a str),
    MissingValue(&'a str),
    MissingSubCmd(&'a str),
    MissingTarget(&'a str),
    NoId(&'a str),
    NoName(&'a str),
    NoNameContains(&'a str),
    SameCon,
    StdinErr(&'a str),
    TooManyArgs(&'a str),
    UnknownSpec(&'a str),
    // UnmatchedQuote(&'a str)
}

pub fn print(err: CmdErr, line: usize) {
    let mut out = io::stdout();
    let outstring = {
        match err {
            CmdErr::InvalidArg(t) => format!(
                    "\x1b[{};1H\x1b[2KERR: invalid argument '{}'",
                    line, t
                ),

            CmdErr::InvalidCommand(t) => format!(
                    "\x1b[{};1H\x1b[2KERR: invalid command '{}'",
                    line, t
                ),

            CmdErr::InvalidConId(t) => format!(
                    "\x1b[{};1H\x1b[2KERR: invalid context id '{}'",
                    line, t
                ),

            CmdErr::InvalidDec(t) => format!(
                    "\x1b[{};1H\x1b[2KERR: '{}' cannot be coerced to decimal",
                    line, t
                ),

            CmdErr::InvalidHex(t) => format!(
                    "\x1b[{};1H\x1b[2KERR: '{}' is not valid hexadecimal",
                    line, t
                ),

            CmdErr::InvalidIndex(t) => format!(
                    "\x1b[{};1H\x1b[2KERR: index '{}' is out of bounds",
                    line, t
                ),

      //      CmdErr::InvalidRange(t) => format!(
      //              "\x1b[{};1H\x1b[2KERR: '{}' is not a valid range",
      //              line, t
      //          ),

            CmdErr::InvalidSubCmd(t) => format!(
                    "\x1b[{};1H\x1b[2KERR: invalid sub-command '{}'",
                    line, t
                ),

            CmdErr::MissingConId(t) => format!(
                    "\x1b[{};1H\x1b[2KERR: missing context id for '{}'",
                    line, t
                ),

            CmdErr::MissingList(t) => format!(
                    "\x1b[{};1H\x1b[2KERR: missing list for '{}'",
                    line, t
                ),

            CmdErr::MissingLocation(t) => format!(
                    "\x1b[{};1H\x1b[2KERR: missing location for '{}'",
                    line, t
                ),

            CmdErr::MissingName(t) => format!(
                    "\x1b[{};1H\x1b[2KERR: missing name for '{}'",
                    line, t
                ),

            CmdErr::MissingRange(t) => format!(
                    "\x1b[{};1H\x1b[2KERR: missing range for '{}'",
                    line, t
                ),

            CmdErr::MissingTarget(t) => format!(
                    "\x1b[{};1H\x1b[2KERR: missing target index for '{}'",
                    line, t
                ),

            CmdErr::MissingValue(t) => format!(
                    "\x1b[{};1H\x1b[2KERR: missing value for '{}'",
                    line, t
                ),

            CmdErr::MissingSubCmd(t) => format!(
                    "\x1b[{};1H\x1b[2KERR: missing sub-command for '{}'",
                    line, t
                ),

            CmdErr::NoId(t) => format!(
                    "\x1b[{};1H\x1b[2KERR: no column id '{}'",
                    line, t
                ),

            CmdErr::NoName(t) => format!(
                    "\x1b[{};1H\x1b[2KERR: no column called '{}'",
                    line, t
                ),

            CmdErr::NoNameContains(t) => format!(
                    "\x1b[{};1H\x1b[2KERR: no column name contains '{}'",
                    line, t
                ),

            CmdErr::SameCon => format!(
                    "\x1b[{};1H\x1b[2KERR: can't splice the active context to itself",
                    line
                ),

            CmdErr::StdinErr(t) => format!(
                    "x1b[{};1H\x1b[2KERR: stdin error while processing '{}'",
                    line, t
                ),

            CmdErr::TooManyArgs(t) => format!(
                    "x1b[{};1H\x1b[2KERR: too many argumentss for '{}'",
                    line, t
                ),

            CmdErr::UnknownSpec(t) => format!(
                    "\x1b[{};1H\x1b[2KERR: unknown specifier '{}'",
                    line, t
                ),

      //      CmdErr::UnmatchedQuote(t) => format!(
      //              "\x1b[{};1H\x1b[2KERR: unmatched quote in {}",
      //              line, t
      //          ),
        }
    };

    let _ = write!(out, "{outstring}");
    out.flush().unwrap();
}
