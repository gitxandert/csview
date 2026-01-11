use std::{
    mem, ptr,
    fs::{self, File},
    io::{self, Error, Read},
};
use libc::input_event;

use crate::terminal::WinInfo;

// Linux keycodes
//
pub const EV_KEY: u16 = 0x01;

pub const KEY_DEPRESSED:    i32 = 1;
pub const KEY_RELEASED:     i32 = 0;
pub const KEY_REPEAT:       i32 = 2;

pub const KEY_SPACE: u16 = 57;
pub const KEY_ENTER: u16 = 28;

pub const KEY_UP:       u16 = 103;
pub const KEY_LEFT:     u16 = 105;
pub const KEY_RIGHT:    u16 = 106;
pub const KEY_DOWN:     u16 = 108; 

pub const KEY_LEFTCTRL:     u16 = 29;
pub const KEY_RIGHTCTRL:    u16 = 97;

pub const KEY_LEFTSHIFT:    u16 = 42;
pub const KEY_RIGHTSHFIT:   u16 = 54;

pub const KEY_LEFT_ALT:     u16 = 56;
pub const KEY_RIGHT_ALT:    u16 = 100;

// input funcitons
//
pub fn find_kbd() -> Result<File, io::Error> {
    // flesh out better later
    File::open("/dev/input/event3")
} 

pub fn process_input(w_info: &mut WinInfo, kbd: &mut File) {
    let ev = match read_char(kbd) {
        Some(event) => event,
        None => return,
    };
    if ev.type_ == EV_KEY {
        if ev.value == KEY_DEPRESSED {
            match ev.code {
                KEY_LEFTCTRL | KEY_RIGHTCTRL => w_info.page(),
                KEY_LEFTSHIFT | KEY_RIGHTSHFIT => w_info.shift(),
                KEY_LEFT => w_info.w_offset_left(),
                KEY_RIGHT => w_info.w_offset_right(),
                KEY_UP => w_info.h_offset_up(),
                KEY_DOWN => w_info.h_offset_down(),
                _ => (),
            }
        } else if ev.value == KEY_REPEAT {
            match ev.code {
                KEY_LEFT => w_info.w_offset_left(),
                KEY_RIGHT => w_info.w_offset_right(),
                KEY_UP => w_info.h_offset_up(),
                KEY_DOWN => w_info.h_offset_down(),
                _ => (),
            }
        } else if ev.value == KEY_RELEASED {
            match ev.code {
                KEY_LEFTCTRL | KEY_RIGHTCTRL => w_info.unpage(),
                KEY_LEFTSHIFT | KEY_RIGHTSHFIT => w_info.unshift(),
                _ => (),
            }
        }
    }
}

#[inline(always)]
fn read_char(kbd: &mut File) -> Option<input_event> {
    let mut buf = [0u8; mem::size_of::<input_event>()];
    match kbd.read_exact(&mut buf) {
        Err(_) => None,
        Ok(_) => {
            let ev = unsafe {
                ptr::read_unaligned(buf.as_ptr() as *const input_event)
            };
            Some(ev)
        }
    }
}
