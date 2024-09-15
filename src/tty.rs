#![allow(unused)]
use libc::{c_int, termios};
use std::mem::MaybeUninit;
use std::{
    ffi::CStr,
    io::{self, stdin, stdout, Read, Write},
    mem,
    os::fd::{AsFd, AsRawFd, RawFd},
};
// input flags (iflag)
use libc::{BRKINT, ICRNL, INPCK, ISTRIP, IUTF8, IXON};
// output flags (oflag)
use libc::OPOST;
// misc flags (lflag)
use libc::{ECHO, ECHONL, ICANON, IEXTEN, ISIG};
// exports
pub mod password;
use password::*;

/// Specifies behavior of libc::tcsetattr
#[derive(Debug, Clone, Copy)]
pub enum SetAction {
    /// Flushes the terminal output buffer, discarding unprocessed input
    /// buffer data
    TCSAFLUSH,
    /// Applies changes immediately. Buffers are not affected.
    TCSANOW,
    /// Flushes the terminal output buffer, and keeps the input buffer.
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

/// Struct that encapsulates terminal option setting. It has convenience
/// methods for setting raw mode, cooked mode, password (noecho) mode, and
/// resetting the terminal.
///
/// `.new()` takes two arguments: the first should implement io::Read, and
/// the second should implement both io::Write and fd::AsRawFd. For a
/// terminal application, these will likely be stdin and stdout,
/// respectively. Note that the fd associated with the second parameter
/// will be the target for the terminal ioctls.
///
/// Example:
///
/// ```
/// use std::io::{stdin, stdout};
/// let mut t = Term::new(stdin(), stdout())?;
/// // set raw mode
/// t.raw_mode()
///     .set(SetAction::TCSAFLUSH)?;
/// // read a keystroke into a buffer
/// let mut buf = [0u8; 4];
/// // Term implements the io::Read trait (and Write, too)
/// let bytes_read = t.read(&mut buf)?;
/// // Term remembers the state of the terminal when it was created
/// t.reset(SetAction::TCSANOW)?;
/// println!("read {} bytes", bytes_read);
/// println!("buffer is {:?}", buf);
/// ```
#[derive(Debug)]
pub struct Term<I, O> {
    fd_out: O,
    fd_in: I,
    orig_t: termios,
    t: termios,
}
impl<I: Read, O> std::io::Read for Term<I, O> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.fd_in.read(buf)
    }
}
impl<I, O: Write> std::io::Write for Term<I, O> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.fd_out.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.fd_out.flush()
    }
}
impl<O: AsRawFd, I> Term<I, O> {
    pub fn new(fd_in: I, fd_out: O) -> io::Result<Self> {
        let orig_t = get_termios(fd_out.as_raw_fd())?;
        Ok(Self {
            fd_out,
            fd_in,
            orig_t,
            t: orig_t.clone(),
        })
    }
    /// raw mode: unsets ECHO and ICANON, disable output flow control,
    /// disable ctrl-v, disable input carriage return translation (ctrl-m),
    /// disable ctrl-c signalling, and some other stuff. Note that this
    /// function disables output processing (auto carriage return insert).
    pub fn raw_mode(&mut self) -> &mut Self {
        self.t.c_iflag &= !(IXON | ICRNL | BRKINT | INPCK | ISTRIP);
        self.t.c_lflag &= !(ECHO | ICANON | IEXTEN | ISIG);
        self.t.c_oflag &= !(OPOST);
        self.t.c_cflag &= !libc::CS8;
        self
    }
    /// cooked mode: sets ECHO, ECHONL, ICANON. Note that this is the bare
    /// minimum to get utf input awareness and character display; it does
    /// not restore flow control, unset the input timeout, etc. The
    /// `.reset()` method is a more reliable way to recover from raw and
    /// password modes.
    pub fn cooked_mode(&mut self) -> &mut Self {
        self.t.c_lflag |= (ECHO | ECHONL | ICANON);
        self
    }
    /// Turns off all output processing, like translating newline into
    /// carriage return + newline
    pub fn disable_output_processing(&mut self) -> &mut Self {
        self.t.c_oflag &= !OPOST;
        self
    }
    /// Enables output processing (OPOST flag). One effect this has is to
    /// tell the terminal to automatically insert carriage returns before
    /// newlines in the output. If your output unexpectedly looks like
    /// this:
    ///
    ///     line of text
    ///                 next line of text
    ///
    /// then try enabling output processing.
    pub fn enable_output_processing(&mut self) -> &mut Self {
        self.t.c_oflag |= OPOST;
        self
    }
    /// Sets ECHO and ECHONL. Useful if you want to echo keystrokes in raw
    /// mode for some reason.
    pub fn enable_echo(&mut self) -> &mut Self {
        self.t.c_lflag |= (ECHO | ECHONL);
        self
    }
    /// Unsets ECHO and ECHONL. If you are prompting for a password, use
    /// `.password_mode()` instead.
    pub fn disable_echo(&mut self) -> &mut Self {
        self.t.c_lflag &= !(ECHO | ECHONL);
        self
    }
    /// password mode: unset ECHO, set ECHONL
    pub fn password_mode(&mut self) -> &mut Self {
        self.t.c_lflag &= !ECHO;
        self.t.c_lflag |= (ECHONL | ICANON);
        self
    }
    /// disable output flow control (ctrl-s and ctrl-q)
    pub fn disable_flow_control(&mut self) -> &mut Self {
        self.t.c_iflag &= !IXON;
        self
    }
    /// enable output flow control (ctrl-s and ctrl-q)
    pub fn enable_flow_control(&mut self) -> &mut Self {
        self.t.c_iflag |= IXON;
        self
    }
    /// Set input timeout, granularity is tenths of a second. Values over
    /// 25.5s are set to 25.5s, values under 0.1s are set to 0.1s.
    /// Useful only when the terminal has been set to raw mode.
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
    /// Disables a previously set input timeout.
    pub fn disable_input_timeout(&mut self) -> &mut Self {
        self.t.c_cc[libc::VMIN] = 1;
        self.t.c_cc[libc::VTIME] = 0;
        self
    }
    /// Applies all changes to the terminal
    pub fn set(&self, action: SetAction) -> io::Result<()> {
        set_termios(self.fd_out.as_raw_fd(), action, &self.t)?;
        Ok(())
    }
    /// Restores the terminal to its original state
    pub fn reset(&mut self, action: SetAction) -> io::Result<()> {
        self.t = self.orig_t.clone();
        self.set(action)
    }
}
impl<I: Read, O: AsRawFd + Write> Term<I, O> {
    /// Convenience function that sets the terminal to password
    /// mode, prompts for a password, and resets the terminal.
    pub fn prompt_for_password(&mut self, prompt: impl std::fmt::Display) -> io::Result<Password> {
        self.password_mode().set(SetAction::TCSAFLUSH)?;
        let mut pw = Password::new();
        write!(self, "{}: ", prompt)?;
        self.fd_out.flush()?;
        pw.read_line(&mut self.fd_in)?;
        self.reset(SetAction::TCSAFLUSH)?;
        Ok(pw)
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
