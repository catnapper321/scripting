use std::{
    ffi::CStr,
    io::{self, Read},
};

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
    /// given Reader. Removes the trailing newline from the input. If the
    /// input is larger than `PASSWORD_BUFFER_LEN`, returns an error of
    /// `std::io::ErrorKind::InvalidData`.
    pub fn read_line(&mut self, mut fd: &mut impl Read) -> io::Result<()> {
        let mut index = 0;
        loop {
            let buf = &mut self.buf[index..];
            let n = fd.read(buf)?;
            if n == 0 { break; }
            index += n;
            if index >= PASSWORD_BUFFER_LEN { return Err(io::ErrorKind::InvalidData.into()); }
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

