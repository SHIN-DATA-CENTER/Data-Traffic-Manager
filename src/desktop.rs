//! Desktop integration: single instance, start with Windows, opening folders.

use std::io;
use std::path::Path;

/// Command line flag used by the autostart entry to start in the tray.
pub const MINIMIZED_FLAG: &str = "--minimized";

/// Command line flag that asks a running instance to save its data and exit
/// (used by the installer before replacing or removing the executable).
pub const QUIT_FLAG: &str = "--quit";

/// Result of [`single_instance`].
pub enum Instance {
    /// This process is the first instance. Keep the value alive.
    Primary(#[allow(dead_code)] InstanceGuard),
    /// Another instance is running and has been asked to show its window.
    Secondary,
}

/// Ensures only one instance runs per user session.
///
/// When another instance is already running it is notified (it calls
/// `on_activate` on a background thread) and [`Instance::Secondary`] is
/// returned. The primary instance calls `on_quit` (on a background thread)
/// when [`request_quit`] is used. On platforms without support every
/// process is primary.
pub fn single_instance(on_activate: impl Fn() + Send + 'static, on_quit: impl Fn() + Send + 'static) -> Instance {
    imp::single_instance(on_activate, on_quit)
}

/// Asks the running instance, if any, to exit. Returns whether one was found.
pub fn request_quit() -> bool {
    imp::request_quit()
}

pub use imp::InstanceGuard;

/// Whether "start with Windows" is available on this platform.
pub fn autostart_supported() -> bool {
    cfg!(windows)
}

/// Whether the application is registered to start at logon.
pub fn autostart_enabled() -> bool {
    imp::autostart_enabled()
}

/// Registers or removes the logon entry. The entry starts the application
/// minimized to the tray.
pub fn set_autostart(enabled: bool) -> io::Result<()> {
    imp::set_autostart(enabled)
}

/// Opens a folder in the system file manager.
pub fn open_folder(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)?;
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program).arg(path).spawn().map(drop)
}

