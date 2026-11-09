//! The ways a command is written on its way through the Bash shim. Claude Code hands it POSIX shell text. A Windows program reads
//! its command line by Microsoft's C runtime rules, and `cmd.exe` by its own. Git Bash, started by a Windows program, reads its
//! command line by Cygwin's rules, which differ: inside double quotes it turns `\\` into `\`. Text that crosses one of these the
//! wrong way is silently changed, so each crossing is written here once and tested.

use std::path::{Path, PathBuf};

/// The words of a line that is nothing but words, such as `cmd /c echo 'it'"'"'s'`, which is how Claude Code writes an MCP
/// server's command and arguments for the prefix. None for anything a shell would act on, such as `;`, `$`, `*` or a leading
/// `NAME=`: that is a script, and only a shell can run it.
pub fn plain_words(line: &str) -> Option<Vec<String>> {
    const ALWAYS_SPECIAL: &str = "|&;<>()$`*?[]{}\n\r";
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c => word.push(c),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '$' | '`' => return None,
                        // In double quotes a backslash escapes only these; before anything else it stays.
                        '\\' => match chars.next()? {
                            next @ ('"' | '\\') => word.push(next),
                            '$' | '`' => return None,
                            '\n' => {}
                            next => {
                                word.push('\\');
                                word.push(next);
                            }
                        },
                        c => word.push(c),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.next()? {
                    '\n' => {}
                    next => word.push(next),
                }
            }
            c if ALWAYS_SPECIAL.contains(c) => return None,
            '~' | '#' if !in_word => return None,
            '=' if words.is_empty() => return None,
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    (!words.is_empty()).then_some(words)
}

/// A Windows command line that a program reading it by the C runtime's rules splits back into exactly these words. The first
/// is the program, which Windows ends at a closing quote with no escapes, so it is only quoted.
pub fn msvcrt_line(words: &[String]) -> String {
    let mut line = String::new();
    for (index, word) in words.iter().enumerate() {
        if index > 0 {
            line.push(' ');
        }
        if index == 0 {
            if word.contains([' ', '\t']) {
                line.push('"');
                line.push_str(word);
                line.push('"');
            } else {
                line.push_str(word);
            }
            continue;
        }
        if !word.is_empty() && !word.contains([' ', '\t', '\n', '\x0b', '"']) {
            line.push_str(word);
            continue;
        }
        line.push('"');
        push_doubling_backslashes_before_quotes(&mut line, word);
        line.push('"');
    }
    line
}

/// The C runtime's rule inside double quotes: backslashes are literal, except a run of them before a quote, or before the
/// closing quote, which is doubled; a quote is escaped.
fn push_doubling_backslashes_before_quotes(line: &mut String, word: &str) {
    let mut backslashes = 0;
    for c in word.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                line.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                line.push('"');
                backslashes = 0;
            }
            c => {
                line.extend(std::iter::repeat_n('\\', backslashes));
                line.push(c);
                backslashes = 0;
            }
        }
    }
    line.extend(std::iter::repeat_n('\\', backslashes * 2));
}

/// One argument as Cygwin, and so Git Bash, reads it from the command line a Windows program gave it. In double quotes Cygwin turns
/// `\\` into `\` and `\"` into `"`, and leaves a single backslash before anything else: so a run of two or more backslashes, or
/// one before a quote or the closing quote, is doubled, and a lone one, as in a Windows path, costs nothing. A text that starts
/// like a drive (`C:`) would make Cygwin keep every backslash as it is, so its first character is quoted on its own.
pub fn cygwin_argument(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 4);
    let mut rest = text;
    let mut chars = text.chars();
    if let (Some(first), Some(':')) = (chars.next(), chars.next())
        && first.is_ascii_alphabetic()
    {
        quoted.push('\'');
        quoted.push(first);
        quoted.push('\'');
        rest = &text[first.len_utf8()..];
    }
    quoted.push('"');
    let characters: Vec<char> = rest.chars().collect();
    let mut i = 0;
    while i < characters.len() {
        if characters[i] == '\\' {
            let run = characters[i..].iter().take_while(|&&c| c == '\\').count();
            let next = characters.get(i + run);
            let doubled = run >= 2 || matches!(next, None | Some('"'));
            quoted.extend(std::iter::repeat_n('\\', if doubled { run * 2 } else { run }));
            i += run;
            continue;
        }
        if characters[i] == '"' {
            quoted.push('\\');
        }
        quoted.push(characters[i]);
        i += 1;
    }
    quoted.push('"');
    quoted
}

