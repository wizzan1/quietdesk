//! The value of `CLAUDE_CODE_SHELL_PREFIX`. Claude Code starts it in two ways (read from Claude Code 2.1.317, and captured):
//!
//! - The Bash tool and hooks: Git Bash runs `quietdesk-bash '<script>'`. Git Bash rewrites what looks like its own paths before a
//!   Windows program gets it (`/usr/bin/env` becomes `C:/Program Files/Git/usr/bin/env`, `$PWD` a Windows path), so the script is
//!   read from the command line Claude Code gave Git Bash itself, and the environment is the one Claude Code gave it. Both go on to
//!   that same bash, the script written so that bash reads back exactly what it was given.
//! - An MCP server: Claude Code starts this program itself, with the server's command and arguments as one line of shell words.
//!   The program is found and started as Claude Code's own starter, cross-spawn, would start it with no prefix: by PATHEXT, with
//!   batch files such as `npx.cmd` through `cmd.exe`, and scripts through the interpreter on their `#!` line.
//!
//! Either way it runs on the hidden desktop while QuietDesk is on, and nothing else about it changes.

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use quietdesk::Place;
use quietdesk::shell::{cmd_line, cygwin_argument, cygwin_words, find_program, msvcrt_line, msys_path, plain_words, shebang_interpreter};

const BASH_NAME: &str = "bash.exe";
/// What a shell returns for a command it cannot find.
const NOT_FOUND: i32 = 127;
/// What a shell returns when it was called wrongly.
const MISUSED: i32 = 2;
/// Windows' own list when PATHEXT is not set.
const DEFAULT_PATH_EXTENSIONS: &str = ".EXE;.CMD;.BAT;.COM";
/// As much of a file as cross-spawn reads to find its `#!` line.
const SHEBANG_BYTES: usize = 150;

fn usable(path: PathBuf) -> Option<PathBuf> {
    (path.file_name().is_some_and(|name| name.eq_ignore_ascii_case(BASH_NAME)) && path.is_file()).then_some(path)
}

/// Git Bash, the way Claude Code itself is told where it is, then its usual install. Not a PATH search: `bash.exe` in
/// System32 is WSL's, which is another operating system.
fn git_bash() -> Option<PathBuf> {
    let from_env = |name: &str| std::env::var_os(name).map(PathBuf::from).and_then(usable);
    from_env("CLAUDE_CODE_GIT_BASH_PATH").or_else(|| from_env("SHELL")).or_else(|| {
        let program_files = std::env::var_os("ProgramFiles")?;
        usable(Path::new(&program_files).join(r"Git\bin").join(BASH_NAME))
    })
}

/// The Git Bash that started this program, which then runs the script just as it would have run it with no prefix.
fn bash_parent() -> Option<(u32, PathBuf)> {
    let (pid, path) = quietdesk::parent_process()?;
    let path = PathBuf::from(path);
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    let windows = PathBuf::from(std::env::var_os("SystemRoot")?);
    ((name == "bash.exe" || name == "sh.exe") && !path.starts_with(windows)).then_some((pid, path))
}

/// The script as Claude Code gave it to Git Bash, which runs `-c [-l] "'<this program>' '<script>'"`, and whether that bash is a
/// login shell. None for any other shape: then the argument this program got is all there is.
fn script_from_parent(parent: u32) -> Option<(String, bool)> {
    let words = cygwin_words(&quietdesk::command_line_of(parent)?);
    let after_c = words.iter().position(|word| word == "-c")? + 1;
    let script_at = (after_c..words.len()).find(|&index| !words[index].starts_with('-'))?;
    let login = words[1..script_at].iter().any(|word| word == "-l" || word == "--login");
    let inner = plain_words(&words[script_at])?;
    let [program, script] = inner.as_slice() else { return None };
    let own = std::env::current_exe().ok()?;
    let same = |a: &Path, b: &Path| a.to_string_lossy().replace('/', "\\").eq_ignore_ascii_case(&b.to_string_lossy().replace('/', "\\"));
    same(Path::new(program), &own).then(|| (script.clone(), login))
}

/// QuietDesk's own variables, such as whether the run has been decided: this program's, not the environment's it hands on.
fn is_ours(entry: &[u16]) -> bool {
    String::from_utf16_lossy(entry).to_ascii_uppercase().starts_with("QUIETDESK_")
}

/// The environment Claude Code gave the Git Bash that started this program, for the bash below, which then starts as that one did,
/// with nothing Git Bash rewrote for a Windows program. QuietDesk's own variables come from this program. Sorted, as Windows wants
/// a block it is given.
fn environment_for_bash(parent: u32) -> Option<Vec<u16>> {
    let given = quietdesk::initial_environment_of(parent)?;
    let mut entries: Vec<Vec<u16>> = given.split(|&unit| unit == 0).filter(|entry| !entry.is_empty() && !is_ours(entry)).map(<[u16]>::to_vec).collect();
    for (name, value) in std::env::vars_os() {
        let entry: Vec<u16> = name.encode_wide().chain(std::iter::once(u16::from(b'='))).chain(value.encode_wide()).collect();
        if is_ours(&entry) {
            entries.push(entry);
        }
    }
    let name_of = |entry: &Vec<u16>| String::from_utf16_lossy(entry).to_uppercase();
    entries.sort_by_cached_key(name_of);
    let mut block: Vec<u16> = entries.into_iter().flat_map(|entry| entry.into_iter().chain(std::iter::once(0))).collect();
    block.push(0);
    if block.len() == 1 {
        block.push(0);
    }
    Some(block)
}

