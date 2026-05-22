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
- **d**
    - yank and remove content of focused cell
- **p**
    - paste yanked content to focused cell
- **ctrl + p**
    - (write mode) paste yanked content at the cursor
- **o**
    - display full content of focused cell to the screen
- **number keys**
    - switch between different files

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
    - `col d {other}` | `col diff {other}`
        - shows differences between all unique values in the focused column and all unique values in the other column
    - `col si {other}` | `col sim {other}`
        - shows similarities between all unique values in the focused column and all unique values in the other column
- **row**: row functions
    - `col pre {string} {nb}` | `col prepend {string} {nonblank}`
        - prepends the string to cells in the focused column
        - optional flag `nb`/`nonblank` prepends the string only to cells with content
    - `col app {string} {nb}` | `col append {string} {nonblank}`
        - appends the string to cells in the focused column
        - optional flag `nb`/`nonblank` appends the string only to cells with content
    - `col dup` | `col duplicate`
        - duplicates the focused column
    - `row c` | `row count`
        - prints how many rows there are
    - `row i {count}` | `row insert {count}`
        - inserts count rows below the focused row (default 1 row)
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
            - switch between sheets with the number keys
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
        - sheet id is a number key associated with a sheet apart from the focused sheet's
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
- **ctrl + q**
    - quits command mode
