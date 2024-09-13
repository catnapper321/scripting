#![allow(unused)]
use std::mem::MaybeUninit;
use std::{
    os::fd::{AsFd, AsRawFd, RawFd},
    io::{self, Read, Write, stdout, stdin},
    mem,
    ffi::CStr,
};
use libc::{
    c_int,
    termios,
};
mod password;
pub use password::*;

// input flags (iflag)
pub use libc::{
    IXON,
    ICRNL,
    IUTF8,
    BRKINT,
    INPCK,
    ISTRIP,
};
// output flags (oflag)
pub use libc::{
    OPOST,
};
// misc flags (lflag)
pub use libc::{
    ECHO,
    ECHONL, 
    ICANON,
    IEXTEN,
    ISIG
};

/// specify behavior of tcsetattr
#[derive(Debug, Clone, Copy)]
pub enum SetAction {
    /// flush output buffer to terminal, discard unprocessed input buffer
    TCSAFLUSH,
    /// apply changes immediately, disregarding buffer states
    TCSANOW,
    /// flush output buffer to terminal, keep input buffer
    TCSADRAIN,
}
impl SetAction {
    pub fn as_flag(&self) -> c_int {
        match self {
            Self::TCSAFLUSH => libc::TCSAFLUSH,
            Self::TCSANOW => libc::TCSANOW,
            Self::TCSADRAIN => libc::TCSADRAIN,
        }
    }
}

#[derive(Debug)]
pub struct Term {
    fd: RawFd, 
    orig_t: termios,
    t: termios,
}
impl Term {
    pub fn new(fd: impl AsRawFd) -> io::Result<Self> {
        let fd = fd.as_raw_fd();
        let orig_t = get_termios(fd)?;
        Ok(Self {
            fd,
            orig_t,
            t: orig_t.clone(),
        })
    }
    /// raw mode: unset ECHO and ICANON, disable output flow control, disable
    /// ctrl-v, disable carriage return translation (ctrl-m), disable ctrl-c
    /// signalling, and some other stuff. Note that this function disables
    /// output processing (auto carriage return insert).
    pub fn raw_mode(&mut self) -> &mut Self {
        self.t.c_iflag &= ! (IXON | ICRNL | BRKINT | INPCK | ISTRIP);
        self.t.c_lflag &= ! (ECHO | ICANON | IEXTEN | ISIG);
        self.t.c_oflag &= ! (OPOST);
        self.t.c_cflag &= ! libc::CS8;
        self
    }
    /// cooked mode: set ECHO, ECHONL, ICANON
    /// note that this does the bare minimum to get utf input awareness and
    /// character display; it does not restore flow control, unset the input
    /// timeout, etc.
    pub fn cooked_mode(&mut self) -> &mut Self {
        self.t.c_lflag |= (ECHO | ECHONL | ICANON);
        self
    }
    /// Turns off all output processing, like translating newline into
    /// carriage return + newline
    pub fn disable_output_processing(&mut self) -> &mut Self {
        self.t.c_oflag &= ! OPOST;
        self
    }
    /// Enables output processing, so that newlines automatically have carriage
    /// returns inserted before them
    pub fn enable_output_processing(&mut self) -> &mut Self {
        self.t.c_oflag |= OPOST;
        self
    }
    /// password mode: unset ECHO, set ECHONL
    pub fn password_mode(&mut self) -> &mut Self {
        self.t.c_lflag &= ! ECHO;
        self.t.c_lflag |= (ECHONL | ICANON);
        self
    }
    /// disable output flow control (ctrl-s and ctrl-q)
    pub fn disable_flow_control(&mut self) -> &mut Self {
        self.t.c_iflag &= ! IXON;
        self
    }
    /// enable output flow control (ctrl-s and ctrl-q)
    pub fn enable_flow_control(&mut self) -> &mut Self {
        self.t.c_iflag |= IXON;
        self
    }
    /// Set input timeout, granularity is tenths of a second. Values over
    /// 25.5s are set to 25.5s, values under 0.1s are set to 0.1s
    pub fn input_timeout(&mut self, vtime: std::time::Duration) -> &mut Self {
        self.t.c_cc[libc::VMIN] = 0; // return immediately after one byte read
        let mut tenths = vtime.as_millis() / 100;
        let tenths = match tenths {
            0 => 1,
            1..=255 => tenths as u8,
            _ => 255,
        };
        self.t.c_cc[libc::VTIME] = tenths;
        self
    }
    pub fn disable_input_timeout(&mut self) -> &mut Self {
        self.t.c_cc[libc::VMIN] = 1;
        self.t.c_cc[libc::VTIME] = 0;
        self
    }
    /// Convenience function that sets the terminal to password
    /// mode, prompts for a password, and resets the terminal. Uses
    /// [`default_termios()`] internally, and so requires [`init()`] to be
    /// called first.
    pub fn prompt_for_password(&mut self, prompt: impl std::fmt::Display) -> io::Result<Password> {
        self.password_mode().set(SetAction::TCSAFLUSH)?;
        let mut pw = Password::new();
        print!("{}: ", prompt);
        stdout().flush()?;
        pw.read_line(stdin())?;
        self.reset(SetAction::TCSAFLUSH)?;
        Ok(pw)
    }
    pub fn set(&self, action: SetAction) -> io::Result<()> {
        set_termios(self.fd.clone(), action, &self.t)?;
        Ok(())
    }
    pub fn reset(&mut self, action: SetAction) -> io::Result<()> {
        self.t = self.orig_t.clone();
        self.set(action)
    }
}

/// Safe wrapper around `libc::tcgetattr`. Returns a `libc::termios`.
pub fn get_termios(fd: impl AsRawFd) -> io::Result<termios> {
    let mut t = mem::MaybeUninit::<termios>::uninit();
    io_result(unsafe { libc::tcgetattr(fd.as_raw_fd(), t.as_mut_ptr()) })?;
    Ok(unsafe { t.assume_init() })
}

/// Safe wrapper around `libc::tcsetattr`.
pub fn set_termios(fd: impl AsRawFd, action: SetAction, t: &termios) -> io::Result<()> {
    io_result(unsafe { libc::tcsetattr(fd.as_raw_fd(), action.as_flag(), t) })
}

/// Returns true if the fd is a tty
pub fn isatty(fd: impl AsRawFd) -> bool {
    get_termios(fd).is_ok()
}

/// Converts a c return value (c_int) to an io Result
fn io_result(c_return: c_int) -> io::Result<()> {
    if c_return == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

