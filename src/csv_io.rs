use std::{
    env,
    ptr,
    fs::{self, File},
    time::{SystemTime, UNIX_EPOCH},
    io::{self, Error, ErrorKind, Read, Write, stdin, stdout},
    path::{Component, Path, PathBuf}
};
use libc::{poll, pollfd, POLLIN, STDIN_FILENO};

use crate::cells::{Cell, Cells, Column, EscSeq};

fn parse_by_newline(block: &str) -> Vec<String> {
    let mut parsed = Vec::<String>::new();
    
    let mut line = String::new();
    let mut is_quoted = false;
    let mut saw_newline = false;
    
    for c in block.chars() {
        match c {
            '"' => {
                is_quoted = !is_quoted;
                line.push(c);
            }
            '\n' => {
                // guard against consecutive newlines
                if !saw_newline {
                    saw_newline = true;
                    if !is_quoted {
                        parsed.push(line.clone());
                        line.clear();
                    }
                }
            }
            _ => {
                saw_newline = false;
                line.push(c);
            }
        }
    }

    if !saw_newline {
        parsed.push(line);
    }

    parsed
}

fn parse_by_delim(line: &str, delim: char) -> Vec<Cell> {
    let mut parsed = Vec::<Cell>::new();
    
    let mut cell_str = String::new();
    let mut is_quoted = false;
   
    let delim = match delim {
        't' => '\t',
        _ => delim
    };

    for c in line.chars() {
        match c {
            '"' => {
                is_quoted = !is_quoted;
            }
            ch if ch == delim && ch != '"' && !is_quoted => {
                let cell = Cell::new(cell_str.trim());
                parsed.push(cell);
                cell_str = String::new();
            }
            _ => cell_str.push(c),
        }
    }

    let cell = Cell::new(&cell_str);
    parsed.push(cell);
    parsed
}

const POWERS_OF_26: [u32; 7] = [
    0, 
    26, 
    702, 
    18278, 
    475254, 
    12356630, 
    321272406
];

fn get_range_branchless(x: u32) -> usize {
    (x >= POWERS_OF_26[1]) as usize +
    (x >= POWERS_OF_26[2]) as usize +
    (x >= POWERS_OF_26[3]) as usize +
    (x >= POWERS_OF_26[4]) as usize +
    (x >= POWERS_OF_26[5]) as usize +
    (x >= POWERS_OF_26[6]) as usize
}

pub fn int_to_base_26(mut x: u32) -> String {
    let range: usize = get_range_branchless(x);

    let mut bytes = vec![0u8; range + 1];

    for i in (0..=range).rev() {
        bytes[i] = ((x % 26) as u8) + 65;
        x = (x / 26).wrapping_sub(1);
    }

    String::from_utf8(bytes).unwrap()
}

pub fn make_col_ids(num_cols: usize) -> Vec<Cell> {
    let mut row = Vec::<Cell>::new();

    for i in 0..num_cols as u32 {
        let cell = Cell::new(&int_to_base_26(i));
        row.push(cell);
    }

    row
}

fn parse_csv_into_cells(filename: String, csv: String, delim: char) -> Result<Cells, io::Error> {
    // extract lines, but parse into columns
    let mut lines: Vec<String> = parse_by_newline(&csv);

    let mut header: Vec<Cell> = parse_by_delim(&lines.remove(0), delim.clone());
    let col_len = header.len();
    let col_ids: Vec<Cell> = make_col_ids(col_len);
    
    let mut cells = Cells::new(filename, delim.clone(), header, col_ids, col_len);

    if lines.len() == 0 {
        for j in 0..col_len {
            let mut column = Column::new();
            cells.push_column(column);
            cells.push_to_col(j, Cell::new(""));
        }
    }
    for i in 0..lines.len() {
        let row: Vec<Cell> = parse_by_delim(&lines[i], delim);
        for j in 0..col_len {
            if i == 0 {
                let mut column = Column::new();
                cells.push_column(column);
            }
            let cell = row.get(j).unwrap().clone();
            cells.push_to_col(j, cell);
        }
    }
    // set w_cell now, since there will be multiple contexts
    cells.set_w_cell(0, 0);

    Ok(cells)
}

pub fn load_csv(filename: String, delim: char) -> Result<Cells, io::Error> {
    let mut file = fs::read_to_string(filename.clone())?;
    // don't parse carriage returns
    file = file.replace("\r", " ");

    let cells = parse_csv_into_cells(filename, file, delim)?;

    Ok(cells)
}

