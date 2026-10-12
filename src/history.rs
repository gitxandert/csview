use std::mem;

use crate::{
    cells::{Cell, Context, Csvs},
    terminal::WinInfo,
};

// oldest edits are dropped once the history grows past this
const MAX_HISTORY: usize = 50;

// every edit stores the state it replaced;
// undoing or redoing an edit swaps that state
// with the current one, so the same edit
// can travel back and forth between the stacks
enum Edit {
    // a single cell's content (d, p, write mode)
    Cell {
        ctx: usize,
        col: usize,
        real_row: usize, // index into Column.cells
        row: usize,      // displayed row, for refocusing
        cell: Cell,
    },
    // a whole sheet (most commands)
    Sheet {
        ctx: usize,
        con: Context,
    },
    // every sheet (slicing and splicing add/remove sheets)
    Workspace {
        contexts: Vec<Context>,
        handle: usize,
    },
}

pub struct History {
    undo: Vec<Edit>,
    redo: Vec<Edit>,
}

impl History {
    pub fn new() -> Self {
        Self {
            undo: Vec::<Edit>::new(),
            redo: Vec::<Edit>::new(),
        }
    }

    fn push(&mut self, edit: Edit) {
        self.undo.push(edit);
        if self.undo.len() > MAX_HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }
}

// state saved before running a command
pub enum Checkpoint {
    Non,
    Sheet {
        ctx: usize,
        con: Context,
        was_written: bool,
    },
    Workspace {
        contexts: Vec<Context>,
        handle: usize,
    },
}

impl Checkpoint {
    pub fn take(csvs: &mut Csvs, w_info: &WinInfo, tokens: &[&str]) -> Self {
        let changes_sheets = matches!(tokens.get(0), Some(&"sh") | Some(&"sheet"))
            && matches!(
                tokens.get(1),
                Some(&"sl") | Some(&"slice") | Some(&"sp") | Some(&"splice")
            );
        let can_edit = matches!(
            tokens.get(0),
            Some(&"cn") | Some(&"col") | Some(&"row") | Some(&"sh") | Some(&"sheet")
        );
        if !can_edit {
            return Checkpoint::Non;
        }

        csvs.save_context(w_info);
        if changes_sheets {
            return Checkpoint::Workspace {
                contexts: csvs.contexts.iter().map(|con| con.clone()).collect(),
                handle: csvs.handle,
            };
        }

        let ctx = csvs.handle;
        let con = csvs.get_context().clone();
        // every edit sets written, so clear it
        // to find out whether this command edits
        let cells = csvs.get_cells();
        let was_written = cells.written;
        cells.written = false;

        Checkpoint::Sheet { ctx, con, was_written }
    }

    pub fn commit(self, csvs: &mut Csvs) {
        match self {
            Checkpoint::Non => (),
            Checkpoint::Sheet { ctx, con, was_written } => {
                let cells = &mut csvs.contexts[ctx].cells;
                if cells.written {
                    csvs.history.push(Edit::Sheet { ctx, con });
                }
                csvs.contexts[ctx].cells.written |= was_written;
            }
            Checkpoint::Workspace { contexts, handle } => {
                // slicing and splicing only succeed
                // if they add or remove a sheet
                if csvs.num_contexts() != contexts.len() {
                    csvs.history.push(Edit::Workspace { contexts, handle });
                }
            }
        }
    }
}

// call after editing the focused cell, with a copy of it from before the edit
pub fn record_cell(csvs: &mut Csvs, old: Cell) {
    let ctx = csvs.handle;
    let cells = csvs.get_cells();
    if cells.w_cell().raw_content() == old.raw_content() {
        return;
    }

    let (col, row) = cells.w_cell;
    let real_row = cells.columns[col].indices[row];
    csvs.history.push(Edit::Cell { ctx, col, real_row, row, cell: old });
}

pub fn undo(csvs: &mut Csvs, w_info: &mut WinInfo) -> bool {
    match csvs.history.undo.pop() {
        Some(mut edit) => {
            apply(&mut edit, csvs, w_info);
            csvs.history.redo.push(edit);
            true
        }
        None => false,
    }
}

pub fn redo(csvs: &mut Csvs, w_info: &mut WinInfo) -> bool {
    match csvs.history.redo.pop() {
        Some(mut edit) => {
            apply(&mut edit, csvs, w_info);
            csvs.history.undo.push(edit);
            true
        }
        None => false,
    }
}

pub fn undo_len(csvs: &Csvs) -> usize {
    csvs.history.undo.len()
}

pub fn redo_len(csvs: &Csvs) -> usize {
    csvs.history.redo.len()
}

