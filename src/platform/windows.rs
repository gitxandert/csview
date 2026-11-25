// Windows 10 1809+ console backend. With ENABLE_VIRTUAL_TERMINAL_INPUT the
// console encodes keys as the same VT sequences a unix terminal sends, and
// ENABLE_VIRTUAL_TERMINAL_PROCESSING makes it draw csview's ANSI output.
// There are no signals: resizes are found by checking the window size while
// waiting for input, and ctrl + c / ctrl + \ arrive as bytes 3 / 28 that the
// reader turns into the same flags the unix signal handlers set.

use std::{
    collections::VecDeque,
    io, mem, ptr, slice,
    sync::{atomic::Ordering, Mutex},
};
use windows_sys::Win32::{
    Foundation::{
        GlobalFree, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
        WAIT_TIMEOUT,
    },
    Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING},
    System::{
        Console::{
            GetConsoleMode, GetConsoleScreenBufferInfo, GetStdHandle, ReadConsoleInputW,
            SetConsoleMode, SetStdHandle, CONSOLE_SCREEN_BUFFER_INFO, DISABLE_NEWLINE_AUTO_RETURN,
            ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
            ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_INPUT,
            ENABLE_VIRTUAL_TERMINAL_PROCESSING, ENABLE_WINDOW_INPUT, INPUT_RECORD, KEY_EVENT,
            STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        },
        DataExchange::{
            CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable,
            OpenClipboard, SetClipboardData,
        },
        Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
        Ole::CF_UNICODETEXT,
        Threading::WaitForSingleObject,
    },
};

use super::{PollEvent, RawMode, GOT_INT, GOT_QUIT, GOT_WINCH, IN_COMMAND};

#[derive(Clone, Copy)]
pub struct SavedMode {
    input: u32,
    output: u32,
}

// how often to check the window size while waiting for keys
const RESIZE_POLL_MS: u32 = 100;

struct InputState {
    // decoded UTF-8 not yet handed out (a read may give more than the caller's buffer)
    pending: VecDeque<u8>,
    // first half of a surrogate pair whose second half hasn't been read yet
    high_surrogate: Option<u16>,
    last_size: Option<(usize, usize)>,
}

static INPUT: Mutex<InputState> = Mutex::new(InputState {
    pending: VecDeque::new(),
    high_surrogate: None,
    last_size: None,
});

fn stdin_handle() -> HANDLE {
    unsafe { GetStdHandle(STD_INPUT_HANDLE) }
}

// point stdin at the console, so keys can be read after piped input has
// been used up
pub fn reattach_stdin() -> Result<(), io::Error> {
    let name: Vec<u16> = "CONIN$\0".encode_utf16().collect();
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || unsafe { SetStdHandle(STD_INPUT_HANDLE, handle) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn stdout_handle() -> HANDLE {
    unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }
}

// (width, height) of the visible console window, if stdout is a console
pub fn term_size() -> Option<(usize, usize)> {
    unsafe {
        let mut info: CONSOLE_SCREEN_BUFFER_INFO = mem::zeroed();
        if GetConsoleScreenBufferInfo(stdout_handle(), &mut info) == 0 {
            return None;
        }
        let w = info.srWindow.Right - info.srWindow.Left + 1;
        let h = info.srWindow.Bottom - info.srWindow.Top + 1;
        Some((w.max(0) as usize, h.max(0) as usize))
    }
}

// switch the console to raw VT mode; returns the previous modes, or None if
// stdin/stdout aren't a console. Both modes are the same here: interrupts are
// told apart from keys in poll_stdin instead.
pub fn enter_raw(_mode: RawMode) -> Option<SavedMode> {
    unsafe {
        let mut saved = SavedMode { input: 0, output: 0 };
        if GetConsoleMode(stdin_handle(), &mut saved.input) == 0
        || GetConsoleMode(stdout_handle(), &mut saved.output) == 0 {
            return None;
        }
        let raw_in = (saved.input & !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT))
            | ENABLE_VIRTUAL_TERMINAL_INPUT
            | ENABLE_WINDOW_INPUT;
        // DISABLE_NEWLINE_AUTO_RETURN makes '\n' a bare line feed, as in a raw unix tty
        let raw_out = saved.output
            | ENABLE_PROCESSED_OUTPUT
            | ENABLE_VIRTUAL_TERMINAL_PROCESSING
            | DISABLE_NEWLINE_AUTO_RETURN;
        SetConsoleMode(stdin_handle(), raw_in);
        SetConsoleMode(stdout_handle(), raw_out);
        Some(saved)
    }
}

pub fn restore(saved: &SavedMode) {
    unsafe {
        SetConsoleMode(stdin_handle(), saved.input);
        SetConsoleMode(stdout_handle(), saved.output);
    }
}

fn take_pending(state: &mut InputState, buf: &mut [u8]) -> usize {
    let n = state.pending.len().min(buf.len());
    for (dst, src) in buf.iter_mut().zip(state.pending.drain(..n)) {
        *dst = src;
    }
    n
}

// true (once) when the window size changed since the last check
fn resized(state: &mut InputState) -> bool {
    let now = term_size();
    if now == state.last_size {
        return false;
    }
    let first_check = state.last_size.is_none();
    state.last_size = now;
    !first_check
}

