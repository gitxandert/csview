# csview - command-line TUI for friendly/ier CSV rendering
This program takes a single CSV file argument and renders it as a series of cells across the entire width and height of the terminal. Horizontal and vertical offsets are controlled via the arrow keys. Every time the window is resized, correspondingly more or less cells are shown.

## Installation

This package is installed with Rust's cargo toolchain; if you're not already a Rustacean, install cargo first.

If you don't want to tinker with the build, you can just run the provided install script:  

    ./install_csview

## Use

To run csview:  

    csview some_csv_file.csv

csview currently admits the following functionality:
- **ctrl + c**
    - closes the program
- **arrow keys**
    - highlight individual cells
- **shift + arrow key**
    - shift columns or rows by one
- **ctrl + arrow key**
    - shift columns or rows by a "page"
- **alt + arrow key (left or right)**
    - scrolls through text within a cell
