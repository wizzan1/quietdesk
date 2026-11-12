//! The paths Claude Code takes, against the built programs: where a command lands, and that nothing about it changes.
//! No window reaches the screen: the only window here opens on the hidden desktop.

use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

const PWSH: &str = env!("CARGO_BIN_EXE_pwsh");
const QUIETDESK_BASH: &str = env!("CARGO_BIN_EXE_quietdesk-bash");
const QUIETDESK: &str = env!("CARGO_BIN_EXE_quietdesk");
const PROBE: &str = env!("CARGO_BIN_EXE_quietdesk-probe");
const HIDDEN: &str = "desktop=QuietDesk";
/// Every command here starts from the real desktop, whatever desktop the test run itself is on.
const REAL: &str = "desktop=Default";

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A throwaway install: the shim in one folder, a probe as the "real" pwsh in the next one on PATH, its own mode file, and its own
/// copy of the console setting: the shims set it on every command, and a test must never touch the user's.
struct Setup {
    root: PathBuf,
    shim: PathBuf,
    path: String,
    console_key: String,
}

impl Setup {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("quietdesk-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
        let (shim_dir, real_dir) = (root.join("shim"), root.join("real"));
        std::fs::create_dir_all(&shim_dir).unwrap();
        std::fs::create_dir_all(&real_dir).unwrap();
        let shim = shim_dir.join("pwsh.exe");
        std::fs::copy(PWSH, &shim).unwrap();
        std::fs::copy(PROBE, real_dir.join("pwsh.exe")).unwrap();
        let system = PathBuf::from(std::env::var("SystemRoot").unwrap());
        let path = format!("{};{};{};{}", shim_dir.display(), real_dir.display(), system.join("System32").display(), system.display());
        let console_key = format!(r"Software\QuietDeskTest\{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst));
        Self { root, shim, path, console_key }
    }

    fn mode(&self, word: &str) {
        std::fs::write(self.root.join("mode"), word).unwrap();
    }

    /// As Claude Code starts a command: `CLAUDECODE` set, nothing decided yet, whatever the shell running the tests has. It starts
    /// from the real desktop, through the probe's launcher: from a test run that is itself hidden, as under QuietDesk, a command that
    /// stays where it started would look hidden too, and every check of where a command ran would pass for nothing.
    fn command(&self, program: &Path) -> Command {
        let mut command = Command::new(PROBE);
        command.arg(program).env("QUIETDESK_PROBE_LAUNCH", "1");
        command.env("QUIETDESK_DIR", &self.root).env("PATH", &self.path).env("CLAUDECODE", "1").env_remove("QUIETDESK_DECIDED").env_remove("QUIETDESK_SHIM_DIRS");
        command.env("QUIETDESK_CONSOLE_KEY", &self.console_key);
        command
    }

    /// Where Claude Code looks for PowerShell 7 off PATH, made empty as far as a test can: Windows gives every program the real
    /// Program Files folder, whatever its parent says.
    fn nowhere_else(&self, command: &mut Command) {
        for name in ["LOCALAPPDATA", "USERPROFILE"] {
            command.env(name, self.root.join("nowhere"));
        }
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = Command::new("reg").args(["delete", &format!(r"HKCU\{}", self.console_key), "/f"]).output();
    }
}

fn text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn line<'a>(output: &'a str, name: &str) -> &'a str {
    output.lines().find(|l| l.starts_with(name)).unwrap_or_else(|| panic!("no {name} line in: {output}"))
}

/// Without the error mode the shims set, Windows shows a dialog on the real screen and the program waits for its answer, so
/// this fails on the timeout. It then leaves that dialog open, naming the program.
#[test]
fn a_program_that_cannot_start_ends_with_its_error_and_asks_nobody() {
    const STATUS_DLL_NOT_FOUND: u32 = 0xC000_0135;
    const IMPORT: &[u8] = b"kernel32.dll";
    let setup = Setup::new();
    setup.mode("on");
    let mut program = std::fs::read(PROBE).unwrap();
    // Every mention, in whichever case: one of them is the import.
    let mentions: Vec<usize> = (0..program.len() - IMPORT.len()).filter(|&at| program[at..at + IMPORT.len()].eq_ignore_ascii_case(IMPORT)).collect();
    assert!(!mentions.is_empty(), "the probe imports this DLL");
    for at in mentions {
        program[at] = b'z';
    }
    std::fs::write(setup.root.join("real").join("pwsh.exe"), program).unwrap();
    let mut child = setup.command(&setup.shim).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            panic!("the program waited for an answer to a dialog");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status.code().map(|code| code as u32), Some(STATUS_DLL_NOT_FOUND));
}