/// Starts an MCP server's command as cross-spawn would, which is how Claude Code starts it with no prefix.
fn start_directly(words: &[String], place: Place) -> i32 {
    let working_folder = std::env::current_dir().unwrap_or_default();
    let path = std::env::var("PATH").unwrap_or_default();
    let extensions = std::env::var("PATHEXT").unwrap_or_else(|_| DEFAULT_PATH_EXTENSIONS.to_string());
    let find = |name: &str| find_program(name, &working_folder, &path, &extensions);
    let (mut command, mut arguments) = (words[0].clone(), words[1..].to_vec());
    let Some(mut file) = find(&command) else {
        eprintln!("quietdesk: cannot find {command}");
        return NOT_FOUND;
    };
    let extension = |file: &Path| file.extension().map(|extension| extension.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    // A script with no Windows extension names its interpreter in its first line.
    if !matches!(extension(&file).as_str(), "exe" | "com" | "cmd" | "bat")
        && let Some(interpreter) = std::fs::read(&file).ok().and_then(|bytes| shebang_interpreter(&bytes[..bytes.len().min(SHEBANG_BYTES)]))
    {
        arguments.insert(0, file.to_string_lossy().into_owned());
        command = interpreter;
        let Some(found) = find(&command) else {
            eprintln!("quietdesk: cannot find {command}, which {} names on its first line", arguments[0]);
            return NOT_FOUND;
        };
        file = found;
    }
    if matches!(extension(&file).as_str(), "exe" | "com") {
        let mut line = vec![file.to_string_lossy().into_owned()];
        line.extend(arguments);
        return quietdesk::run_command_line(&file, &msvcrt_line(&line), place);
    }
    let comspec = std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".to_string());
    let shim = file.to_string_lossy().to_ascii_lowercase().replace('/', "\\");
    let double_escape = shim.ends_with(".cmd") && shim.contains(r"node_modules\.bin\");
    quietdesk::run_command_line(Path::new(&comspec), &cmd_line(&comspec, &command, &arguments, double_escape), place)
}

fn main() {
    let Some(arguments) = std::env::args_os().skip(1).map(|argument| argument.into_string().ok()).collect::<Option<Vec<String>>>() else {
        eprintln!("quietdesk: the command is not Unicode text");
        std::process::exit(MISUSED);
    };
    let Some((given, rest)) = arguments.split_first() else {
        eprintln!("quietdesk: no command given");
        std::process::exit(MISUSED);
    };
    let place = quietdesk::decide_place_for(false, &arguments.join(" "));
    let parent = bash_parent();

    if parent.is_none()
        && rest.is_empty()
        && let Some(words) = plain_words(given)
    {
        std::process::exit(start_directly(&words, place));
    }

    let Some(bash) = parent.as_ref().map(|(_, path)| path.clone()).or_else(git_bash) else {
        eprintln!("quietdesk: Git Bash not found; set CLAUDE_CODE_GIT_BASH_PATH");
        std::process::exit(NOT_FOUND);
    };
    let (script, login) = match &parent {
        Some((pid, _)) if rest.is_empty() => script_from_parent(*pid).unwrap_or_else(|| (given.clone(), false)),
        _ => (given.clone(), false),
    };
    let environment = parent.as_ref().and_then(|(pid, _)| environment_for_bash(*pid));
    if parent.is_some() && environment.is_none() {
        // The environment Claude Code gave could not be read: this program's, with what Git Bash rewrote for it put back as far as
        // it can be. Git Bash wrote these as Windows paths, and the bash below would keep them that way.
        // Single-threaded here: nothing else runs yet.
        unsafe { std::env::remove_var("PWD") };
        if let Some(old) = std::env::var("OLDPWD").ok().and_then(|path| msys_path(&path)) {
            unsafe { std::env::set_var("OLDPWD", old) };
        }
        // Git Bash's login start-up sets these only when they are missing, and they came back as Windows paths: without them
        // the login shell below sets them as the one above did.
        if login {
            for name in ["ORIGINAL_TMP", "ORIGINAL_TEMP", "SSH_ASKPASS"] {
                unsafe { std::env::remove_var(name) };
            }
        }
    }
    let mut line = String::from(if login { " -c -l " } else { " -c " });
    line.push_str(&cygwin_argument(&script));
    for argument in rest {
        line.push(' ');
        line.push_str(&cygwin_argument(argument));
    }
    let line: Vec<u16> = line.encode_utf16().collect();
    std::process::exit(quietdesk::forward_in(&bash, &line, place, environment.as_deref()));
}
