## csview - command-line TUI for friendly (or at least friendlier) CSV rendering
This program takes a single CSV file argument and renders it as a series of cells across the entire width and height of the terminal. Horizontal and vertical offsets are controlled via the arrow keys. Every time the window is resized, correspondingly more or less cells are shown.

To install:  

    cargo install --path .

To run:  

    csview some_csv_file.csv
