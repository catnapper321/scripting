#![allow(dead_code, unused)]
use std::{
    io::{self, Write, Read, stdout, stdin}, 
    fmt::Display,
    process::Command,
    path::{Path, PathBuf},
    ffi::{CString, CStr, OsStr, OsString},
    os::unix::ffi::{OsStrExt, OsStringExt},
};
pub use creche::*;

#[derive(Debug)]
pub struct Keystroke([u8; 4]);
impl Keystroke {
    pub fn is_ctrl_c(&self) -> bool {
        self.0 == [3, 0, 0, 0]
    }
    pub fn is_esc(&self) -> bool {
        self.0 == [27, 0, 0, 0]
    }
    pub fn is_enter(&self) -> bool {
        self.0[0] == 13
    }
    pub fn as_char(&self) -> Option<char> {
        char::from_u32(u32::from_ne_bytes(self.0))
    }
}

/// must set tty to raw mode prior to call
pub fn get_raw_keystroke() -> Result<Keystroke, io::Error> {
    let mut buf = [0u8; 4];
    let _bytes_read = stdin().read(&mut buf)?;
    Ok(Keystroke(buf))
}

pub fn set_tty<S>(args: impl IntoIterator<Item = S>) -> Result<(), io::Error> where 
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new("/usr/bin/stty");
    cmd.args(args);
    let mut child = cmd.spawn()?;
    child.wait()?;
    Ok(())
}

pub fn set_tty_raw_noecho() -> Result<(), io::Error> {
    set_tty(["raw", "-echo"])
}

pub fn set_tty_cooked() -> Result<(), io::Error> {
    set_tty(["cooked", "echo"])
}

pub fn keystroke() -> Result<Keystroke, io::Error> {
    set_tty(["raw"])?;
    let x = get_raw_keystroke();
    set_tty_cooked()?;
    println!("");
    x
}

pub fn keystroke_noecho() -> Result<Keystroke, io::Error> {
    set_tty_raw_noecho()?;
    let x = get_raw_keystroke();
    set_tty_cooked()?;
    x
}

pub fn prompt_yn(default: Option<bool>, msg: impl Display) -> bool {
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
        let keystroke = keystroke().unwrap();
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

pub fn press_any_key() {
    println!("Press any key to continue.");
    _ = stdout().flush();
    _ = keystroke_noecho();
}

pub fn prompt_menu(
    default: Option<char>,
    prompt: impl AsRef<str>,
    menu: impl IntoIterator<Item = impl AsRef<str>>,
) -> char {
    let mut choices = String::new();
    // print the menu
    // println!("");
    for line in menu {
        let s: &str = line.as_ref();
        let mut chars = s.char_indices();
        if let Some((_, opt)) = chars.next() {
            if choices.contains(&[opt]) { panic!("'{opt}' is a duplicate menu option"); }
            choices.push(opt);
            if let Some((text_index, _)) = chars.next() {
                println!("{opt}){}", unsafe {s.get_unchecked(text_index..)});
            }
        }
    }
    // check that default option exists
    if let Some(d) = default {
        if ! choices.contains(&[d]) {
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
        let keystroke = keystroke().unwrap();
        if keystroke.is_enter() {
            if let Some(default) = default {
                return default;
            }
        };
        if let Some(c) = keystroke.as_char() {
            if choices.contains(&[c]) { return c; }
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

// Checks euid, execs this process with doas if not root.
// Sets DOAS_UID to the current euid prior to exec.
// Returns the DOAS_USER and DOAS_UID env variable after exec
pub fn ensure_running_doas() -> Result<(String, u32), std::io::Error> {
    let euid = nix::unistd::geteuid();
    if euid.is_root() {
        let user =
            std::env::var("DOAS_USER").expect("Should have found DOAS_USER environment variable");
        let uid = std::env::var("DOAS_UID")
            .expect("Should have found DOAS_UID environment variable")
            .parse::<u32>()
            .expect("DOAS_UID environment variable should be parseable as an integer");
        return Ok((user, uid));
    }
    unsafe { std::env::set_var("DOAS_UID", euid.to_string()); }
    let current_exe = std::env::current_exe()?;
    let args = std::env::args_os();
    doas(current_exe, args).expect("Unable to execute doas");
    unreachable!()
}

fn doas(
    executable: impl Into<Argument>,
    args: impl IntoIterator<Item = impl Into<Argument>>,
) -> Result<(), std::ffi::NulError> {
    let doas_path = PathBuf::from("/usr/bin/doas");
    if !doas_path.exists() {
        panic!("doas utility not found");
    }
    let doas_bin = CString::new(doas_path.as_os_str().as_bytes())?;

    let cstring_args: Vec<CString> = args
        .into_iter()
        .map(Into::<Argument>::into)
        .map(|arg| arg.into_c_string())
        .collect();

    // build CString args for the exec call
    let executable = executable.into();
    let mut xs: Vec<&CStr> = Vec::new();
    xs.push(&executable);
    xs.push(&executable); // expected to be the name of the executable
    for arg in cstring_args.iter() {
        xs.push(arg.as_c_str());
    }
    // exec
    nix::unistd::execvp(&doas_bin, xs.as_slice()).expect("Should have execed a new process");
    unreachable!()
}
