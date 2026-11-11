# csview - command-line spreadsheet TUI
This program takes multiple file arguments and renders them as series of cells across the entire width and height of the terminal. Horizontal and vertical offsets are controlled via the arrow, ctrl, and shift keys. Every time the window is resized, correspondingly more or less cells are shown.

It is also possible to write to cells (see **Use** below). To preserve data on write, a mini versioning system has been instigated, preserving the last ten iterations of a file to a .csview/backups directory in the user's home directory.

## Installation

This package is installed with Rust's cargo toolchain; if you're not already a Rustacean, install cargo first.

If you don't want to tinker with the build, you can just run the provided install script:  

    ./install_csview

## Use

To run csview:  

    csview some_csv_file.csv

Run it with no file to start on an empty sheet named Untitled.csv: one "Unnamed" column with enough rows to fill the screen. Nothing is written to disk unless you save it.

There is an argument -d/--delimiter that is default set to ',', but can take any other single char as an argument ('t' = tab): 
    
    csview -d : some_colon_separated_file

You can feed multiple files as arguments to csview, but they currently need to all have the same delimiter.

csview currently admits the following key bindings:
- **ctrl + C | ctrl + forward-slash**
    - exits the program
- **ctrl + W**
    - toggle write mode, allowing editing of the focused cell
- **ctrl + S**
    - saves file as is, with a pre-save backup
    - (on exit, the program will prompt the user to save any unsaved edits)
- **ctrl + Q**
    - exits command mode (see below)
- **ctrl + O**
    - switches focus to the other pane in split view
- **arrow keys**
    - highlight individual cells; if in write mode, these scroll through text
- **ctrl + arrow key**
    - offest column/row view by the window width/height
- **shift + arrow key**
    - offset column/row view by one
- **alt + arrow key (left or right)**
    - scroll through text within a cell when not in write mode
- **ctrl + shift + arrow key (left or right)**  
    - adjust column width
- **y**
    - yank (copy) content of focused cell
- **ctrl + y**
    - yank content of focused cell and copy it to the system clipboard (uses wl-copy, xclip, xsel, or pbcopy)
- **d**
    - yank and remove content of focused cell
- **p**
    - paste yanked content to focused cell
- **ctrl + p**
    - paste system clipboard content to the focused cell (uses wl-paste, xclip, xsel, or pbpaste)
    - (write mode) paste yanked content at the cursor
- **u**
    - undo the last edit (cell edits, deletes, pastes, and any command that changes a sheet)
    - keeps the last 50 edits across all open sheets
- **ctrl + r**
    - redo the last undone edit
- **o**
    - display full content of focused cell to the screen
- **number keys**
    - switch to sheets 0-9
- **Tab** | **Shift-Tab**
    - switch to the next or previous sheet, wrapping around (reaches sheets past 9)
