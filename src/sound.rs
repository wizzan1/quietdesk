//! Keeps test runs silent: mutes the audio of programs running out of sight, and only those, with Windows' own per-program
//! mute. The user's game and apps are never touched.
//!
//! Windows remembers that mute per program file, as it stood when the session closed, and ignores an unmute sent after the
//! process has ended (measured: the next launch started muted). So a test run of a game would leave the user's own launch
//! of it silent. Each muted program is therefore written down before its mute and stays on the list after the run: when it
//! next plays outside a test, muted, the guard gives it its sound back within a tick. This also covers a crash of the guard.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;
use std::ptr::null;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use windows::Win32::Media::Audio::{
    DEVICE_STATE_ACTIVE, IAudioSessionControl, IAudioSessionControl2, IAudioSessionManager2, IAudioSessionNotification,
    IAudioSessionNotification_Impl, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator, eRender,
};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree};
use windows::core::{GUID, Interface, Ref, implement};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};

use crate::{Desktop, desktop_is_hidden, monitor, note_problem, process_path, state_dir};

/// How often the guard looks even when no new sound was announced: for a program on the monitor whose sound began before it was
/// known to be a test's, and for outputs that came or went. New sound wakes it at once.
pub const GUARD_TICK: Duration = Duration::from_millis(200);
/// A process's ancestors, as far as a test's audio helper can be from its window.
const MOST_ANCESTORS: usize = 16;
/// How many ticks the outputs are kept between looking for new ones: opening them costs more than the rest of a tick together.
const TICKS_PER_OUTPUTS_LOOK: u32 = 25;
const SETTING_FILE: &str = "mute-test-sounds";
const LIST_FILE: &str = "muted-programs";

fn setting_file() -> PathBuf {
    state_dir().join(SETTING_FILE)
}

fn list_file() -> PathBuf {
    state_dir().join(LIST_FILE)
}

/// On unless the user turned it off.
pub fn muting_enabled() -> bool {
    !matches!(std::fs::read_to_string(setting_file()), Ok(text) if text.trim() == "off")
}

pub fn set_muting(enabled: bool) -> io::Result<()> {
    std::fs::create_dir_all(state_dir())?;
    std::fs::write(setting_file(), if enabled { "on\n" } else { "off\n" })
}

/// The processes of what runs on the virtual monitor, which to Windows are on the real desktop like the user's own: the
/// commands in their jobs, those with a window on it, and everything those started. A parent counts only if it started before
/// its child: Windows reuses the id of a process that ended, and a launcher that has exited can leave the user's own game with
/// a parent id a test process now holds.
fn monitor_processes() -> HashSet<u32> {
    let Some(bounds) = monitor::current_bounds() else { return HashSet::new() };
    let mut roots: HashSet<u32> = monitor::command_processes().into_iter().collect();
    if let Ok(real) = Desktop::real() {
        roots.extend(crate::windows_on(&real).iter().filter(|window| monitor::holds(&bounds, window)).map(|window| window.pid));
    }
    if roots.is_empty() {
        return roots;
    }
    let Some(table) = monitor::process_table() else { return roots };
    let parents: HashMap<u32, u32> = table.into_iter().collect();
    let mut started: HashMap<u32, Option<u64>> = HashMap::new();
    let mut started_at = |pid: u32| *started.entry(pid).or_insert_with(|| monitor::started_at(pid));
    parents.keys().copied().filter(|&pid| descends_from(pid, &roots, &parents, &mut started_at)).collect()
}

/// Whether a process is one of `roots` or started, through real parents, from one of them. A recorded parent counts only if it
/// started before its child: one that started later holds a reused id, and is not the parent.
fn descends_from(pid: u32, roots: &HashSet<u32>, parents: &HashMap<u32, u32>, started_at: &mut impl FnMut(u32) -> Option<u64>) -> bool {
    let mut current = pid;
    for _ in 0..MOST_ANCESTORS {
        if roots.contains(&current) {
            return true;
        }
        let Some(&parent) = parents.get(&current) else { return false };
        let is_its_parent = matches!((started_at(current), started_at(parent)), (Some(child), Some(parent)) if child >= parent);
        if parent == current || !is_its_parent {
            return false;
        }
        current = parent;
    }
    false
}

