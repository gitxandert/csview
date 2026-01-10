## csview - command-line TUI for friendly (or at least friendlier) CSV rendering
This program takes a single CSV file argument and renders it to the entire width and height of the terminal, truncating horizontally and vertically. Horizontal and vertical offsets are controlled via the arrow keys. Every time the window is resized, correspondingly more or less of the current CSV area is shown.

To run:  

    cargo run -- some_csv_file.csv