#[cfg(windows)]
mod imp {
    use std::io;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, GetLastError, HANDLE, NO_ERROR, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
    };
    use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForSingleObject};
    use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};

    use super::{Instance, MINIMIZED_FLAG};

    /// Prefix of the named events; `.Activate` and `.Quit` are appended.
    const EVENT_BASE: &str = "Local\\SHIN-DATA-CENTER.DataTrafficManager";
    const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    const RUN_VALUE: &str = "DataTrafficManager";

    pub struct InstanceGuard(#[allow(dead_code)] HANDLE);

    // SAFETY: an event handle can be used from any thread.
    unsafe impl Send for InstanceGuard {}

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    pub fn single_instance(on_activate: impl Fn() + Send + 'static, on_quit: impl Fn() + Send + 'static) -> Instance {
        single_instance_named(EVENT_BASE, on_activate, on_quit)
    }

    pub fn request_quit() -> bool {
        request_quit_named(EVENT_BASE)
    }

    /// Creates (or opens) an auto-reset named event. Returns the handle and
    /// whether the event already existed.
    fn named_event(name: &str) -> Option<(HANDLE, bool)> {
        let name = wide(name);
        // SAFETY: valid, NUL terminated name; default security attributes.
        let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
        // SAFETY: called right after CreateEventW on the same thread.
        let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        (!event.is_null()).then_some((event, existed))
    }

    /// Calls `callback` every time `event` is signalled. The handle stays open
    /// for the lifetime of the process.
    fn watch(event: HANDLE, thread_name: &str, callback: impl Fn() + Send + 'static) {
        let event = event as usize;
        std::thread::Builder::new()
            .name(thread_name.into())
            .spawn(move || {
                // SAFETY: the handle is never closed while the process runs.
                while unsafe { WaitForSingleObject(event as HANDLE, INFINITE) } == WAIT_OBJECT_0 {
                    callback();
                }
            })
            .ok();
    }

    pub(super) fn single_instance_named(
        base: &str,
        on_activate: impl Fn() + Send + 'static,
        on_quit: impl Fn() + Send + 'static,
    ) -> Instance {
        let Some((activate, existed)) = named_event(&format!("{base}.Activate")) else {
            // Cannot tell; behave as if we were alone.
            return Instance::Primary(InstanceGuard(std::ptr::null_mut()));
        };
        if existed {
            // SAFETY: `activate` is a valid handle owned by us.
            unsafe {
                // Let the running instance bring its window to the front.
                AllowSetForegroundWindow(ASFW_ANY);
                SetEvent(activate);
                CloseHandle(activate);
            }
            return Instance::Secondary;
        }
        watch(activate, "instance-activation", on_activate);
        if let Some((quit, _)) = named_event(&format!("{base}.Quit")) {
            watch(quit, "instance-quit", on_quit);
        }
        Instance::Primary(InstanceGuard(activate))
    }

    pub(super) fn request_quit_named(base: &str) -> bool {
        let Some((quit, existed)) = named_event(&format!("{base}.Quit")) else {
            return false;
        };
        // SAFETY: `quit` is a valid handle owned by us.
        unsafe {
            if existed {
                SetEvent(quit);
            }
            CloseHandle(quit);
        }
        existed
    }

    pub fn autostart_enabled() -> bool {
        autostart_enabled_named(RUN_VALUE)
    }

    pub fn set_autostart(enabled: bool) -> io::Result<()> {
        set_autostart_named(RUN_VALUE, enabled)
    }

    pub(super) fn autostart_enabled_named(name: &str) -> bool {
        let key = wide(RUN_KEY);
        let value = wide(name);
        let mut size = 0u32;
        // SAFETY: only queries the size of the value.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        status == NO_ERROR
    }

    pub(super) fn set_autostart_named(name: &str, enabled: bool) -> io::Result<()> {
        let key = wide(RUN_KEY);
        let value = wide(name);
        let status = if enabled {
            let exe = std::env::current_exe()?;
            let command = wide(&format!("\"{}\" {MINIMIZED_FLAG}", exe.display()));
            // SAFETY: `command` is a NUL terminated UTF-16 buffer of the given size.
            unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    key.as_ptr(),
                    value.as_ptr(),
                    REG_SZ,
                    command.as_ptr().cast(),
                    (command.len() * 2) as u32,
                )
            }
        } else {
            // SAFETY: valid NUL terminated strings.
            match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) } {
                ERROR_FILE_NOT_FOUND => NO_ERROR,
                other => other,
            }
        };
        if status == NO_ERROR { Ok(()) } else { Err(io::Error::from_raw_os_error(status as i32)) }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::io;

    use super::Instance;

    pub struct InstanceGuard;

    pub fn single_instance(_on_activate: impl Fn() + Send + 'static, _on_quit: impl Fn() + Send + 'static) -> Instance {
        Instance::Primary(InstanceGuard)
    }

    pub fn request_quit() -> bool {
        false
    }

    pub fn autostart_enabled() -> bool {
        false
    }

    pub fn set_autostart(_enabled: bool) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "自動起動はこのOSでは利用できません"))
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    use super::imp::{autostart_enabled_named, request_quit_named, set_autostart_named, single_instance_named};
    use super::*;

    // The tests use their own names so they never touch a real installation.

    fn wait_for(counter: &AtomicU32, expected: u32) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while counter.load(Ordering::SeqCst) < expected && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(counter.load(Ordering::SeqCst), expected);
    }

    #[test]
    fn second_instance_activates_the_first() {
        let name = format!("Local\\DataTrafficManager.Test.{}", std::process::id());
        let activations = Arc::new(AtomicU32::new(0));
        let quits = Arc::new(AtomicU32::new(0));
        let (a, q) = (activations.clone(), quits.clone());
        let first = single_instance_named(
            &name,
            move || {
                a.fetch_add(1, Ordering::SeqCst);
            },
            move || {
                q.fetch_add(1, Ordering::SeqCst);
            },
        );
        assert!(matches!(first, Instance::Primary(_)));
        assert!(matches!(single_instance_named(&name, || {}, || {}), Instance::Secondary));
        wait_for(&activations, 1);

        assert!(request_quit_named(&name));
        wait_for(&quits, 1);
        assert_eq!(activations.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn quit_without_running_instance() {
        let name = format!("Local\\DataTrafficManager.NoInstance.{}", std::process::id());
        assert!(!request_quit_named(&name));
        // The probe must not leave an event behind that looks like an instance.
        assert!(!request_quit_named(&name));
    }

    #[test]
    fn autostart_roundtrip() {
        let name = format!("DataTrafficManagerTest{}", std::process::id());
        assert!(!autostart_enabled_named(&name));
        set_autostart_named(&name, true).unwrap();
        assert!(autostart_enabled_named(&name));
        set_autostart_named(&name, false).unwrap();
        assert!(!autostart_enabled_named(&name));
        set_autostart_named(&name, false).unwrap(); // removing twice is fine
    }
}