/// One audio session: its process, Windows' name for this very session, and its mute control. A program can have several, one
/// per output it plays to, and a new one when the default output changes.
struct Session {
    pid: u32,
    instance: String,
    volume: ISimpleAudioVolume,
}

struct Muted {
    /// Each session of the process this guard muted, by its instance name.
    sessions: HashMap<String, ISimpleAudioVolume>,
    program: String,
}

/// Wakes the guard the moment any program opens a new audio session, which is before it plays. Waiting for the next tick instead let
/// a hidden test be heard for 25 to 190 ms (measured with a 200 ms tick). It only wakes the guard: the guard's own thread mutes, so a
/// program is always written down before its mute.
#[implement(IAudioSessionNotification)]
struct NewSound {
    wake: usize,
}

impl IAudioSessionNotification_Impl for NewSound_Impl {
    fn OnSessionCreated(&self, _session: Ref<IAudioSessionControl>) -> windows::core::Result<()> {
        unsafe { SetEvent(self.wake as HANDLE) };
        Ok(())
    }
}

pub struct Guard {
    devices: IMMDeviceEnumerator,
    outputs: Vec<IAudioSessionManager2>,
    /// Windows' names for the outputs above: only a change of them makes the guard open them anew.
    output_names: Vec<String>,
    /// Each output's announcer of new sound, for taking it back.
    listeners: Vec<(IAudioSessionManager2, IAudioSessionNotification)>,
    /// Set by an announcer; the guard waits on it between ticks.
    wake: HANDLE,
    ticks_until_outputs_look: u32,
    muted: HashMap<u32, Muted>,
    /// Programs Windows may still start muted, from runs that ended; the list file keeps them across restarts.
    left_muted: HashSet<String>,
    /// Whether a process runs out of sight, by its id and start time: reading a process's memory, as `runs_hidden` must, is done
    /// once per process, not five times a second for the user's game.
    hidden: HashMap<u32, (u64, bool)>,
}

impl Guard {
    /// For the calling thread, which keeps it: audio objects stay on the thread that made them.
    pub fn new() -> windows::core::Result<Self> {
        unsafe {
            CoInitializeEx(Some(null()), COINIT_MULTITHREADED).ok()?;
            let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            // A program that no longer exists cannot start silent: test builds in scratch folders would otherwise pile up here.
            let listed = std::fs::read_to_string(list_file()).unwrap_or_default();
            let listed: Vec<&str> = listed.lines().filter(|line| !line.is_empty()).collect();
            let left_muted: HashSet<String> = listed.iter().filter(|line| std::path::Path::new(line).exists()).map(|line| line.to_string()).collect();
            let gone = left_muted.len() < listed.len();
            let wake = CreateEventW(null(), 0, 0, null());
            if wake.is_null() {
                return Err(windows::core::Error::from_thread());
            }
            let guard = Self {
                devices,
                outputs: Vec::new(),
                output_names: Vec::new(),
                listeners: Vec::new(),
                wake,
                ticks_until_outputs_look: 0,
                muted: HashMap::new(),
                left_muted,
                hidden: HashMap::new(),
            };
            if gone {
                guard.write_list();
            }
            Ok(guard)
        }
    }

    /// Opens the active outputs and has each announce new sound, unless they are the ones already open.
    fn look_for_outputs(&mut self) -> windows::core::Result<()> {
        unsafe {
            let outputs = self.devices.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
            let mut devices = Vec::new();
            let mut names = Vec::new();
            for index in 0..outputs.GetCount()? {
                let device = outputs.Item(index)?;
                let name = device.GetId()?;
                names.push(name.to_string().unwrap_or_default());
                CoTaskMemFree(Some(name.0 as *const _));
                devices.push(device);
            }
            if names == self.output_names {
                return Ok(());
            }
            self.stop_listening();
            let mut managers = Vec::new();
            for device in devices {
                let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
                let listener: IAudioSessionNotification = NewSound { wake: self.wake as usize }.into();
                // Without it new sound waits for the next tick, as it always did: noted, and the guard goes on.
                match manager.RegisterSessionNotification(&listener) {
                    Ok(()) => self.listeners.push((manager.clone(), listener)),
                    Err(error) => note_problem(&format!("an output will not announce new sound, so a test may be heard for up to 0.2 s: {error}")),
                }
                managers.push(manager);
            }
            self.outputs = managers;
            self.output_names = names;
        }
        Ok(())
    }

