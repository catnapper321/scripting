use std::{
    ffi::CStr,
    io::{self, Read},
};

/// Buffer size was selected to hold at least 127 UTF-8 characters.
pub const PASSWORD_BUFFER_LEN: usize = 512;
/// Type that owns a buffer on the heap that will not reallocate. It is
/// intended to hold user entered password data. When dropped, it
/// overwrites the buffer contents with nul bytes.
///
/// The fixed buffer size, defined by [`PASSWORD_BUFFER_LEN`], should
/// allow it to hold very long UTF8 password data.
///
/// Example:
/// ```
/// let mut pw = Password::new();
/// pw.read_line(std::io::stdin())?;
/// if pw.as_str() == "SECRET" {
///     println!("password is correct");
/// } else {
///     println!("wrong password");
/// }
/// ```
pub struct Password {
    buf: Box<[u8; PASSWORD_BUFFER_LEN]>,
}
impl Password {
    pub fn new() -> Self {
        Self {
            buf: Box::new([0; PASSWORD_BUFFER_LEN]),
        }
    }
    // /// Returns true if the buffer's last byte is a nul
    // pub fn is_nul_terminated(&self) -> bool {
    //     self.buf[PASSWORD_BUFFER_LEN - 1] == 0
    // }
    /// Returns a `&CStr` to the buffer data.
    pub fn as_cstr(&self) -> &CStr {
        CStr::from_bytes_until_nul(self.buf.as_slice())
            .expect("Password buffer requires terminating nul byte")
    }
    /// Returns a `&str` if the buffer contains UTF-8 data.
    pub fn as_str(&self) -> Result<&str, std::str::Utf8Error> {
        self.as_cstr().to_str()
    }
    /// Convenience method for reading a newline terminated input from the
    /// given Reader. Removes the trailing newline from the input. Inputs
    /// larger than [`PASSWORD_BUFFER_LEN`] - 1 are truncated.
    pub fn read_line(&mut self, mut fd: &mut impl Read) -> io::Result<()> {
        let mut index = 0;
        loop {
            let buf = &mut self.buf[index..PASSWORD_BUFFER_LEN - 1];
            let n = fd.read(buf)?;
            if n == 0 {
                break;
            }
            index += n;
            if self.buf[index - 1] == b'\n' {
                // replace the trailing newline with a nul byte
                self.buf[index - 1] = 0;
                break;
            }
            // truncate large inputs
            if index >= PASSWORD_BUFFER_LEN - 1 {
                break;
            }
        }
        Ok(())
    }
    /// Returns a slice of bytes containing the password data without a
    /// trailing nul byte. Equivalent to `Self::as_cstr().to_bytes()`.
    pub fn as_bytes(&self) -> &[u8] {
        self.as_cstr().to_bytes()
    }
    /// Returns a mutable slice for the buffer. The slice size is one less
    /// than [`PASSWORD_BUFFER_LEN`] so that the buffer will always have a
    /// terminal nul byte. Useful if you would rather roll your own input
    /// routine and just need a buffer for the secret. Example:
    /// ```
    /// let stdin = std::io::stdin();
    /// let mut pw = Password::new();
    /// // read from stdin directly into the Password buffer
    /// let bytes_read = stdin.read(pw.as_mut_slice()).unwrap();
    /// ```
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self.buf.as_mut_slice()
    }
}
impl Drop for Password {
    fn drop(&mut self) {
        self.buf.fill(0);
        use std::sync::atomic::{self, Ordering};
        atomic::compiler_fence(Ordering::SeqCst);
        atomic::fence(Ordering::SeqCst);
    }
}
