//! Global shortcuts via a low-level keyboard hook.
//!
//! The hook sees key presses before Windows' hotkey processing and can
//! therefore also take over combinations already in use, like `Win+Alt+K`. It
//! runs on its own thread and only does the bare minimum there: look up the
//! combination, swallow the key, pass on the action. If the hook takes too
//! long, Windows silently removes it.
//!
//! For window hints the hook can also capture the keyboard for a moment:
//! every key press then goes to the daemon instead of the focused window
//! (see [`Hotkeys::set_capture`]).

use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use std::sync::Mutex;
use std::sync::mpsc;
use std::thread::JoinHandle;

use tessera_core::action::Action;
use tessera_core::keys::{Key, KeyCombo, Modifiers};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE, VK_LCONTROL, VK_LMENU,
    VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT, VkKeyScanW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG,
    PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_APP, WM_KEYDOWN,
    WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

/// Unassigned virtual key used to suppress the Start menu (after Win) or the
/// menu bar (after Alt).
const VK_DUMMY: u16 = 0xE8;
/// Thread message to the hook thread: reinstall the hook.
const WM_REINSTALL: u32 = WM_APP + 10;
/// The hook ends a keyboard capture by itself after this time, even if the
/// daemon does not (hangs, misses the end): the keyboard must never stay
/// captured.
const CAPTURE_MS: u32 = 10_000;

/// What the hook reports to the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    /// A bound key combination was pressed.
    Action(Action),
    /// A key pressed during a capture. `Escape` ends the capture in the
    /// hook right away.
    Key { vk: u16, mods: Modifiers },
}

type Callback = Box<dyn Fn(HookEvent) + Send>;

struct Shared {
    bindings: HashMap<(Modifiers, u16), Action>,
    on_event: Callback,
    /// Keys whose keydown was swallowed - swallow their keyup too.
    swallowed: HashSet<u16>,
    /// Tick count (`GetTickCount`) until which all key presses are captured.
    capture_until: Option<u32>,
}

static SHARED: Mutex<Option<Shared>> = Mutex::new(None);

/// Runs as long as the value lives.
pub struct Hotkeys {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl Hotkeys {
    /// `on_event` runs on the hook thread and must return immediately.
    pub fn start(on_event: impl Fn(HookEvent) + Send + 'static) -> anyhow::Result<Self> {
        *lock() = Some(Shared {
            bindings: HashMap::new(),
            on_event: Box::new(on_event),
            swallowed: HashSet::new(),
            capture_until: None,
        });

        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("hotkeys".into())
            .spawn(move || {
                // SAFETY: hook is installed, serviced and removed on this thread.
                unsafe {
                    let module = GetModuleHandleW(None).ok();
                    let hook = SetWindowsHookExW(
                        WH_KEYBOARD_LL,
                        Some(hook_proc),
                        module.map(Into::into),
                        0,
                    );
                    let mut hook = match hook {
                        Ok(hook) => hook,
                        Err(e) => {
                            let _ = ready_tx.send(Err(e));
                            return;
                        }
                    };
                    let _ = ready_tx.send(Ok(GetCurrentThreadId()));
                    let mut msg = MSG::default();
                    while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                        if msg.message == WM_REINSTALL {
                            // Windows can silently remove LL hooks (timeout,
                            // sleep); a fresh hook costs nothing.
                            let _ = UnhookWindowsHookEx(hook);
                            match SetWindowsHookExW(
                                WH_KEYBOARD_LL,
                                Some(hook_proc),
                                module.map(Into::into),
                                0,
                            ) {
                                Ok(fresh) => hook = fresh,
                                Err(e) => tracing::error!("Reinstalling keyboard hook: {e}"),
                            }
                            continue;
                        }
                        DispatchMessageW(&msg);
                    }
                    let _ = UnhookWindowsHookEx(hook);
                }
            })?;

        let thread_id = ready_rx.recv()??;
        Ok(Self {
            thread_id,
            thread: Some(thread),
        })
    }

    /// Reinstalls the hook (watchdog, after sleep).
    pub fn reinstall(&self) {
        // SAFETY: thread message to the hook thread.
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_REINSTALL, WPARAM(0), LPARAM(0));
        }
    }

    /// Starts or ends capturing the keyboard: while it runs, every key press
    /// is swallowed and reported as [`HookEvent::Key`]. A binding for
    /// [`Action::Hint`] starts a capture in the hook itself, so that no key
    /// pressed right after it slips through before the daemon reacts.
    pub fn set_capture(&self, capture: bool) {
        if let Some(shared) = lock().as_mut() {
            shared.capture_until = capture.then(capture_deadline);
        }
    }

    /// Replaces all bindings. Returns messages for combinations whose
    /// character does not exist on the current keyboard layout.
    pub fn set_bindings(&self, bindings: &[(KeyCombo, Action)]) -> Vec<String> {
        let mut errors = Vec::new();
        let mut map = HashMap::with_capacity(bindings.len());
        for (combo, action) in bindings {
            match resolve(combo.key) {
                Some(vk) => {
                    map.insert((combo.mods, vk), *action);
                }
                None => errors.push(format!(
                    "`{combo}`: character does not exist on the current keyboard layout"
                )),
            }
        }
        if let Some(shared) = lock().as_mut() {
            shared.bindings = map;
        }
        errors
    }
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        // SAFETY: ends the hook thread's message loop.
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        *lock() = None;
    }
}

