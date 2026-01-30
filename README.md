# csview - command-line spreadsheet TUI
This program takes a single file argument and renders it as a series of cells across the entire width and height of the terminal. Horizontal and vertical offsets are controlled via the arrow, ctrl, and shift keys. Every time the window is resized, correspondingly more or less cells are shown.

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

csview currently admits the following key bindings:
- **ctrl + c | ctrl + forward-slash**
    - closes the program
- **ctrl + w**
    - toggle write mode, allowing writes to a cell (writes are saved to file on program exit)
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

csview also has a command mode, accessed through ':':    
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
- **ctrl + Q**
    - quits command mode
