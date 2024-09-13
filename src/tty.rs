#![allow(unused)]
use std::mem::MaybeUninit;
use std::{
    os::fd::{AsFd, AsRawFd},
    io::{self, Read, Write, stdout, stdin},
    mem,
    ffi::CStr,
};
use libc::{
    c_int,
    termios,
};

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

const STDOUT_DEFAULT_TERMIOS: std::cell::OnceCell<termios> = std::cell::OnceCell::new();

const PASSWORD_BUFFER_LEN: usize = 512;
/// Type that owns a heap allocated buffer that will not reallocate to
/// accomodate input greater than PASSWORD_BUFFER_LEN bytes. When dropped,
/// it overwrites the buffer contents with nul bytes. Users must ensure
/// that the buffer contains at least one nul byte before calling
/// [`as_cstr()`] or [`as_str()`].
pub struct Password {
    buf: Box<[u8; PASSWORD_BUFFER_LEN]>,
}
impl Password {
    pub fn new() -> Self {
        Self { buf: Box::new([0; PASSWORD_BUFFER_LEN]) }
    }
    pub fn is_nul_terminated(&self) -> bool {
        self.buf[PASSWORD_BUFFER_LEN - 1] == 0
    }
    /// Panics if the buffer does not contain a nul byte
    pub fn as_cstr(&self) -> &CStr {
        CStr::from_bytes_until_nul(self.buf.as_slice()).expect("Password buffer requires terminating nul byte")
    }
    pub fn as_str(&self) -> Result<&str, std::str::Utf8Error> {
        self.as_cstr().to_str()
    }
    /// Convenience method for reading a newline terminated input from the
    /// given Reader. Panics if the buffer would overflow. Removes the
    /// trailing newline from the input.
    pub fn read_line(&mut self, mut fd: impl Read) -> io::Result<()> {
        let mut index = 0;
        loop {
            let buf = &mut self.buf[index..];
            let n = fd.read(buf)?;
            if n == 0 { break; }
            index += n;
            if index >= PASSWORD_BUFFER_LEN { panic!("Password buffer length exceeded"); }
            if self.buf[index - 1] == b'\n' {
                // replace the trailing newline with a nul byte
                self.buf[index - 1] = 0;
                break;
            }
        }
        Ok(())
    }
}
impl Drop for Password {
    fn drop(&mut self) {
        self.buf.fill(0);
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
        std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
    }
}
impl std::ops::Deref for Password {
    type Target = [u8; PASSWORD_BUFFER_LEN];

    fn deref(&self) -> &Self::Target {
        self.buf.as_ref()
    }
}
impl std::ops::DerefMut for Password {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.buf.as_mut()
    }
}


/// raw mode: unset ECHO and ICANON, disable output flow control, disable
/// ctrl-v, disable carriage return translation (ctrl-m), disable ctrl-c
/// signalling, and some other stuff. Note that this function disables
/// output processing (auto carriage return insert).
pub fn termios_set_raw_mode(t: &mut termios) {
    t.c_iflag &= ! (IXON | ICRNL | BRKINT | INPCK | ISTRIP);
    t.c_lflag &= ! (ECHO | ICANON | IEXTEN | ISIG);
    t.c_oflag &= ! (OPOST);
    t.c_cflag &= ! libc::CS8;
}

/// cooked mode: set ECHO, ECHONL, ICANON
/// note that this does the bare minimum to get utf input awareness and
/// character display; it does not restore flow control, unset the input
/// timeout, etc.
pub fn termios_set_cooked_mode(t: &mut termios) {
    t.c_lflag |= (ECHO | ECHONL | ICANON);
}

/// Turns off all output processing, like translating newline into
/// carriage return + newline
pub fn termios_disable_output_processing(t: &mut termios) {
    t.c_oflag &= ! OPOST
}

/// Enables output processing, so that newlines automatically have carriage
/// returns inserted before them
pub fn termios_enable_output_processing(t: &mut termios) {
    t.c_oflag |= OPOST
}

/// password mode: unset ECHO, set ECHONL
pub fn termios_set_password_mode(t: &mut termios) {
    t.c_lflag &= ! ECHO;
    t.c_lflag |= (ECHONL | ICANON);
}

/// disable output flow control (ctrl-s and ctrl-q)
pub fn termios_disable_flow_control(t: &mut termios) {
    t.c_iflag &= ! IXON;
}
/// enable output flow control (ctrl-s and ctrl-q)
pub fn termios_enable_flow_control(t: &mut termios) {
    t.c_iflag |= IXON;
}
/// Set input timeout, granularity is tenths of a second. Values over
/// 25.5s are set to 25.5s, values under 0.1s are set to 0.1s
pub fn termios_set_input_timeout(t: &mut termios, vtime: std::time::Duration) {
    t.c_cc[libc::VMIN] = 0; // return immediately after one byte read
    let mut tenths = vtime.as_millis() / 100;
    let tenths = match tenths {
        0 => 1,
        1..=255 => tenths as u8,
        _ => 255,
    };
    t.c_cc[libc::VTIME] = tenths;
}

/// Convenience function that sets the terminal on stdout to password mode,
/// prompts for a password, and resets the terminal. Uses [`default_termios()`]
/// internally, and so requires [`init()`] to be called first.
pub fn get_password(prompt: impl std::fmt::Display) -> io::Result<Password> {
    let mut t = get_termios(stdout())?;
    let orig_t = get_termios(stdout())?;
    termios_set_password_mode(&mut t);
    set_termios(stdout(), SetAction::TCSAFLUSH, &t)?;
    let mut pw = Password::new();
    print!("{}: ", prompt);
    stdout().flush()?;
    pw.read_line(stdin())?;
    set_termios(std::io::stdout(), SetAction::TCSAFLUSH, &orig_t)?;
    Ok(pw)
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