/// The words of a command line as Git Bash reads it when a Windows program started it, for what Claude Code writes: words split at
/// white space, double-quoted parts with `\\`, `\"` and `""` unescaped, single-quoted parts as they are. Used to read the command
/// Claude Code gave Git Bash itself, before Git Bash rewrote anything.
pub fn cygwin_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut chars = line.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            return words;
        }
        let mut word = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                break;
            }
            chars.next();
            match c {
                '"' => {
                    while let Some(c) = chars.next() {
                        match c {
                            '\\' if matches!(chars.peek(), Some('\\' | '"')) => word.push(chars.next().expect("peeked")),
                            '"' if chars.peek() == Some(&'"') => word.push(chars.next().expect("peeked")),
                            '"' => break,
                            c => word.push(c),
                        }
                    }
                }
                '\'' => {
                    for c in chars.by_ref() {
                        if c == '\'' {
                            break;
                        }
                        word.push(c);
                    }
                }
                c => word.push(c),
            }
        }
        words.push(word);
    }
}

/// A Windows path as Git Bash writes it (`C:/Windows` or `C:\Windows` becomes `/c/Windows`). Git Bash turns variables that look
/// like its paths into Windows paths for a Windows program, and does not turn them all back for the bash that program starts.
pub fn msys_path(path: &str) -> Option<String> {
    let mut chars = path.chars();
    let drive = chars.next().filter(char::is_ascii_alphabetic)?;
    if chars.next() != Some(':') {
        return None;
    }
    let rest = chars.as_str();
    if !(rest.is_empty() || rest.starts_with(['/', '\\'])) {
        return None;
    }
    Some(format!("/{}{}", drive.to_ascii_lowercase(), rest.replace('\\', "/")))
}

/// What `cmd.exe` treats specially, which cross-spawn, the starter Claude Code uses, escapes with `^`.
const CMD_SPECIAL: &str = "()][%!^\"`<>&|;, *?";

fn caret_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len() * 2);
    for c in text.chars() {
        if CMD_SPECIAL.contains(c) {
            escaped.push('^');
        }
        escaped.push(c);
    }
    escaped
}

/// The `cmd.exe /d /s /c "..."` line that runs a batch file with its arguments, escaped as cross-spawn escapes it, which is how
/// Claude Code starts an MCP server such as `npx` with no prefix. npm's shims in `node_modules\.bin` read their arguments once
/// more, so cross-spawn escapes theirs twice.
pub fn cmd_line(comspec: &str, command: &str, arguments: &[String], double_escape: bool) -> String {
    let mut inner = caret_escape(command);
    for argument in arguments {
        let mut quoted = String::from("\"");
        push_doubling_backslashes_before_quotes(&mut quoted, argument);
        quoted.push('"');
        let mut escaped = caret_escape(&quoted);
        if double_escape {
            escaped = caret_escape(&escaped);
        }
        inner.push(' ');
        inner.push_str(&escaped);
    }
    format!("\"{comspec}\" /d /s /c \"{inner}\"")
}

/// The program a name stands for, found as cross-spawn finds it: a name with a folder in it from the working folder, any other in
/// the working folder and then each PATH folder, trying each PATHEXT extension, and the name as given first when it has one.
pub fn find_program(name: &str, working_folder: &Path, path: &str, path_extensions: &str) -> Option<PathBuf> {
    let extensions: Vec<&str> = path_extensions.split(';').filter(|extension| !extension.is_empty()).collect();
    let mut tries: Vec<String> = Vec::new();
    if Path::new(name).extension().is_some() {
        tries.push(name.to_string());
    }
    tries.extend(extensions.iter().map(|extension| format!("{name}{extension}")));
    let folders: Vec<PathBuf> = if name.contains(['\\', '/']) {
        vec![working_folder.to_path_buf()]
    } else {
        std::iter::once(working_folder.to_path_buf()).chain(std::env::split_paths(path)).collect()
    };
    folders.iter().flat_map(|folder| tries.iter().map(move |name| folder.join(name))).find(|candidate| candidate.is_file())
}