fn lock() -> std::sync::MutexGuard<'static, Option<Shared>> {
    SHARED.lock().unwrap_or_else(|e| e.into_inner())
}

fn resolve(key: Key) -> Option<u16> {
    match key {
        Key::Vk(vk) => Some(vk),
        Key::Char(c) => {
            let mut buf = [0u16; 2];
            let [unit] = c.encode_utf16(&mut buf) else {
                return None;
            };
            // SAFETY: pure query of the keyboard layout.
            let scan = unsafe { VkKeyScanW(*unit) };
            (scan != -1).then_some((scan as u16) & 0xFF)
        }
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: with HC_ACTION, lparam points to a KBDLLHOOKSTRUCT.
        let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if event.flags.0 & LLKHF_INJECTED.0 == 0 && handle(wparam.0 as u32, event.vkCode as u16) {
            return LRESULT(1);
        }
    }
    // SAFETY: passing on to the next hook procedure.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn capture_deadline() -> u32 {
    // SAFETY: pure query without parameters.
    unsafe { GetTickCount() }.wrapping_add(CAPTURE_MS)
}

/// Is the tick count `deadline` still ahead (robust against wrap-around)?
fn before(deadline: u32) -> bool {
    // SAFETY: pure query without parameters.
    let now = unsafe { GetTickCount() };
    (deadline.wrapping_sub(now) as i32) > 0
}

/// `true` = swallow the key.
fn handle(message: u32, vk: u16) -> bool {
    if is_modifier(vk) {
        return false;
    }
    let mut guard = lock();
    let Some(shared) = guard.as_mut() else {
        return false;
    };
    match message {
        WM_KEYUP | WM_SYSKEYUP => shared.swallowed.remove(&vk),
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let mods = current_modifiers();
            if let Some(deadline) = shared.capture_until {
                if before(deadline) {
                    // Auto-repeat of a key that is still held (e.g. the key
                    // that started the capture) is not a new key press.
                    if !shared.swallowed.insert(vk) {
                        return true;
                    }
                    if mods.contains(Modifiers::WIN) || mods.contains(Modifiers::ALT) {
                        tap_dummy_key();
                    }
                    if vk == VK_ESCAPE.0 {
                        shared.capture_until = None;
                    }
                    (shared.on_event)(HookEvent::Key { vk, mods });
                    return true;
                }
                shared.capture_until = None;
            }
            let Some(&action) = shared.bindings.get(&(mods, vk)) else {
                return false;
            };
            if mods.contains(Modifiers::WIN) || mods.contains(Modifiers::ALT) {
                tap_dummy_key();
            }
            shared.swallowed.insert(vk);
            if action == Action::Hint {
                shared.capture_until = Some(capture_deadline());
            }
            (shared.on_event)(HookEvent::Action(action));
            true
        }
        _ => false,
    }
}

fn is_modifier(vk: u16) -> bool {
    [
        VK_LWIN,
        VK_RWIN,
        VK_SHIFT,
        VK_LSHIFT,
        VK_RSHIFT,
        VK_CONTROL,
        VK_LCONTROL,
        VK_RCONTROL,
        VK_MENU,
        VK_LMENU,
        VK_RMENU,
    ]
    .iter()
    .any(|k| k.0 == vk)
}

fn current_modifiers() -> Modifiers {
    // SAFETY: pure queries of the key state.
    let down = |vk: VIRTUAL_KEY| unsafe { GetAsyncKeyState(vk.0 as i32) } < 0;
    let mut mods = Modifiers::NONE;
    if down(VK_LWIN) || down(VK_RWIN) {
        mods = mods | Modifiers::WIN;
    }
    if down(VK_CONTROL) {
        mods = mods | Modifiers::CTRL;
    }
    if down(VK_MENU) {
        mods = mods | Modifiers::ALT;
    }
    if down(VK_SHIFT) {
        mods = mods | Modifiers::SHIFT;
    }
    mods
}

/// A key press in between prevents releasing Win from opening the Start menu
/// or releasing Alt from opening the menu bar.
fn tap_dummy_key() {
    let key = |flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(VK_DUMMY),
                dwFlags: flags,
                ..Default::default()
            },
        },
    };
    // SAFETY: valid INPUT structures with correct size.
    unsafe {
        SendInput(
            &[key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)],
            size_of::<INPUT>() as i32,
        );
    }
}