// swap the edit's state with the current state
// and point w_info at whatever changed
fn apply(edit: &mut Edit, csvs: &mut Csvs, w_info: &mut WinInfo) {
    csvs.save_context(w_info);
    match edit {
        Edit::Cell { ctx, col, real_row, row, cell } => {
            csvs.set_handle(*ctx);
            w_info.set_context(csvs.get_context());

            let cells = csvs.get_cells();
            let current = &mut cells.columns[*col].cells[*real_row];
            mem::swap(&mut current.content, &mut cell.content);
            mem::swap(&mut current.escape_sequences, &mut cell.escape_sequences);
            current.text_offset = 0;
            cells.written = true;

            w_info.set_w_pointer(*col);
            w_info.set_h_pointer(*row);
        }
        Edit::Sheet { ctx, con } => {
            csvs.set_handle(*ctx);
            csvs.get_context().swap_state(con);
            csvs.get_cells().written = true;
            w_info.set_context(csvs.get_context());
        }
        Edit::Workspace { contexts, handle } => {
            mem::swap(&mut csvs.contexts, contexts);
            mem::swap(&mut csvs.handle, handle);
            csvs.get_cells().written = true;
            w_info.set_context(csvs.get_context());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cells::{Cells, Column},
        csv_io::make_col_ids,
    };

    fn csvs_with_rows(names: Vec<&str>, rows: Vec<Vec<&str>>) -> Csvs {
        let num_cols = names.len();
        let header = names.into_iter().map(Cell::new).collect::<Vec<Cell>>();
        let mut cells = Cells::new(
            "test.csv".to_string(), ',', header, make_col_ids(num_cols), num_cols
        );
        for _ in 0..num_cols {
            cells.push_column(Column::new());
        }
        for row in rows {
            for (i, content) in row.into_iter().enumerate() {
                cells.push_to_col(i, Cell::new(content));
            }
        }
        cells.set_w_cell(0, 0);

        Csvs::new(vec![Context::new(0, cells)])
    }

    fn values(csvs: &mut Csvs, col: usize) -> Vec<String> {
        let column = &csvs.get_cells().columns[col];
        (0..column.len()).map(|r| column.view_cell(r).to_string()).collect()
    }

    fn win_for(csvs: &mut Csvs) -> WinInfo {
        let mut win = WinInfo::new();
        win.width = 80;
        win.height = 10;
        win.set_context(csvs.get_context());
        win
    }

    #[test]
    fn cell_edit_undoes_and_redoes() {
        let mut csvs = csvs_with_rows(vec!["a"], vec![vec!["one"], vec!["two"]]);
        let mut win = win_for(&mut csvs);

        let old = csvs.get_cells().w_cell().clone();
        csvs.get_cells().w_cell().content = "changed".to_string();
        record_cell(&mut csvs, old);

        assert!(undo(&mut csvs, &mut win));
        assert_eq!(values(&mut csvs, 0), vec!["one", "two"]);
        assert!(!undo(&mut csvs, &mut win));

        assert!(redo(&mut csvs, &mut win));
        assert_eq!(values(&mut csvs, 0), vec!["changed", "two"]);
        assert!(!redo(&mut csvs, &mut win));
    }

    #[test]
    fn unchanged_cell_is_not_recorded() {
        let mut csvs = csvs_with_rows(vec!["a"], vec![vec!["one"]]);
        let old = csvs.get_cells().w_cell().clone();
        record_cell(&mut csvs, old);

        assert_eq!(undo_len(&csvs), 0);
    }

    #[test]
    fn sheet_command_is_recorded_only_when_it_edits() {
        let mut csvs = csvs_with_rows(vec!["a"], vec![vec!["one"], vec!["two"]]);
        let win = win_for(&mut csvs);

        // read-only command
        let cp = Checkpoint::take(&mut csvs, &win, &["row", "c"]);
        cp.commit(&mut csvs);
        assert_eq!(undo_len(&csvs), 0);
        assert!(!csvs.get_cells().written);

        let cp = Checkpoint::take(&mut csvs, &win, &["row", "d"]);
        csvs.get_cells().columns[0].remove_cell(1);
        csvs.get_cells().written = true;
        cp.commit(&mut csvs);
        assert_eq!(undo_len(&csvs), 1);
        assert_eq!(values(&mut csvs, 0), vec!["one"]);
    }

    #[test]
    fn sheet_command_keeps_earlier_written_flag() {
        let mut csvs = csvs_with_rows(vec!["a"], vec![vec!["one"]]);
        let win = win_for(&mut csvs);
        csvs.get_cells().written = true;

        let cp = Checkpoint::take(&mut csvs, &win, &["row", "c"]);
        cp.commit(&mut csvs);
        assert!(csvs.get_cells().written);
    }

    #[test]
    fn sheet_edit_round_trips() {
        let mut csvs = csvs_with_rows(vec!["a"], vec![vec!["one"], vec!["two"]]);
        let mut win = win_for(&mut csvs);

        let cp = Checkpoint::take(&mut csvs, &win, &["row", "d"]);
        csvs.get_cells().columns[0].remove_cell(1);
        csvs.get_cells().written = true;
        cp.commit(&mut csvs);

        undo(&mut csvs, &mut win);
        assert_eq!(values(&mut csvs, 0), vec!["one", "two"]);
        redo(&mut csvs, &mut win);
        assert_eq!(values(&mut csvs, 0), vec!["one"]);
    }

    #[test]
    fn workspace_edit_restores_removed_sheet() {
        let mut csvs = csvs_with_rows(vec!["a"], vec![vec!["one"]]);
        let mut win = win_for(&mut csvs);
        let other = csvs.get_context().clone();
        csvs.push_context(other);
        csvs.contexts[1].id = 1;

        let cp = Checkpoint::take(&mut csvs, &win, &["sh", "sp", "c", "1", "A"]);
        csvs.remove_context(1);
        cp.commit(&mut csvs);
        assert_eq!(csvs.num_contexts(), 1);

        undo(&mut csvs, &mut win);
        assert_eq!(csvs.num_contexts(), 2);
        redo(&mut csvs, &mut win);
        assert_eq!(csvs.num_contexts(), 1);
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut csvs = csvs_with_rows(vec!["a"], vec![vec!["one"]]);
        let mut win = win_for(&mut csvs);

        let old = csvs.get_cells().w_cell().clone();
        csvs.get_cells().w_cell().content = "x".to_string();
        record_cell(&mut csvs, old);
        undo(&mut csvs, &mut win);
        assert_eq!(redo_len(&csvs), 1);

        let old = csvs.get_cells().w_cell().clone();
        csvs.get_cells().w_cell().content = "y".to_string();
        record_cell(&mut csvs, old);
        assert_eq!(redo_len(&csvs), 0);
    }
}