- **`** (backtick)
    - switch back to the sheet that was focused before this one

csview also has a command mode, accessed through ':' :
- **q** | **quit**
    - exits the program
- **cn**: column name functions
    - by itself: displays the current column's name
    - `cn f {name}` | `cn find {name}`
        - moves the focus to the column by that name
    - `cn to {name}`
        - changes the focused column's name to a new name
- **col**: column functions
    - `col c` | `col count`
        - prints how many columns there are
    - `col mv {range} {loc}` | `col move {range} {loc}`
        - moves the range of colummns provided to a new location
        - range can be expressed as:
            - a single column name/id
            - two column names/ids separated by a hyphen
            - a column name/id, a '+', and the number of additional columns to include in the move
            - additionally:
                - _ indicates the focused column
                - an empty value for either part of a range indicate respectively the first or last columns
        - loc is either the target column name in quotes or the target column ID (i.e. A, B, C) without quotes
    - `col f {value}` | `col find {value}`
        - finds all occurrences of the provided value within the focused column
        - control occurrence search by:
            - 'n': next
            - 'b': back
            - any other key returns to normal scrolling
    - `col n '{name}'` | `col new '{name}'`
        - creates a new column with the provided column name at the focused location
    - `col rm '{name}'` | `col remove '{name}'`
        - removes the focused column after confirmation
    - `col g '{list}'` | `col group '{list}'`  
        - groups together a comma-separated list of columns (specified by quoted column names) 
    - `col u` | `col unique`
        - reformats the focused column to show all unique values
    - `col s {arg}` | `col sort {arg}`
        - sort the focused column alphanumerically-ascending
        - an additional argument controls the sort:
            - `a`: default sort
            - `r` | `ar`: alphanumerically-descending
            - `n`: numerically-ascending
            - `nr`: numerically-descending
        - when sorting numerically, non-numerical values are pushed to the bottom of the column
    - `col rv` | `col revert`
        - reverts column indices back to original order ("unsorts"/"un-uniques" a column)
    - `col fn {value}` | `col fillna {value}`
        - fill empty cells with the provided value (use quotes for sentences)
    - `col r {target} {new}` | `col replace {target} {new}`
        - replaces the target value with the new one
    - `col a` | `col add`
        - sums all numeric cells in a column
    - `col d {other} {-f}` | `col diff {other} {-f}`
        - shows differences between all unique values in the focused column and all unique values in the other column
        - optional flag `-f`/`--filter` filters the focused column to the rows whose values aren't in the other column
    - `col si {other} {-f}` | `col sim {other} {-f}`
        - shows similarities between all unique values in the focused column and all unique values in the other column
        - optional flag `-f`/`--filter` filters the focused column to the rows whose values are in the other column
    - `col pre {string}` | `col prepend {string}`
        - prepends the string to cells with content in the focused column (blank cells stay blank)
    - `col app {string}` | `col append {string}`
        - appends the string to cells with content in the focused column (blank cells stay blank)
    - `col dup` | `col duplicate`
        - duplicates the focused column
- **row**: row functions
    - `row c` | `row count`
        - prints how many rows there are
    - `row i {count}` | `row insert {count}`
        - inserts count rows below the focused row (default 1 row)
    - `row dup {count}` | `row duplicate {count}`
        - duplicates the focused row count times (default once)
    - `row d` | `row delete`
        - deletes the focused row after confirmation
    - `row mv {range} {index}` | `row move {range} {index}`
        - moves a given range of rows to the target index
        - ranges can be expressed as:
            - either a single quoted row index or unquoted integer (e.g. '000F' or 15)
            - a hyphenated range of quoted indices or unquoted integers (e.g. '000F'-'00B3' or 15-35)
            - a quoted index or unquoted integer, a '+', and the number of rows to include in the move (e.g. '000F'+20 or 15+20)
    - `row g {location}` | `row goto {location}`
        - goes to the target row, provided as a single quoted index or unquoted integer
    - `row n` | `row num`
        - shows the current row index (which is in hexidecimal) in decimal
    - `row a` | `row add`
        - sums all numerics cells in a row
- **sh/sheet**: sheet functions
    - `sh split {h|horizontal|v|vertical} [sheet]`
        - splits the terminal into two equal panes (`h` is top/bottom, `v` is left/right)
        - the optional sheet is resolved like `sh goto`; without one, uses the next sheet or creates an empty sheet
        - run the command again to change orientation or replace the second pane's sheet
    - `sh split off`
        - turns off split view and keeps the active pane's sheet full-screen
        - both sheets remain open and available through normal sheet switching
    - `sh s {column} {direction}` | `sheet sortby {column} {direction}`
        - reindexes all columns according to the sorted column's indices
        - direction arguments are the same as for `col sort`
    - `sh sf {column}` | `sheet siftby {column}`
        - sifts sheet according to unique values in the indicated column
    - `sh rv` | `sh revert`
        - reverts sheet to state prior to sorting/sifting
            - does not work on sliced-out sheets, but does work on sheets that have been sliced
    - `sh sl {orientation} {range}` | `sheet slice {orientation} {range}`
        - slices out the specified columns or rows into a separate sheet
            - switch between sheets with the number keys, Tab / Shift-Tab, or `sh g`
        - orientation can be:
            - 'c' | 'cols'
            - 'r' | 'rows'
        - range can be expressed as:
            - a single column name/id or row index/id
            - two column names/ids or row indices/ids separated by a '-'
            - a column name/id or row index/id, a '+', and the number of columns or row afterward to include
        - shortcuts for range indices:
            - no index: the first row/col if before -/+, the last row/col if after - (but not +)
            - underscore: the focused row/col
    - `sh sp {orientation} {sheet id} {loc}` | `sheet splice {sheet id} {orientation} {loc}`
        - adds one sheet's content to the focused sheet by orientation and at the location specified
        - sheet id is a number or a sheet name (see `sh g`) apart from the focused sheet's
        - orientation can be:
            - 'c' | 'cols'
            - 'r' | 'rows'
        - loc is the column name/id or row index/id of the focused sheet to splice at
    - `sh r {target} {new}` | `sheet replace {target} {new}`
        - replaces the target value with a new one across the entire sheet
    - `sh f {term} {arguments}` | `sheet find {term} {arguments}`
        - finds the provided term across the entire sheet
        - arguments are preceded by dashes and include:
            - -i/--case-insensitive: case-insensitive search
            - -f/--filter: filters the sheet to only the rows that contain the term
    - `sh rn {new_name}` | `sheet rename {new_name}`
        - renames the current sheet to the provided new_name
    - `sh g {target}` | `sheet goto {target}`
        - switches to a sheet by index or by name
        - a name matches a sheet's filename, its basename, or the basename without its extension (case-insensitive); failing an exact match, a unique prefix works
        - errors if nothing matches or the name is ambiguous; never touches undo history
    - `sh o {path} {arguments}` | `sheet open {path} {arguments}`
        - opens a file as a new sheet and switches to it
        - quote paths that contain spaces; a leading ~/ expands to the home directory
        - arguments are preceded by dashes and include:
            - -i/--in-place: replaces the focused sheet instead
                - if the focused sheet has unsaved edits, asks whether to save it first (y/n, anything else cancels)
            - -d/--delimiter {char}: the file's delimiter (t for tab); defaults to the focused sheet's
        - undo closes the opened sheet, or brings back the replaced one
    - `sh c` | `sheet close`
        - closes the focused sheet and switches to the next one
        - if the sheet has unsaved edits, asks whether to save it first (y/n, anything else cancels)
        - closing the only open sheet quits csview, as `q` does
        - undo brings the closed sheet back
- **up/down arrows**
    - scroll through previously run commands; scrolling past the newest restores what you were typing
    - the last 100 commands are kept across sessions in ~/.csview/cmd_history
- **ctrl + q**
    - quits command mode