    fn stop_listening(&mut self) {
        for (manager, listener) in self.listeners.drain(..) {
            let _ = unsafe { manager.UnregisterSessionNotification(&listener) };
        }
    }

    /// Until new sound is announced, or a tick has passed.
    fn wait(&self) {
        unsafe { WaitForSingleObject(self.wake, GUARD_TICK.as_millis() as u32) };
    }

    /// Every audio session on every active output: its process and its mute control. An output that went away fails here, and
    /// the next tick looks for the outputs again.
    fn sessions(&mut self) -> windows::core::Result<Vec<Session>> {
        if self.ticks_until_outputs_look == 0 {
            self.look_for_outputs()?;
            self.ticks_until_outputs_look = TICKS_PER_OUTPUTS_LOOK;
        }
        self.ticks_until_outputs_look -= 1;
        let found = self.sessions_on_known_outputs();
        if found.is_err() {
            // Opened anew at the next tick, even if Windows still names the same outputs.
            self.ticks_until_outputs_look = 0;
            self.output_names.clear();
        }
        found
    }

    fn sessions_on_known_outputs(&self) -> windows::core::Result<Vec<Session>> {
        let mut found = Vec::new();
        unsafe {
            for manager in &self.outputs {
                let list = manager.GetSessionEnumerator()?;
                for session in 0..list.GetCount()? {
                    let control = list.GetSession(session)?;
                    let details: IAudioSessionControl2 = control.cast()?;
                    let Ok(pid) = details.GetProcessId() else { continue };
                    if pid == 0 {
                        continue;
                    }
                    let name = details.GetSessionInstanceIdentifier()?;
                    let instance = name.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(name.0 as *const _));
                    found.push(Session { pid, instance, volume: control.cast()? });
                }
            }
        }
        Ok(found)
    }

    /// Whether this process runs out of sight, read once per process. A process Windows will not say it of yet, such as one too new
    /// to read, is asked again next time; one it will never say it of costs only a refused open each time.
    fn runs_hidden(&mut self, pid: u32) -> bool {
        let Some(started) = monitor::started_at(pid) else { return false };
        if let Some(&(at, hidden)) = self.hidden.get(&pid)
            && at == started
        {
            return hidden;
        }
        match desktop_is_hidden(pid) {
            Some(hidden) => {
                self.hidden.insert(pid, (started, hidden));
                hidden
            }
            None => false,
        }
    }

    fn write_list(&self) {
        let mut programs: Vec<&str> = self.muted.values().map(|entry| entry.program.as_str()).filter(|name| !name.is_empty()).collect();
        programs.extend(self.left_muted.iter().map(String::as_str));
        programs.sort_unstable();
        programs.dedup();
        let result = std::fs::create_dir_all(state_dir()).and_then(|()| std::fs::write(list_file(), programs.join("\n")));
        if let Err(error) = result {
            note_problem(&format!("could not write the list of muted programs: {error}"));
        }
    }

    pub fn tick(&mut self) {
        let enabled = muting_enabled();
        if !enabled && self.muted.is_empty() && self.left_muted.is_empty() {
            return;
        }
        let sessions = match self.sessions() {
            Ok(sessions) => sessions,
            Err(error) => {
                note_problem(&format!("could not read the audio sessions: {error}"));
                return;
            }
        };
        let mut changed = false;
        let guid = GUID::zeroed();
        let mut tests = HashSet::new();
        let playing: HashSet<u32> = sessions.iter().map(|session| session.pid).collect();
        self.hidden.retain(|pid, _| playing.contains(pid));
        // Hidden programs first: telling them apart is a read of each process, while the monitor's processes cost a look at every
        // window and process, which new hidden sound must not wait for.
        let (hidden, others): (Vec<Session>, Vec<Session>) = sessions.into_iter().partition(|session| enabled && self.runs_hidden(session.pid));
        let mut on_monitor: Option<HashSet<u32>> = None;
        let hidden = hidden.into_iter().map(|session| (session, true));
        for (Session { pid, instance, volume }, is_hidden) in hidden.chain(others.into_iter().map(|session| (session, false))) {
            if is_hidden || (enabled && on_monitor.get_or_insert_with(monitor_processes).contains(&pid)) {
                tests.insert(pid);
                if let Entry::Vacant(slot) = self.muted.entry(pid) {
                    // Written down before the mute, so no crash can leave a mute nobody knows about.
                    slot.insert(Muted { sessions: HashMap::new(), program: process_path(pid) });
                    self.write_list();
                }
                let entry = self.muted.get_mut(&pid).expect("inserted above");
                if let Entry::Vacant(slot) = entry.sessions.entry(instance) {
                    if let Err(error) = unsafe { volume.SetMute(true, &guid) } {
                        note_problem(&format!("could not mute a test's sound: {error}"));
                    }
                    slot.insert(volume);
                }
            } else if !self.muted.contains_key(&pid) && !self.left_muted.is_empty() {
                let program = process_path(pid);
                // Only a session actually found muted clears the entry: the user's copy, already running and unmuted when
                // a test of the same program ended, says nothing about what the next launch will inherit.
                if self.left_muted.contains(&program) && unsafe { volume.GetMute() }.is_ok_and(|muted| muted.as_bool()) {
                    match unsafe { volume.SetMute(false, &guid) } {
                        Ok(()) => {
                            self.left_muted.remove(&program);
                            changed = true;
                        }
                        Err(error) => note_problem(&format!("could not give {program} its sound back: {error}")),
                    }
                }
            }
        }
        let finished: Vec<u32> = self.muted.keys().copied().filter(|pid| !tests.contains(pid)).collect();
        for pid in finished {
            if let Some(entry) = self.muted.remove(&pid) {
                // Still running (the option was turned off): this works, and is what Windows will remember. Ended: too late,
                // so it stays on the list.
                if !unmute(pid, &entry) {
                    self.left_muted.insert(entry.program);
                }
                changed = true;
            }
        }
        if changed {
            self.write_list();
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.stop_listening();
        unsafe { CloseHandle(self.wake) };
    }
}