pub fn save_backup(file: &String) -> Result<String, io::Error> {
    let content = fs::read_to_string(file)?;
    // find home dir or current dir; propagate error if neither
    let home_dir = match env::home_dir() {
        Some(path) => path,
        None => match env::current_dir() {
            Ok(path) => path,
            Err(e) => return Err(e),
        }
    };

    let file = Path::new(file);
    let abs = if file.is_absolute() {
        file.to_path_buf()
    } else {
        env::current_dir()?.join(file)
    };

    let abs = fs::canonicalize(&abs)?;

    let parent = abs.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound, "Could not find parent dir"
        )
    })?;

    let stem = abs.file_stem().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound, "Could not find file stem"
        )
    })?;

    let rel_parent: PathBuf = parent
                .components()
                .filter_map(|c| match c {
                    Component::Normal(p) => Some(p),
                    _ => None,
                })
                .collect();

    // either open or create backup directory
    let dir_path = Path::new(&home_dir)
        .join(".csview")
        .join("backups")
        .join(rel_parent)
        .join(stem);

    fs::create_dir_all(&dir_path)?;

    let backup_dir = fs::read_dir(&dir_path)?;
    let mut backup_files: Vec<_> = backup_dir
        .filter_map(|res| res.ok())
        .map(|e| e.path())
        .collect();

    // limit files in backup dir to 10
    let mut dir_len = backup_files.len();
    let mut stage_for_removal = Vec::<_>::new();
    for bf in &backup_files {
        if dir_len >= 10 {
            stage_for_removal.push(bf.clone());
            dir_len = dir_len.saturating_sub(1);
        } else {
            break;
        }
    }

    // set default to the name of the youngest file,
    // with timestamp incremented by 1
    let default = match backup_files.pop() {
        Some(backup) => {
            let stem = backup.file_stem().unwrap_or_default();
            let ext = backup.extension().unwrap_or_default();

            let stem_str = stem.to_string_lossy();
            let ext_str = ext.to_string_lossy();

            let mut parts: Vec<&str> = stem_str.split('_').collect();
            let prefix = parts[0];
            let timestamp_str = match parts.pop() {
                Some(ts) => ts,
                None => "0",
            };
            let timestamp: u64 = match timestamp_str.parse() {
                Ok(val) => val,
                Err(_) => 0u64,
            };
            let faux_new = timestamp + 1;

            let path = format!("{}_{}.{}", prefix, faux_new, ext_str);

            dir_path.join(path)
        }
        // if no previous file, just make it
        // {filename}_0(.ext)
        None => {
            let stem = file.file_stem().unwrap_or_default();
            let ext = file.extension().unwrap_or_default();

            let stem_str = stem.to_string_lossy();
            let ext_str = ext.to_string_lossy();

            let mut parts: Vec<&str> = stem_str.split('.').collect();
            let prefix = parts[0];
            let path = match parts.get(0) {
                Some(ext) => format!("{}_0.{}", prefix, ext),
                None => format!("{}_0", prefix),
            };

            dir_path.join(path)
        }
    };

    // try to get timestamp to affix to file stem
    let time = SystemTime::now().duration_since(UNIX_EPOCH);
    let backup = match time {
        Ok(t) => {
            let stem = file.file_stem().unwrap_or_default();
            let ext = file.extension().unwrap_or_default();

            let prefix = stem.to_string_lossy();
            let ext = ext.to_string_lossy();

            let timestamp = t.as_secs();

            let path = format!("{}_{}.{}", prefix, timestamp, ext);

            dir_path.join(path)
        }
        Err(_) => default,
    };

    // remove old backups only if write succeeds
    match fs::write(&backup, content) {
        Ok(()) => {
            for path in stage_for_removal {
                match fs::remove_file(path) {
                    Ok(()) => (),
                    Err(e) => return Err(e),
                }
            }
            Ok(format!("Wrote backup to {:?}", backup))
        }
        Err(e) => Err(e),
    }
}

// TODO: reintroduce escape sequences
pub fn write_to_file(cells: &mut Cells) -> Result<String, io::Error> {
    let mut sheet = String::new();
    let delim = cells.delim; 
    {
        let header = &cells.header;
        for i in 0..header.len() {
            let cell = header.get(i).unwrap();
            if cell.content.contains(delim) {
                sheet.push_str(&format!("\"{}\"", cell.content));
            } else {
                sheet.push_str(&cell.content);
            }

            if i != header.len() - 1 {
                sheet.push(delim);
            }
        }
        sheet.push('\n');
    }

    for i in 0..cells.num_rows() {
        let mut row = String::new();
        for j in 0..cells.num_cols() {
            let column = cells.get_column(j);
            let cell = column.get_cell(i);
            if cell.content.contains(delim) {
                row.push_str(&format!("\"{}\"", cell.content));
            } else {
                row.push_str(&cell.content);
            }
            if j != cells.num_cols() - 1 {
                row.push(delim);
            }
        }

        if i != cells.num_rows() - 1 {
            row.push('\n');
        }

        sheet.push_str(&row);
    }
    
    match fs::write(&cells.filename, sheet) {
        Ok(()) => {
            Ok(format!("Wrote {} to file", cells.filename))
        }
        Err(e) => Err(e),
    }
}

pub enum PollEvent {
    Data(usize),
    Sig,
}

// can use randomly for reading from stdin
pub fn poll_stdin(buf: &mut [u8]) -> Result<PollEvent, io::Error> {
    let mut fds = [pollfd {
        fd: STDIN_FILENO,
        events: POLLIN,
        revents: 0,
    }];

    let ret = unsafe {
        poll(
            fds.as_mut_ptr(), 
            fds.len() as libc::nfds_t, 
            -1
        )
    };

    if ret < 0 {
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::Interrupted { 
            return Ok(PollEvent::Sig); 
        }
        return Err(err);
    }

    if fds[0].revents & POLLIN != 0 {
        let n = stdin().read(buf)?;
        return Ok(PollEvent::Data(n));
    }

    return Ok(PollEvent::Data(0));
}
