//! Installed as `pwsh.exe`, first on the user's PATH. Claude Code finds PowerShell by name, so its PowerShell tool starts
//! this; it starts the real pwsh with the same arguments, on the hidden desktop while QuietDesk is on. Claude Code's
//! permission checks and the command's text stay as they are, above this. If this file is ever removed, Claude Code simply
//! finds the real pwsh.

use std::path::PathBuf;

const REAL_NAME: &str = "pwsh.exe";
/// What a shell returns for a command it cannot find.
const NOT_FOUND: i32 = 127;

/// Where Claude Code (2.1.317) looks for PowerShell 7 when PATH has none, in its order. Never Windows PowerShell 5.1, which
/// Claude Code turns to last: having found this program by the name pwsh, it tells the model the session has PowerShell 7, and
/// 5.1 runs some of what is written for 7 differently.
fn where_claude_code_looks() -> Vec<PathBuf> {
    let folder = |name: &str| std::env::var_os(name).map(PathBuf::from);
    [
        folder("ProgramFiles").map(|folder| folder.join(r"PowerShell\7").join(REAL_NAME)),
        folder("LOCALAPPDATA").map(|folder| folder.join(r"Microsoft\WindowsApps").join(REAL_NAME)),
        folder("USERPROFILE").map(|folder| folder.join(r".dotnet\tools").join(REAL_NAME)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn main() {
    let own_dir = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()));
    let on_path = own_dir.and_then(|dir| quietdesk::next_on_path(REAL_NAME, &quietdesk::shim_dirs_including(&dir)));
    // The Store's pwsh is an alias with nothing to read but its link, as `next_on_path` finds it too.
    let real = on_path.or_else(|| where_claude_code_looks().into_iter().find(|file| std::fs::symlink_metadata(file).is_ok()));
    let Some(real) = real else {
        eprintln!("quietdesk: no PowerShell 7 ({REAL_NAME}) after this one, on PATH or where Claude Code looks; install it, or uninstall QuietDesk");
        std::process::exit(NOT_FOUND);
    };
    let place = quietdesk::decide_place(true);
    std::process::exit(quietdesk::forward(&real, &quietdesk::arguments_after_program(), place));
}
