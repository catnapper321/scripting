//! Utilities useful for scripting and programs that run in a terminal.

#![allow(dead_code, unused)]
use std::{
    ffi::{CStr, CString, OsStr, OsString},
    fmt::Display,
    io::{self, stdin, stdout, Read, Write},
    ops::{Deref, DerefMut},
    os::fd::AsRawFd,
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    process::Command,
    env::{args_os, ArgsOs},
};
mod tty;
pub use tty::{password::*, SetAction, Term};

pub type DoasUser = String;
pub type DoasUid = u32;

#[derive(Debug)]
pub struct Keystroke([u8; 4]);
impl Keystroke {
    pub fn new() -> Self {
        Self([0; 4])
    }
    pub fn is_empty(&self) -> bool {
        self.0 == [0, 0, 0, 0]
    }
    pub fn is_ctrl_c(&self) -> bool {
        self.0 == [3, 0, 0, 0]
    }
    pub fn is_esc(&self) -> bool {
        self.0 == [27, 0, 0, 0]
    }
    pub fn is_esc_code(&self) -> bool {
        self.0[0] == 27 && self.0[1] == b'['
    }
    pub fn is_enter(&self) -> bool {
        self.0[0] == 13
    }
    pub fn as_char(&self) -> Option<char> {
        char::from_u32(u32::from_ne_bytes(self.0))
    }
}
impl Deref for Keystroke {
    type Target = [u8; 4];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DerefMut for Keystroke {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// must set terminal to raw mode prior to call
pub fn get_raw_keystroke<I: Read, O>(term: &mut tty::Term<I, O>) -> io::Result<Keystroke> {
    let mut keystroke = Keystroke::new();
    let n = term.read(keystroke.as_mut_slice())?;
    Ok(keystroke)
}

pub fn keystroke<I: Read, O: AsRawFd>(term: &mut Term<I, O>) -> io::Result<Keystroke> {
    term.raw_mode().set(SetAction::TCSAFLUSH)?;
    let keystroke = get_raw_keystroke(term);
    term.reset(SetAction::TCSANOW)?;
    keystroke
}

pub fn prompt_yn<I: Read, O: AsRawFd>(
    term: &mut Term<I, O>,
    default: Option<bool>,
    msg: impl Display,
) -> bool {
    loop {
        if let Some(default) = default {
            if default {
                print!("{} [yn] (default y)? ", msg);
            } else {
                print!("{} [yn] (default n)? ", msg);
            }
        } else {
            print!("{} [yn]? ", msg);
        }
        _ = stdout().flush();
        let keystroke = keystroke(term).unwrap();
        if keystroke.is_enter() {
            if let Some(default) = default {
                return default;
            }
        }
        if let Some(c) = keystroke.as_char() {
            match c {
                'y' | 'Y' => return true,
                'n' | 'N' => return false,
                _ => continue,
            }
        }
    }
}

pub fn press_any_key<I: Read, O: AsRawFd + Write>(term: &mut Term<I, O>) {
    writeln!(term, "Press any key to continue.");
    _ = term.flush();
    _ = term.raw_mode().set(SetAction::TCSAFLUSH);
    _ = keystroke(term);
    _ = term.reset(SetAction::TCSANOW);
}

pub fn prompt_menu<I: Read, O: AsRawFd + Write>(
    term: &mut Term<I, O>,
    default: Option<char>,
    prompt: impl AsRef<str>,
    menu: impl IntoIterator<Item = impl AsRef<str>>,
) -> char {
    let mut choices = String::new();
    // print the menu
    for line in menu {
        let s: &str = line.as_ref();
        let mut chars = s.char_indices();
        if let Some((_, opt)) = chars.next() {
            if choices.contains(&[opt]) {
                panic!("'{opt}' is a duplicate menu option");
            }
            choices.push(opt);
            if let Some((text_index, _)) = chars.next() {
                println!("{opt}){}", unsafe { s.get_unchecked(text_index..) });
            }
        }
    }
    // check that default option exists
    if let Some(d) = default {
        if !choices.contains(&[d]) {
            panic!("default choice '{d}' is not a menu option");
        }
    }
    loop {
        if let Some(d) = default {
            print!("\n{} [{choices}] (default {d})? ", prompt.as_ref());
        } else {
            print!("\n{} [{choices}]? ", prompt.as_ref());
        }
        _ = stdout().flush();
        let keystroke = keystroke(term).unwrap();
        if keystroke.is_enter() {
            if let Some(default) = default {
                return default;
            }
        };
        if let Some(c) = keystroke.as_char() {
            if choices.contains(&[c]) {
                return c;
            }
            println!("'{c}' is not a menu option");
        }
    }
}

pub fn underscored_heading(msg: impl AsRef<str>) {
    let msg = msg.as_ref();
    let mut guard = stdout().lock();
    _ = writeln!(guard, "{msg}");
    for _ in msg.chars() {
        _ = write!(guard, "-");
    }
    _ = writeln!(guard, "");
}

pub fn is_root_user() -> bool {
    nix::unistd::geteuid().is_root()
}

// Checks euid, execs this process with doas if not root.
// Sets DOAS_UID to the current euid prior to exec.
// Returns the DOAS_USER and DOAS_UID env variable after exec
pub fn ensure_running_doas() -> Result<(DoasUser, DoasUid), std::io::Error> {
    let euid = nix::unistd::geteuid();
    if euid.is_root() {
        let user = std::env::var("DOAS_USER").expect("Should be running with doas");
        let uid = std::env::var("DOAS_UID")
            .expect("Should have found DOAS_UID environment variable")
            .parse::<u32>()
            .expect("DOAS_UID environment variable should be parseable as an integer");
        return Ok((user, uid));
    }
    unsafe {
        std::env::set_var("DOAS_UID", euid.to_string());
    }
    let current_exe = std::env::current_exe()?;
    doas(current_exe, args_os()).expect("Unable to execute doas");
    unreachable!()
}

fn doas(executable: PathBuf, cli_args: ArgsOs) -> Result<(), std::ffi::NulError> {
    // TODO: make this configurable? rely on PATH instead?
    let doas_bin = CString::new(b"doas".to_vec())?;

    let cstring_args: Vec<CString> = cli_args
        .map(|x| CString::new(x.as_bytes()))
        .flatten()
        .collect();

    let executable = CString::new(executable.into_os_string().into_vec())?;

    // build &CStr args for the exec call
    let mut args: Vec<&CStr> = Vec::new();
    args.push(&executable); // expected to be the name of the executable
    for arg in cstring_args.iter() {
        args.push(arg.as_c_str());
    }
    // exec
    nix::unistd::execvp(&doas_bin, args.as_slice()).expect("Should have execed a new process");
    unreachable!()
}
