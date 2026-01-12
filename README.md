## csview - command-line TUI for friendly (or at least friendlier) CSV rendering
This program takes a single CSV file argument and renders it as a series of cells across the entire width and height of the terminal. Horizontal and vertical offsets are controlled via the arrow keys. Every time the window is resized, correspondingly more or less cells are shown.

To install, run the provided install script:  

    ./install_csview

Note: event devices typically can't be read by a normal user, so you'll need to either run csview as sudo, or get user permission to read the keyboard, with something like:
    
    sudo usermod -aG input "$USER"

Reboot after running the above.

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
