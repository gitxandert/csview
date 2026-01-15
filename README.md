# csview - command-line spreadsheet TUI
This program takes a single file argument and renders it as a series of cells across the entire width and height of the terminal. Horizontal and vertical offsets are controlled via the arrow keys. Every time the window is resized, correspondingly more or less cells are shown.

## Installation

This package is installed with Rust's cargo toolchain; if you're not already a Rustacean, install cargo first.

If you don't want to tinker with the build, you can just run the provided install script:  

    ./install_csview

## Use

To run csview:  

    csview some_csv_file.csv

There is an argument -d/--delimiter that is default set to ',', but can take any other single char as an argument ('t' = tab): 
    
    csview -d : some_colon_separated_file

csview currently admits the following functionality:
- **ctrl + c**
    - closes the program
- **arrow keys**
    - highlight individual cells; if in write mode, these scroll through text
- **shift + arrow key**
    - offset column/row view by one
- **ctrl + arrow key**
    - offest column/row view by the window width/height
- **alt + arrow key (left or right)**
    - scroll through text within a cell
- **ctrl + w**
    - toggle write mode, allowing writes to a cell (writes are saved to file on program exit)