/// `quietdesk status | head` closes the pipe while it still prints. `println!` panics on that, which reads like a broken install.
#[test]
fn a_reader_that_stops_listening_is_no_failure() {
    let setup = Setup::new();
    let mut child = setup.command(Path::new(QUIETDESK)).arg("status").stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    drop(child.stdout.take());
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn hidden_only_while_the_mode_is_on() {
    let setup = Setup::new();
    let run = || text(&setup.command(&setup.shim).output().unwrap());
    setup.mode("on");
    assert_eq!(line(&run(), "desktop="), HIDDEN);
    assert_eq!(line(&run(), "decided="), "decided=1");
    setup.mode("off");
    assert_eq!(line(&run(), "desktop="), REAL);
    assert_eq!(line(&run(), "decided="), "decided=1", "a run started visible stays visible when the mode flips");
    setup.mode("on");
    assert_eq!(line(&run(), "desktop="), HIDDEN, "a flip reaches the very next command");
}

#[test]
fn no_mode_file_or_an_unknown_word_means_off() {
    let setup = Setup::new();
    assert_eq!(line(&text(&setup.command(&setup.shim).output().unwrap()), "desktop="), REAL);
    setup.mode("maybe");
    assert_eq!(line(&text(&setup.command(&setup.shim).output().unwrap()), "desktop="), REAL);
}

#[test]
fn the_users_own_powershell_is_left_alone() {
    let setup = Setup::new();
    setup.mode("on");
    let output = text(&setup.command(&setup.shim).env_remove("CLAUDECODE").output().unwrap());
    assert_eq!(line(&output, "desktop="), REAL);
    assert_eq!(line(&output, "decided="), "decided=");
}

#[test]
fn a_command_inside_a_run_stays_where_the_run_started() {
    let setup = Setup::new();
    setup.mode("on");
    let output = text(&setup.command(&setup.shim).env("QUIETDESK_DECIDED", "1").output().unwrap());
    assert_eq!(line(&output, "desktop="), REAL);
}

#[test]
fn arguments_reach_the_real_program_byte_for_byte() {
    let setup = Setup::new();
    setup.mode("on");
    // The shapes Claude Code really sends, and what would break a re-quoting shim.
    for raw in [
        r#"-NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$__claudeCodeScript = $env:X; Invoke-Expression -Command $__claudeCodeScript""#,
        r#"  two  spaces   "a b" 'c d' ^"e^" %PATH% !x! \"f\" C:\g\ "h\\" "#,
        "unicode åäö ₿ 日本",
        r#"-EncodedCommand AAAA== """triple""""#,
    ] {
        let output = text(&setup.command(&setup.shim).raw_arg(raw).output().unwrap());
        let received = line(&output, "args=").trim_start_matches("args=");
        assert_eq!(received.trim_start(), raw.trim_start(), "for {raw}");
    }
}

#[test]
fn exit_code_and_all_three_streams_carry_through() {
    let setup = Setup::new();
    setup.mode("on");
    for code in [0, 7, 255] {
        let status = setup.command(&setup.shim).env("QUIETDESK_PROBE_EXIT", code.to_string()).output().unwrap().status;
        assert_eq!(status.code(), Some(code));
    }
    let mut child = setup
        .command(&setup.shim)
        .env("QUIETDESK_PROBE_READ_STDIN", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.as_mut().unwrap().write_all(b"from stdin\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(text(&output).contains("stdin=from stdin"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("probe-stderr"));
}

/// What the shim says when PowerShell 7 is nowhere: on PATH after it or where Claude Code looks. A PC with PowerShell 7 in Program
/// Files has it there for the shim too, as for Claude Code, and the run then ends there.
fn ends_in_program_files_or_says_none(output: &Output) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let installed = Path::new(&std::env::var("ProgramFiles").unwrap()).join(r"PowerShell\7\pwsh.exe").is_file();
    if installed {
        assert_eq!(output.status.code(), Some(6), "PowerShell 7 in Program Files did not run: {stderr}");
    } else {
        assert_eq!(output.status.code(), Some(127), "{stderr}");
        assert!(stderr.contains("no PowerShell 7"), "{stderr}");
    }
}

const EXIT_6: [&str; 4] = ["-NoProfile", "-NonInteractive", "-Command", "exit 6"];

#[test]
fn a_shim_alone_on_path_says_so_instead_of_starting_itself() {
    let setup = Setup::new();
    let mut command = setup.command(&setup.shim);
    setup.nowhere_else(command.env("PATH", setup.shim.parent().unwrap()));
    ends_in_program_files_or_says_none(&command.args(EXIT_6).output().unwrap());
}

/// With no PowerShell 7 on PATH after it, the shim looks where Claude Code itself looks next, in its order, as Claude Code would with
/// no shim: Program Files, the Store's alias, then .NET's tools, where a stand-in waits here.
#[test]
fn off_path_the_shim_finds_powershell_where_claude_code_looks() {
    let setup = Setup::new();
    let profile = setup.root.join("profile");
    std::fs::create_dir_all(profile.join(r".dotnet\tools")).unwrap();
    std::fs::copy(PROBE, profile.join(r".dotnet\tools\pwsh.exe")).unwrap();
    let mut command = setup.command(&setup.shim);
    setup.nowhere_else(command.env("PATH", setup.shim.parent().unwrap()));
    let output = command.env("USERPROFILE", &profile).env("QUIETDESK_PROBE_EXIT", "6").args(EXIT_6).output().unwrap();
    assert_eq!(output.status.code(), Some(6), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let installed = Path::new(&std::env::var("ProgramFiles").unwrap()).join(r"PowerShell\7\pwsh.exe").is_file();
    assert_eq!(text(&output).contains("argv=exit 6"), !installed, "{}", text(&output));
}

/// A folder with another copy of the shim, in front of the setup's own on PATH.
fn second_shim_dir(setup: &Setup) -> PathBuf {
    let dir = setup.root.join("second-shim");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(PWSH, dir.join("pwsh.exe")).unwrap();
    dir
}

/// Without the guard these two start each other without end: that happened once, 6252 processes in four minutes.
#[test]
fn two_shims_on_path_end_at_the_real_program() {
    let setup = Setup::new();
    let second = second_shim_dir(&setup);
    let path = format!("{};{}", second.display(), setup.path);
    let output = setup.command(&second.join("pwsh.exe")).env("PATH", path).arg("reached").output().unwrap();
    assert_eq!(output.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert!(line(&text(&output), "args=").ends_with("reached"));
}

#[test]
fn two_shims_and_no_real_program_is_an_error_not_a_loop() {
    let setup = Setup::new();
    let second = second_shim_dir(&setup);
    let path = format!("{};{}", second.display(), setup.shim.parent().unwrap().display());
    let mut command = setup.command(&second.join("pwsh.exe"));
    setup.nowhere_else(command.env("PATH", path));
    ends_in_program_files_or_says_none(&command.args(EXIT_6).output().unwrap());
}

/// The folder of the installed PowerShell: PATH during `cargo test` also holds this build's own shims, in `target`, and
/// the installed shim when QuietDesk is installed.
fn real_pwsh_dir() -> PathBuf {
    let target = Path::new(PWSH).ancestors().nth(2).unwrap().to_string_lossy().to_lowercase();
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .find(|dir| {
            let name = dir.to_string_lossy().to_lowercase();
            !name.starts_with(&target) && !name.contains(r"\quietdesk\bin") && std::fs::symlink_metadata(dir.join("pwsh.exe")).is_ok()
        })
        .expect("this PC has PowerShell 7 on PATH: the test needs it")
}

/// The Store's pwsh is an app execution alias: this fails if the search stops seeing it.
#[test]
fn the_real_powershell_runs_through_the_shim() {
    let setup = Setup::new();
    setup.mode("on");
    let path = format!("{};{}", setup.shim.parent().unwrap().display(), real_pwsh_dir().display());
    let output = setup
        .command(&setup.shim)
        .env("PATH", path)
        .args(["-NoProfile", "-NonInteractive", "-Command", "Write-Output through-the-shim; exit 5"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(5), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert!(text(&output).contains("through-the-shim"));
}

fn git_bash() -> PathBuf {
    std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("ProgramFiles").map(|dir| PathBuf::from(dir).join(r"Git\bin\bash.exe")))
        .filter(|path| path.is_file())
        .expect("Git Bash is installed: the Bash shim needs it")
}

#[derive(Debug, PartialEq)]
struct Ran {
    out: String,
    err: String,
    code: Option<i32>,
}

/// How Claude Code 2.1.317 quotes a word for bash: in single quotes, a single quote as `'"'"'`.
fn quoted(word: &str) -> String {
    format!("'{}'", word.replace('\'', r#"'"'"'"#))
}

/// A script as Claude Code's Bash tool and its hooks run it: Git Bash started by a Windows program, `bash -c -l <script>` with no
/// prefix and `bash -c -l "'<prefix>' '<script>'"` with this one. Both runs, with no prefix first.
fn as_claude_code_runs(setup: &Setup, script: &str) -> [Ran; 2] {
    as_claude_code_runs_with(setup, script, None)
}

/// The same, with `PWD` in the environment Claude Code gives Git Bash, as when Claude Code itself was started from Git Bash.
fn as_claude_code_runs_with(setup: &Setup, script: &str, pwd: Option<&Path>) -> [Ran; 2] {
    [script.to_string(), format!("{} {}", quoted(QUIETDESK_BASH), quoted(script))].map(|line| {
        let mut command = setup.command(&git_bash());
        match pwd {
            Some(folder) => command.current_dir(folder).env("PWD", folder.to_string_lossy().replace('\\', "/")),
            None => command.env_remove("PWD"),
        };
        let output = command.args(["-c", "-l", &line]).output().unwrap();
        Ran { out: text(&output), err: String::from_utf8_lossy(&output.stderr).into_owned(), code: output.status.code() }
    })
}

/// Backslashes, quotes, the working folder, output, errors and exit codes, a hook that starts with a path of Git Bash's own, a
/// program named by its drive, a long script, and the whole environment and shell state: through the shim exactly as with no shim.
/// A raw hand-over to bash halved every doubled backslash once more and left `$PWD` as a Windows path, and a script from the
/// shim's own arguments started `/usr/bin/env` as `C:/Program Files/Git/usr/bin/env`.
#[test]
fn bash_scripts_reach_bash_exactly_as_git_bash_hands_them_over() {
    let setup = Setup::new();
    let long = format!(r#"printf '%s' "{}" | wc -c"#, r"C:/Windows/System32\\drivers\etc\hosts ".repeat(160));
    // Bash commands longer than about 8,000 characters fail in Git Bash started this way, with or without a prefix.
    assert!(long.len() > 6000 && quoted(&long).len() + QUIETDESK_BASH.len() < 7500, "{}", long.len());
    let scripts = [
        r#"printf '[%s]\n' 'a\\b' 'c\\\\d' "e\\\\f" '\\srv\share' 'trail\'"#.to_string(),
        r#"printf '[%s]\n' "q\"uote" 'it'\''s' "$((1+2))" 'åäö ✓' $'tab\there'"#.to_string(),
        "cat <<'EOF'\nx\\\\y \\\\\\\\z \"q\" 'r' $HOME\nEOF".to_string(),
        r#"printf '[%s]\n' "$PWD" "$OLDPWD"; case $PWD in /*) echo posix ;; *) echo windows ;; esac"#.to_string(),
        "echo out; echo err >&2; exit 7".to_string(),
        r#"/usr/bin/echo hook-ran; /usr/bin/env printf '[%s]\n' /tmp"#.to_string(),
        "C:/Windows/System32/cmd.exe /d /c echo drive-start".to_string(),
        long,
        // The shell one level deeper, the last command, and QuietDesk's own variables are all that may differ.
        concat!(
            "shopt -q login_shell && echo login-shell; shopt | LC_ALL=C sort; set -o | LC_ALL=C sort; ",
            r#"env | LC_ALL=C sort | grep -v -E '^(_|SHLVL|QUIETDESK_[A-Z_]+)='"#
        )
        .to_string(),
    ];
    let here = std::env::current_dir().unwrap();
    for pwd in [None, Some(here.as_path())] {
        for script in &scripts {
            let [direct, through] = as_claude_code_runs_with(&setup, script, pwd);
            assert!(!direct.out.is_empty() || direct.code != Some(0), "the script did nothing: {script}");
            assert_eq!(through, direct, "through the shim, then with none, PWD {pwd:?}: {script}");
        }
    }
}

#[test]
fn bash_scripts_run_hidden_with_the_mode_on_and_in_place_with_it_off() {
    let setup = Setup::new();
    let script = format!("'{}' desktop", QUIETDESK.replace('\\', "/"));
    setup.mode("on");
    assert_eq!(as_claude_code_runs(&setup, &script)[1].out, "QuietDesk\n");
    setup.mode("off");
    assert_eq!(format!("desktop={}", as_claude_code_runs(&setup, &script)[1].out.trim_end()), REAL);
}

/// Claude Code starts an MCP server through the prefix itself, with the command and arguments as one line of shell words: this is
/// the line Claude Code 2.1.317 really made (captured). The server must get each argument exactly; through bash, `/c` became `C:/`.
#[test]
fn an_mcp_server_starts_as_claude_code_would_start_it_without_the_prefix() {
    let setup = Setup::new();
    for mode in ["off", "on"] {
        setup.mode(mode);
        let server = format!(r#"'{PROBE}' /c 'it'"'"'s a "test" \\x y' plain ''"#);
        let output = text(&setup.command(Path::new(QUIETDESK_BASH)).arg(&server).output().unwrap());
        let arguments: Vec<&str> = output.lines().filter_map(|row| row.strip_prefix("argv=")).collect();
        assert_eq!(arguments, ["/c", r#"it's a "test" \\x y"#, "plain", ""], "mode {mode}: {output}");
        let desktop = if mode == "on" { HIDDEN } else { REAL };
        assert_eq!(line(&output, "desktop="), desktop);
    }
    // The form Claude Code's documentation gives for an MCP server on Windows.
    let cmd = setup.command(Path::new(QUIETDESK_BASH)).arg("cmd /c echo mcp-started").stdin(Stdio::null()).output().unwrap();
    assert_eq!(text(&cmd).trim(), "mcp-started", "stderr: {}", String::from_utf8_lossy(&cmd.stderr));
}

/// An MCP server that is a batch file, as `npx` is: found by PATHEXT and run through cmd.exe with cross-spawn's escaping, so it
/// gets its arguments as it would with no prefix. Started by CreateProcess alone, it is not found at all.
#[test]
fn an_mcp_server_that_is_a_batch_file_starts_as_npx_does() {
    let setup = Setup::new();
    let tools = setup.root.join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    std::fs::write(tools.join("tool.cmd"), format!("@\"{PROBE}\" %*\r\n")).unwrap();
    let path = format!("{};{}", tools.display(), setup.path);
    for mode in ["off", "on"] {
        setup.mode(mode);
        let output = setup.command(Path::new(QUIETDESK_BASH)).env("PATH", &path).arg("tool -y '@scope/pkg' 'a b&c'").output().unwrap();
        assert_eq!(output.status.code(), Some(0), "mode {mode}: {}", String::from_utf8_lossy(&output.stderr));
        let output = text(&output);
        let arguments: Vec<&str> = output.lines().filter_map(|row| row.strip_prefix("argv=")).collect();
        assert_eq!(arguments, ["-y", "@scope/pkg", "a b&c"], "mode {mode}: {output}");
        assert_eq!(line(&output, "desktop="), if mode == "on" { HIDDEN } else { REAL });
    }
}

#[test]
fn one_command_can_go_to_the_real_screen_or_stay_hidden() {
    let setup = Setup::new();
    let output = text(&setup.command(Path::new(QUIETDESK)).args(["hidden", PROBE, "x", "y z"]).output().unwrap());
    assert_eq!(line(&output, "desktop="), HIDDEN);
    assert!(line(&output, "args=").ends_with(r#"x "y z""#), "{output}");

    let output = text(&setup.command(Path::new(QUIETDESK)).args(["hidden", QUIETDESK, "visible", PROBE]).output().unwrap());
    assert_eq!(line(&output, "desktop="), "desktop=Default", "from hidden back to the real screen");
    assert_eq!(line(&output, "decided="), "decided=1");

    let code = setup.command(Path::new(QUIETDESK)).args(["hidden", PROBE]).env("QUIETDESK_PROBE_EXIT", "9").output().unwrap().status.code();
    assert_eq!(code, Some(9));
    let missing = setup.command(Path::new(QUIETDESK)).args(["visible", "no-such-program-here.exe"]).output().unwrap();
    assert_eq!(missing.status.code(), Some(127));
}

fn registry_value(key: &str, name: &str) -> Option<String> {
    let output = Command::new("reg").args(["query", &format!(r"HKCU\{key}"), "/v", name]).output().unwrap();
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).split_whitespace().last().unwrap().to_string())
}

#[test]
fn the_switch_keeps_the_users_console_choice_and_gives_it_back() {
    let setup = Setup::new();
    let key = setup.console_key.clone();
    let conhost = "{B23D10C0-E52E-411E-9D5B-C09FDF709C7D}";
    let terminal = "{USERS-OWN-CHOICE}";
    let status = Command::new("reg").args(["add", &format!(r"HKCU\{key}"), "/v", "DelegationTerminal", "/d", terminal, "/f"]).output().unwrap().status;
    assert!(status.success());
    let flip = |word: &str| {
        let output = setup.command(Path::new(QUIETDESK)).arg(word).env("QUIETDESK_CONSOLE_KEY", &key).output().unwrap();
        assert!(output.status.success(), "quietdesk {word}: {}", String::from_utf8_lossy(&output.stderr));
    };

    flip("on");
    assert_eq!(registry_value(&key, "DelegationTerminal").as_deref(), Some(conhost));
    assert_eq!(registry_value(&key, "DelegationConsole").as_deref(), Some(conhost));
    assert_eq!(std::fs::read_to_string(setup.root.join("mode")).unwrap().trim(), "on");
    flip("on"); // twice must not overwrite what was saved with our own value

    flip("off");
    assert_eq!(registry_value(&key, "DelegationTerminal").as_deref(), Some(terminal));
    assert_eq!(registry_value(&key, "DelegationConsole"), None, "a value that was not there is removed again");
    assert_eq!(std::fs::read_to_string(setup.root.join("mode")).unwrap().trim(), "off");
    assert!(!registry_key_exists(&format!(r"{key}\QuietDeskBefore")), "the backup stayed behind");
}

fn registry_key_exists(key: &str) -> bool {
    Command::new("reg").args(["query", &format!(r"HKCU\{key}")]).output().unwrap().status.success()
}

/// The Claude app is a packaged app: Windows gives what it starts a copy of the console setting of its own, and the new consoles of
/// those commands follow that copy. The switch set only the copy it ran in, so with the mode on, a test's new console from the Bash
/// tool handed over to Windows Terminal, on the owner's screen during their game. Here a copy the switch never touched holds the
/// user's own Windows Terminal: the next command's shim sets the classic console in it, and with the mode off gives theirs back.
#[test]
fn a_copy_of_the_console_setting_the_switch_never_saw_follows_the_mode_from_the_next_command() {
    let setup = Setup::new();
    let (conhost, terminal) = ("{B23D10C0-E52E-411E-9D5B-C09FDF709C7D}", "{2EACA947-7F5F-4CFA-BA87-8F7FBEEFBE69}");
    let key = setup.console_key.clone();
    for name in ["DelegationConsole", "DelegationTerminal"] {
        assert!(Command::new("reg").args(["add", &format!(r"HKCU\{key}"), "/v", name, "/d", terminal, "/f"]).output().unwrap().status.success());
    }
    // As the switch does where it runs, but nowhere near this copy: only the mode file says on.
    setup.mode("on");
    let output = setup.command(&setup.shim).output().unwrap();
    assert_eq!(line(&text(&output), "desktop="), HIDDEN);
    assert_eq!(registry_value(&key, "DelegationConsole").as_deref(), Some(conhost), "the command's copy still hands new consoles to the Terminal");
    assert_eq!(registry_value(&key, "DelegationTerminal").as_deref(), Some(conhost));
    assert_eq!(registry_value(&format!(r"{key}\QuietDeskBefore"), "DelegationConsole").as_deref(), Some(terminal), "the user's choice was not kept");
    // A second command finds it set, and keeps the user's choice as it was.
    setup.command(&setup.shim).output().unwrap();
    assert_eq!(registry_value(&format!(r"{key}\QuietDeskBefore"), "DelegationTerminal").as_deref(), Some(terminal), "the backup was overwritten");

    setup.mode("off");
    let output = setup.command(&setup.shim).output().unwrap();
    assert_eq!(line(&text(&output), "desktop="), REAL);
    assert_eq!(registry_value(&key, "DelegationConsole").as_deref(), Some(terminal), "the user's own choice did not come back");
    assert_eq!(registry_value(&key, "DelegationTerminal").as_deref(), Some(terminal));
    assert!(!registry_key_exists(&format!(r"{key}\QuietDeskBefore")), "the backup stayed behind");
}

const TEST_APP: &str = r#"
param([string]$Title)
Add-Type -AssemblyName System.Windows.Forms
$form = New-Object System.Windows.Forms.Form
$form.Text = $Title
$form.Width = 520; $form.Height = 220
$box = New-Object System.Windows.Forms.TextBox
$box.Left = 20; $box.Top = 20; $box.Width = 460
$button = New-Object System.Windows.Forms.Button
$button.Text = 'Press me'; $button.Left = 20; $button.Top = 60; $button.Width = 140; $button.Height = 34
$label = New-Object System.Windows.Forms.Label
$label.Text = 'not pressed yet'; $label.Left = 20; $label.Top = 110; $label.Width = 460
$button.Add_Click({ $label.Text = "pressed: $($box.Text)" })
$form.Controls.AddRange(@($box, $button, $label))
[void]$form.ShowDialog()
"#;

fn field<'a>(listing: &'a str, marker: &str) -> &'a str {
    listing
        .lines()
        .find(|l| l.contains(marker))
        .unwrap_or_else(|| panic!("no {marker} in: {listing}"))
        .split_whitespace()
        .next()
        .unwrap()
}

/// Asks the test window to close and stops its starter, however the test ends: a window left behind would hold the
/// output pipe of the whole test run open.
struct RunningApp<'a> {
    setup: &'a Setup,
    title: String,
    child: std::process::Child,
}

impl Drop for RunningApp<'_> {
    fn drop(&mut self) {
        let _ = self.setup.command(Path::new(QUIETDESK)).args(["close", &self.title]).output();
        let _ = self.child.kill();
    }
}

/// See, read and talk, with no mouse, no keyboard and no focus, on a real window.
#[test]
fn a_hidden_window_can_be_seen_read_and_operated() {
    let setup = Setup::new();
    let title = format!("QuietDeskTestWindow{}", std::process::id());
    let script = setup.root.join("app.ps1");
    std::fs::write(&script, TEST_APP).unwrap();
    let powershell = format!(r"{}\System32\WindowsPowerShell\v1.0\powershell.exe", std::env::var("SystemRoot").unwrap());
    let child = setup
        .command(Path::new(QUIETDESK))
        .args(["hidden", &powershell, "-NoProfile", "-STA", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .arg(&title)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut app = RunningApp { setup: &setup, title: title.clone(), child };

    let quietdesk = |words: &[&str]| setup.command(Path::new(QUIETDESK)).args(words).output().unwrap();
    let started = Instant::now();
    // A form is listed as soon as it shows; its controls follow a moment later.
    let listing = loop {
        let listing = text(&quietdesk(&["controls", &title]));
        if listing.contains("not pressed yet") {
            break listing;
        }
        assert!(started.elapsed() < Duration::from_secs(30), "the window or its controls never appeared on the hidden desktop");
        std::thread::sleep(Duration::from_millis(200));
    };
    assert!(text(&quietdesk(&["windows"])).contains(&title));

    let (edit, button, label) = (field(&listing, ".EDIT."), field(&listing, "Press me"), field(&listing, "not pressed yet"));
    assert!(quietdesk(&["settext", edit, "typed by Claude"]).status.success());
    assert!(quietdesk(&["press", button]).status.success());
    let mut listing = String::new();
    while !listing.contains("pressed: typed by Claude") {
        assert!(started.elapsed() < Duration::from_secs(30), "the button press had no effect: {listing}");
        std::thread::sleep(Duration::from_millis(100));
        listing = text(&quietdesk(&["controls", &title]));
    }
    assert_eq!(field(&listing, "pressed: typed by Claude"), label);

    let picture = setup.root.join("window.png");
    let shot = quietdesk(&["shot", &title, picture.to_str().unwrap()]);
    assert_eq!(shot.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&shot.stderr));
    assert_eq!(&std::fs::read(&picture).unwrap()[..4], b"\x89PNG");

    assert!(quietdesk(&["close", &title]).status.success());
    let status = loop {
        if let Some(status) = app.child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(45) {
            panic!("the window did not close when asked");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(status.code(), Some(0));
}

#[test]
fn a_name_never_matches_a_window_on_the_real_screen() {
    let setup = Setup::new();
    // Explorer's taskbar is on the real desktop on every PC, and a name must not reach it.
    let output = setup.command(Path::new(QUIETDESK)).args(["close", "Shell_TrayWnd"]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no window out of sight matches"));
}