impl Guard {
    /// Takes back every mute it still can, and leaves the rest on the list for the next time the guard runs.
    fn release(&mut self) {
        let entries: Vec<(u32, Muted)> = self.muted.drain().collect();
        for (pid, entry) in entries {
            if !unmute(pid, &entry) {
                self.left_muted.insert(entry.program);
            }
        }
        self.write_list();
    }
}

/// Gives every session of a muted process its sound back, if the process still runs: once it has ended, Windows ignores an unmute
/// and remembers the mute for the program's next launch. True when all of them took it.
fn unmute(pid: u32, entry: &Muted) -> bool {
    let guid = GUID::zeroed();
    process_path(pid) == entry.program && entry.sessions.values().all(|volume| unsafe { volume.SetMute(false, &guid) }.is_ok())
}

/// Runs the guard on the calling thread until `stop` is set, and takes every mute back then.
pub fn run_guard(stop: &AtomicBool) {
    let mut guard = match Guard::new() {
        Ok(guard) => guard,
        Err(error) => {
            note_problem(&format!("the sound guard could not start, so test sounds are not muted: {error}"));
            return;
        }
    };
    while !stop.load(Ordering::Relaxed) {
        guard.tick();
        guard.wait();
    }
    guard.release();
}

#[cfg(test)]
mod tests {
    use super::descends_from;
    use std::collections::{HashMap, HashSet};

    /// The user's game was started by a launcher that has exited; a test process now holds the launcher's old id. The game must not
    /// count as the test's: it started before the test process did.
    #[test]
    fn a_process_whose_recorded_parent_started_later_is_not_its_child() {
        let roots: HashSet<u32> = [10].into();
        // pid -> parent pid, and when each started.
        let parents: HashMap<u32, u32> = [(10, 1), (20, 10), (30, 20), (40, 10)].into();
        let started: HashMap<u32, u64> = [(1, 0), (10, 100), (20, 110), (30, 120), (40, 50)].into();
        let mut started_at = |pid: u32| started.get(&pid).copied();
        assert!(descends_from(10, &roots, &parents, &mut started_at), "a root");
        assert!(descends_from(30, &roots, &parents, &mut started_at), "a grandchild, started after each parent");
        assert!(!descends_from(40, &roots, &parents, &mut started_at), "started before the process that now holds its parent's id");
        assert!(!descends_from(1, &roots, &parents, &mut started_at), "the root's own parent");
    }
}