// decode one UTF-16 unit into pending bytes; with `interrupts`, ctrl + c and
// ctrl + \ set the interrupt flags instead (as SIGINT/SIGQUIT would) and the
// return value says one was seen
fn push_unit(state: &mut InputState, unit: u16, interrupts: bool) -> bool {
    if (0xD800..0xDC00).contains(&unit) {
        state.high_surrogate = Some(unit);
        return false;
    }
    let units: Vec<u16> = state.high_surrogate.take().into_iter().chain([unit]).collect();
    let mut interrupted = false;
    for c in char::decode_utf16(units).map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER)) {
        if interrupts && (c == '\x03' || c == '\x1c') {
            if !IN_COMMAND.load(Ordering::Relaxed) {
                match c {
                    '\x03' => GOT_INT.store(true, Ordering::SeqCst),
                    _ => GOT_QUIT.store(true, Ordering::SeqCst),
                }
            }
            interrupted = true;
            continue;
        }
        let mut utf8 = [0u8; 4];
        state.pending.extend(c.encode_utf8(&mut utf8).as_bytes());
    }
    interrupted
}

// read the console's queued input records (blocks if there are none)
fn read_records(state: &mut InputState, interrupts: bool) -> Result<bool, io::Error> {
    let mut records: [INPUT_RECORD; 64] = unsafe { mem::zeroed() };
    let mut n = 0u32;
    if unsafe { ReadConsoleInputW(stdin_handle(), records.as_mut_ptr(), records.len() as u32, &mut n) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut interrupted = false;
    for record in &records[..n as usize] {
        // resize, focus and mouse records are skipped; the size is checked separately
        if record.EventType as u32 != KEY_EVENT as u32 {
            continue;
        }
        let key = unsafe { record.Event.KeyEvent };
        let unit = unsafe { key.uChar.UnicodeChar };
        if key.bKeyDown == 0 || unit == 0 {
            continue;
        }
        for _ in 0..key.wRepeatCount.max(1) {
            interrupted |= push_unit(state, unit, interrupts);
        }
    }
    Ok(interrupted)
}

// block until there are keys to hand out, the window resizes or an interrupt is typed
pub fn poll_stdin(buf: &mut [u8]) -> Result<PollEvent, io::Error> {
    loop {
        let mut state = INPUT.lock().unwrap_or_else(|e| e.into_inner());
        if !state.pending.is_empty() {
            return Ok(PollEvent::Data(take_pending(&mut state, buf)));
        }
        if resized(&mut state) {
            GOT_WINCH.store(true, Ordering::SeqCst);
            return Ok(PollEvent::Sig);
        }
        drop(state);

        match unsafe { WaitForSingleObject(stdin_handle(), RESIZE_POLL_MS) } {
            WAIT_OBJECT_0 => {
                let mut state = INPUT.lock().unwrap_or_else(|e| e.into_inner());
                if read_records(&mut state, true)? {
                    return Ok(PollEvent::Sig);
                }
            }
            WAIT_TIMEOUT => continue,
            _ => return Err(io::Error::last_os_error()),
        }
    }
}

// plain blocking read, for line editing
pub fn read_stdin(buf: &mut [u8]) -> Result<usize, io::Error> {
    let mut state = INPUT.lock().unwrap_or_else(|e| e.into_inner());
    while state.pending.is_empty() {
        read_records(&mut state, false)?;
    }
    Ok(take_pending(&mut state, buf))
}

// nothing to install: interrupts and resizes are picked up by poll_stdin
pub fn install_sig_handlers() {}

pub fn copy_to_clipboard(content: &str) -> bool {
    let wide: Vec<u16> = content.encode_utf16().chain([0]).collect();
    unsafe {
        if OpenClipboard(ptr::null_mut()) == 0 {
            return false;
        }
        let copied = (|| {
            if EmptyClipboard() == 0 {
                return false;
            }
            let mem = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
            if mem.is_null() {
                return false;
            }
            let dst = GlobalLock(mem) as *mut u16;
            if dst.is_null() {
                GlobalFree(mem);
                return false;
            }
            ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
            GlobalUnlock(mem);
            // on success the clipboard owns the memory
            if SetClipboardData(CF_UNICODETEXT as u32, mem).is_null() {
                GlobalFree(mem);
                return false;
            }
            true
        })();
        CloseClipboard();
        copied
    }
}

pub fn read_from_clipboard() -> Option<String> {
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT as u32) == 0
        || OpenClipboard(ptr::null_mut()) == 0 {
            return None;
        }
        let text = (|| {
            let data = GetClipboardData(CF_UNICODETEXT as u32);
            if data.is_null() {
                return None;
            }
            let src = GlobalLock(data) as *const u16;
            if src.is_null() {
                return None;
            }
            let mut len = 0usize;
            while *src.add(len) != 0 {
                len += 1;
            }
            let text = String::from_utf16_lossy(slice::from_raw_parts(src, len));
            GlobalUnlock(data);
            // windows clipboard text uses CRLF; the unix tools give LF
            Some(text.replace("\r\n", "\n"))
        })();
        CloseClipboard();
        text
    }
}
