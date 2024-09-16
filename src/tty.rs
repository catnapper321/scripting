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
use libc::{BRKINT, ICRNL, INPCK, ISTRIP, IUTF8, IXON, INLCR};
// output flags (oflag)
use libc::OPOST;
// misc flags (lflag)
use libc::{ECHO, ECHONL, ICANON, IEXTEN, ISIG};
// exports
pub mod password;
use password::*;

/// Specifies behavior of [`libc::tcsetattr`]. Used in this library by [`Term::set()`] and [`Term::reset()`].
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

/// A type that provides methods for common terminal operations. It is
/// stateless — it does not attempt to track terminal modes or cursor
/// position, render widgets, and so on.
///
/// This library currently supports Linux only via the `libc` crate.
///
/// # TLDR: All I want is to get a password
///
/// Check out the example for [`Self::prompt_for_password()`].
///
/// # How to use this
///
/// Terminal option setting usually follows this pattern:
///
/// 1. Get the current terminal settings and remember them
/// 2. Twiddle some bit flags to (hopefully) achieve the desired terminal behavior
/// 3. Send the settings to the terminal via ioctl
/// 4. Do stuff
/// 5. Repeat step 3 with the original terminal settings
///
/// This struct abstracts that pattern. It has convenience methods for
/// setting raw mode, cooked mode, password (noecho) mode, and resetting
/// the terminal.
///
/// [`Self::new()`] takes an input and an output argument. There are
/// several ways to call this, depending on what you need to do. The output
/// should always implement the `std::os::fd::AsRawFd` trait. If it also
/// implements `std::io::Write`, then the returned struct will also
/// implement `Write` and may be used to print output to the terminal. If
/// the input argument implements `std::io::Read`, then the returned struct
/// will also implement `Read` and may be used to get input from the
/// terminal. These trait implementations are strictly a convenience, and
/// the standard stream handles obtained from `std::io::{stdout, stdin}`
/// may be used as usual.
///
/// Note that the fd associated with the output argument will be the target
/// for the terminal ioctls.
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
/// // Term implements io::Read (and Write, too) if it's input / outputs do
/// let bytes_read = t.read(&mut buf)?;
/// // Term remembers the state of the terminal when it was created
/// t.reset(SetAction::TCSANOW)?;
/// println!("read {} bytes", bytes_read);
/// println!("buffer is {:?}", buf);
/// ```
#[derive(Debug, Clone)]
pub struct Term<I, O> {
    fd_out: O,
    fd_in: I,
    t: (termios, termios), // (original, working copy)
}
/// If the input argument to [`Self::new()`] implements `std::io::Read`, then
/// Term also gets a `Read` implementation.
impl<I: Read, O> std::io::Read for Term<I, O> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.fd_in.read(buf)
    }
}
/// If the output argument to [`Self::new()`] implements `std::io::Write`, then
/// Term also gets a `Write` implementation.
impl<I, O: Write> std::io::Write for Term<I, O> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.fd_out.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.fd_out.flush()
    }
}
/// If all you want to do is set some terminal options, then the input
/// argument to [`Self::new()`] may simply be set to (), as in this
/// example:
///
/// ```
/// # use std::io::Write;
/// let mut t = Term::new((), 1)?; // stdout is fd 1
/// t.password_mode().set(SetAction::TCSAFLUSH)?;
/// print!("Enter the password: ");
/// std::io::stdout().flush();
/// // …read the password from stdin somehow
/// // restore the terminal to it's original mode
/// t.reset(SetAction::TCSAFLUSH)?;
/// ```
impl<I, O: AsRawFd> Term<I, O> {
    /// Creates a new `Term` with the provided input and output. If the
    /// output does not accept terminal ioctls, then this will fail.
    pub fn new(input: I, output: O) -> io::Result<Self> {
        let t = get_termios(output.as_raw_fd())?;
        Ok(Self {
            fd_out: output,
            fd_in: input,
            t: (t.clone(), t),
        })
    }
    /// Returns false if the output is not connected to a terminal.
    pub fn is_a_tty(&self) -> bool {
        isatty(self.fd_out.as_raw_fd())
    }
    /// Attempts to save the settings from the terminal currently connected
    /// to the output. Future invocations of [`Self::reset()`] will use
    /// this state. 
    pub fn save(&mut self) -> io::Result<()> {
        let t = get_termios(self.fd_out.as_raw_fd())?;
        self.t = (t.clone(), t);
        Ok(())
    }
    /// Gives the provided fn direct access to the [`libc::termios`]
    /// struct.
    pub fn with_termios(&mut self, mut f: impl FnOnce(&mut libc::termios)) {
        f(&mut self.t.1);
    }
    /// Raw mode: unsets ECHO and ICANON, disables output flow control,
    /// disables ctrl-v, disables input carriage return translation
    /// (ctrl-m), disables ctrl-c signalling, and some other stuff. Note
    /// that this function disables output processing (auto carriage return
    /// insert).
    pub fn raw_mode(&mut self) -> &mut Self {
        self.with_termios(|t| {
            t.c_iflag &= !(IXON | ICRNL | BRKINT | INPCK | ISTRIP | INLCR);
            t.c_lflag &= !(ECHO | ICANON | IEXTEN | ISIG);
            t.c_oflag &= !(OPOST);
            t.c_cflag &= !libc::CS8;
        });
        self
    }
    /// Cooked mode: sets ECHO, ECHONL, ICANON. Note that this is the bare
    /// minimum to get utf input awareness and character display; it does
    /// not restore flow control, unset the input timeout, etc. The
    /// [`Self::reset()`] method is a more reliable way to recover from raw and
    /// password modes.
    pub fn cooked_mode(&mut self) -> &mut Self {
        self.with_termios(|t| {
            t.c_iflag |= IUTF8;
            t.c_lflag |= (ECHO | ECHONL | ICANON);
        });
        self
    }
    /// Turns off all output processing, like translating newline into
    /// carriage return + newline
    pub fn disable_output_processing(&mut self) -> &mut Self {
        self.with_termios(|t| {
            t.c_oflag &= !OPOST;
        });
        self
    }
    /// Enables output processing (OPOST flag). One effect this has is to
    /// tell the terminal to translate newlines in the input to carriage
    /// return + newline. If your output unexpectedly looks like this:
    ///
    ///     line of text
    ///                 next line of text
    ///
    /// then try enabling output processing.
    pub fn enable_output_processing(&mut self) -> &mut Self {
        self.with_termios(|t| {
            t.c_oflag |= OPOST;
        });
        self
    }
    /// Sets ECHO and ECHONL. Useful if you want to echo keystrokes in raw
    /// mode for some reason.
    pub fn enable_echo(&mut self) -> &mut Self {
        self.with_termios(|t| {
            t.c_lflag |= (ECHO | ECHONL);
        });
        self
    }
    /// Unsets ECHO and ECHONL. If you are prompting for a password, use
    /// `.password_mode()` instead.
    pub fn disable_echo(&mut self) -> &mut Self {
        self.with_termios(|t| {
            t.c_lflag &= !(ECHO | ECHONL);
        });
        self
    }
    /// Password mode: Disables keystroke echo, but ensures that newlines
    /// echo and that utf input is properly handled.
    pub fn password_mode(&mut self) -> &mut Self {
        self.with_termios(|t| {
            t.c_lflag &= !ECHO;
            t.c_lflag |= (ECHONL | ICANON);
        });
        self
    }
    /// Disable output flow control (ctrl-s and ctrl-q)
    pub fn disable_flow_control(&mut self) -> &mut Self {
        self.with_termios(|t| {
            t.c_iflag &= !IXON;
        });
        self
    }
    /// Enable output flow control (ctrl-s and ctrl-q)
    pub fn enable_flow_control(&mut self) -> &mut Self {
        self.with_termios(|t| {
            t.c_iflag |= IXON;
        });
        self
    }
    /// Set input timeout, granularity is tenths of a second. Values over
    /// 25.5s are set to 25.5s, values under 0.1s are set to 0.1s.
    /// Useful only when the terminal has been set to raw mode.
    pub fn input_timeout(&mut self, vtime: std::time::Duration) -> &mut Self {
        let mut tenths = vtime.as_millis() / 100;
        let tenths = match tenths {
            0 => 1,
            1..=255 => tenths as u8,
            _ => 255,
        };
        self.with_termios(|t| {
            t.c_cc[libc::VMIN] = 0; // return immediately after one byte read
            t.c_cc[libc::VTIME] = tenths;
        });
        self
    }
    /// Disables a previously set input timeout.
    pub fn disable_input_timeout(&mut self) -> &mut Self {
        self.with_termios(|t| {
            t.c_cc[libc::VMIN] = 1;
            t.c_cc[libc::VTIME] = 0;
        });
        self
    }
    /// Applies changes to the terminal.
    pub fn set(&self, action: SetAction) -> io::Result<()> {
        set_termios(self.fd_out.as_raw_fd(), action, &self.t.1)
    }
    /// Restores the terminal to its original state
    pub fn reset(&mut self, action: SetAction) -> io::Result<()> {
        self.t.1 = self.t.0.clone();
        self.set(action)
    }
}
impl<I: Read, O: AsRawFd + Write> Term<I, O> {
    /// Convenience function that sets the terminal to password mode,
    /// prompts for a password, and resets the terminal. A `": "` sequence is
    /// automatically appended to the prompt, and the trailing newline in the
    /// input is automatically trimmed. Example:
    ///
    /// ```
    /// use std::io::{stdin, stdout};
    /// let mut t = Term::new(stdin(), stdout())?;
    /// let pw = t.prompt_for_password("Enter the password")?;
    /// println!("Password entered was {:?}", pw.as_str());
    /// ```
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
