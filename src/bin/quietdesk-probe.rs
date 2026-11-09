//! Test stand-in for the real shell, never installed. It reports where it ran and exactly what it received, so a test
//! compares that with what it sent. `QUIETDESK_PROBE_EXIT` sets its exit code, and a line on stdin is echoed when asked for.
//!
//! With `QUIETDESK_PROBE_LAUNCH` set it is the tests' launcher instead: it starts the rest of its command line, exactly as written,
//! on the real desktop with its own handles and environment, and ends with that program's exit code. A test run that is itself
//! hidden, as under QuietDesk, starts its commands this way: a command that stays where it started must not look hidden then.

use std::io::BufRead;
use std::path::Path;

const LAUNCH: &str = "QUIETDESK_PROBE_LAUNCH";

fn main() {
    let arguments = quietdesk::arguments_after_program();
    let arguments = String::from_utf16_lossy(&arguments);
    if std::env::var_os(LAUNCH).is_some() {
        // Single-threaded here: nothing else runs yet.
        unsafe { std::env::remove_var(LAUNCH) };
        let program = std::env::args_os().nth(1).expect("a program to launch");
        std::process::exit(quietdesk::run_command_line(Path::new(&program), arguments.trim_start(), quietdesk::Place::Real));
    }
    println!("desktop={}", quietdesk::current_desktop_name().expect("the desktop name"));
    println!("decided={}", std::env::var(quietdesk::DECIDED_VAR).unwrap_or_default());
    println!("args={arguments}");
    for argument in std::env::args_os().skip(1) {
        println!("argv={}", argument.to_string_lossy());
    }
    eprintln!("probe-stderr");
    if std::env::var_os("QUIETDESK_PROBE_READ_STDIN").is_some() {
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line).expect("a line on stdin");
        println!("stdin={}", line.trim_end());
    }
    let code = std::env::var("QUIETDESK_PROBE_EXIT").ok().and_then(|text| text.parse().ok()).unwrap_or(0);
    std::process::exit(code);
}