/// The interpreter a script names in its `#!` line, as cross-spawn reads it: `#!/usr/bin/env node` and `#!/usr/bin/node` both
/// give `node`.
pub fn shebang_interpreter(first_bytes: &[u8]) -> Option<String> {
    let line = first_bytes.strip_prefix(b"#!")?;
    let line = String::from_utf8_lossy(line.split(|&byte| byte == b'\n').next()?).trim().to_string();
    let mut parts = line.split_whitespace();
    let program = parts.next()?.rsplit(['/', '\\']).next()?.to_string();
    if program == "env" { parts.next().map(str::to_string) } else { Some(program) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::UI::Shell::CommandLineToArgvW;

    fn words(list: &[&str]) -> Option<Vec<String>> {
        Some(list.iter().map(|word| word.to_string()).collect())
    }

    /// The line Claude Code really handed the prefix for `cmd` with `/c`, `echo`, `it's a "test" \\x y` and `plain`, captured
    /// from Claude Code 2.1.317.
    #[test]
    fn claude_codes_mcp_line_reads_back_as_its_words() {
        let line = r#"cmd /c echo 'it'"'"'s a "test" \\x y' plain"#;
        assert_eq!(plain_words(line), words(&["cmd", "/c", "echo", r#"it's a "test" \\x y"#, "plain"]));
    }

    #[test]
    fn quoting_and_escapes_read_as_a_shell_reads_them() {
        assert_eq!(plain_words(r#"a\ b "c\"d\\e\x" 'f\g' --x=1"#), words(&["a b", r#"c"d\e\x"#, r"f\g", "--x=1"]));
        assert_eq!(plain_words("''"), words(&[""]));
        assert_eq!(plain_words("  spaced\tout  "), words(&["spaced", "out"]));
    }

    /// Anything a shell would act on makes it a script, which only a shell may run.
    #[test]
    fn a_script_is_not_words() {
        for line in [
            "a; b", "a && b", "a | b", "a > f", "echo $HOME", "echo \"$HOME\"", "ls *.txt", "echo `x`", "(a)", "{ a; }",
            "FOO=1 cmd", "~/x", "# note", "a\nb", "echo 'open", "echo \"open", "",
        ] {
            assert_eq!(plain_words(line), None, "{line:?}");
        }
        assert_eq!(plain_words("cmd a#b c~d"), words(&["cmd", "a#b", "c~d"]), "# and ~ only count at a word's start");
    }

    /// Split back by Windows itself (`CommandLineToArgvW`), not by a second copy of the same rules.
    #[test]
    fn an_msvcrt_line_splits_back_into_the_same_words() {
        let cases: Vec<Vec<String>> = vec![
            vec!["C:\\Program Files\\x.exe".into(), "a b".into(), r#"say "hi""#.into(), r"trail\".into(), r"\\srv\share".into(), "".into()],
            vec!["cmd".into(), "/c".into(), r#"echo "x" \"y\" \\"#.into(), "tab\there".into()],
        ];
        for words in cases {
            let line = msvcrt_line(&words);
            let wide: Vec<u16> = line.encode_utf16().chain(std::iter::once(0)).collect();
            let mut count = 0;
            let split = unsafe {
                let list = CommandLineToArgvW(wide.as_ptr(), &mut count);
                assert!(!list.is_null());
                let split: Vec<String> = (0..count as usize)
                    .map(|index| {
                        let word = *list.add(index);
                        let length = (0..).take_while(|&at| *word.add(at) != 0).count();
                        String::from_utf16_lossy(std::slice::from_raw_parts(word, length))
                    })
                    .collect();
                LocalFree(list.cast());
                split
            };
            assert_eq!(split, words, "{line}");
        }
    }

    /// The command line Claude Code 2.1.317 gives Git Bash for the Bash tool with the prefix, in Bun's quoting (captured shape).
    #[test]
    fn git_bashs_own_command_line_reads_back() {
        let line = r#""C:\Program Files\Git\usr\bin\bash.exe" -c -l "'C:\Users\x\quietdesk-bash.exe' 'printf '\''%s'\'' \"a\\\\b\"'""#;
        assert_eq!(
            cygwin_words(line),
            ["C:\\Program Files\\Git\\usr\\bin\\bash.exe", "-c", "-l", r#"'C:\Users\x\quietdesk-bash.exe' 'printf '\''%s'\'' "a\\b"'"#]
        );
    }

    /// cross-spawn's own output for these (cross-spawn 7.0.6, `lib/util/escape.js`), checked by hand.
    #[test]
    fn a_batch_file_line_is_escaped_as_cross_spawn_escapes_it() {
        let line = cmd_line(r"C:\WINDOWS\system32\cmd.exe", "npx", &["-y".into(), "@scope/pkg".into(), "a b&c".into(), r"x\".into()], false);
        assert_eq!(line, r#""C:\WINDOWS\system32\cmd.exe" /d /s /c "npx ^"-y^" ^"@scope/pkg^" ^"a^ b^&c^" ^"x\\^"""#);
        let twice = cmd_line("cmd.exe", "x", &["a&b".into()], true);
        assert_eq!(twice, r#""cmd.exe" /d /s /c "x ^^^"a^^^&b^^^"""#);
    }

    #[test]
    fn programs_are_found_with_their_extension_from_the_working_folder_then_path() {
        let root = std::env::temp_dir().join(format!("quietdesk-find-{}", std::process::id()));
        let (here, on_path) = (root.join("here"), root.join("path"));
        std::fs::create_dir_all(&here).unwrap();
        std::fs::create_dir_all(&on_path).unwrap();
        std::fs::write(on_path.join("tool.cmd"), "").unwrap();
        std::fs::write(on_path.join("both.exe"), "").unwrap();
        std::fs::write(here.join("both.cmd"), "").unwrap();
        let path = on_path.to_string_lossy().to_string();
        // Windows names are not case-sensitive: the extension comes back as PATHEXT writes it.
        let found = |name: &str| find_program(name, &here, &path, ".COM;.EXE;.BAT;.CMD").map(|file| PathBuf::from(file.to_string_lossy().to_lowercase()));
        let (here, on_path) = (PathBuf::from(here.to_string_lossy().to_lowercase()), PathBuf::from(on_path.to_string_lossy().to_lowercase()));
        assert_eq!(found("tool"), Some(on_path.join("tool.cmd")), "a batch file, by PATHEXT");
        assert_eq!(found("both"), Some(here.join("both.cmd")), "the working folder first");
        assert_eq!(found("tool.cmd"), Some(on_path.join("tool.cmd")), "a name with its extension");
        assert_eq!(found("missing"), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_scripts_interpreter_is_read_from_its_first_line() {
        assert_eq!(shebang_interpreter(b"#!/usr/bin/env node\nrest").as_deref(), Some("node"));
        assert_eq!(shebang_interpreter(b"#!/usr/bin/python3 -u\n").as_deref(), Some("python3"));
        assert_eq!(shebang_interpreter(b"MZ\x90\x00"), None);
    }

    #[test]
    fn windows_paths_become_git_bash_paths_and_nothing_else_does() {
        assert_eq!(msys_path("C:/Windows").as_deref(), Some("/c/Windows"));
        assert_eq!(msys_path(r"D:\Games\x").as_deref(), Some("/d/Games/x"));
        assert_eq!(msys_path("C:").as_deref(), Some("/c"));
        for other in ["/c/Windows", "relative", r"\\server\share", "C:relative", "1:/x", ""] {
            assert_eq!(msys_path(other), None, "{other}");
        }
    }
}
