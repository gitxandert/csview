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
    - closes the program
- **ctrl + W**
    - toggle write mode, allowing editing of the focused cell
- **ctrl + S**
    - saves file as is, with a pre-save backup
    - (on exit, the program will prompt the user to save any unsaved edits)
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
- **number keys**
    - switch between different files

csview also has a command mode, accessed through ':' :
- **cn**: column name functions
    - by itself: displays the current column's name
    - `cn f {name}` | `cn find {name}`
        - moves the focus to the column by that name
    - `cn to {name}`
        - changes the focused column's name to a new name
- **col**: column functions
    - `col mv {loc}` | `col move {loc}`
        - moves the focused column to a new location
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
        - scroll up and down, return to normal functionality with `ctrl + q`, `ctrl + c`, or `ctrl + forward-slash`
    - `col s {arg}` | `col sort {arg}`
        - sort the focused column alphanumerically-ascending
        - an additional argument controls the sort:
            - `a`: default sort
            - `r` | `ar`: alphanumerically-descending
            - `n`: numerically-ascending
            - `nr`: numerically-descending
        - when sorting numerically, non-numerical values are pushed to the bottom of the column
    - `col fn {value}` | `col fillna {value}`
        - fill empty cells with the provided value (use quotes for sentences)
    - `col r {target} {new}` | `col replace {target} {new}`
        - replaces the target value with the new one
- **row**: row functions
    - `row i` | `row insert`
        - inserts a row below the focused row
    - `row d` | `row delete`
        - deletes the focused row after confirmation
    - `row mv {range} {index}` | `row move {range} {index}`
        - moves a given range of rows to the target index
        - ranges can be expressed as:
            - either a single quoted row index or unquoted integer (e.g. '000F' or 15)
            - a hyphenated range of quoted indices or unquoted integers (e.g. '000F'-'00B3' or 15-35)
            - a quoted index or unquoted integer, a '+', and the number of rows to include in the move (e.g. '000F'+20 or 15+20)
- **sh/sheet**: sheet functions
    - `sh sb {column} {direction}` | `sheet sortby {column} {direction}`
        - reindexes all columns according to the sorted column's indices
        - direction arguments are the same as for `col sort`
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
    - `sh sp {orientation} {sheet id} {loc}` | `sheet splice {sheet id} {orientation} {loc}`
        - adds one sheet's content to the focused sheet by orientation and at the location specified
        - sheet id is a number key associated with a sheet apart from the focused sheet's
        - orientation can be:
            - 'c' | 'cols'
            - 'r' | 'rows'
        - loc is the column name/id or row index/id of the focused sheet to splice at
- **ctrl + Q**
    - quits command mode
