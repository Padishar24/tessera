//! System and window events for the main thread.
//!
//! WinEvent hooks (out-of-context) and the window procedure of the
//! [`MessageWindow`](crate::msgloop::MessageWindow) put events into a
//! thread-local queue that the daemon empties with [`drain`] after each
//! message.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    CHILDID_SELF, EVENT_OBJECT_CLOAKED, EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE,
    EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_SHOW,
    EVENT_OBJECT_UNCLOAKED, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND,
    EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZESTART, OBJID_WINDOW,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};

use crate::msgloop::Waker;
use crate::window::Window;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowEvent {
    Shown,
    Hidden,
    Destroyed,
    Foreground,
    MoveSizeStart,
    MoveSizeEnd,
    MinimizeStart,
    MinimizeEnd,
    TitleChanged,
    /// Position, size or maximized state has changed.
    LocationChanged,
    Cloaked,
    Uncloaked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Window(WindowEvent, Window),
    /// Monitors plugged in/unplugged, resolution or scaling changed.
    DisplayChanged,
    /// Work area changed (e.g. taskbar moved).
    WorkAreaChanged,
    /// A timer set via [`Timers`](crate::msgloop::Timers) has elapsed.
    Timer(usize),
    /// The computer has resumed from sleep/hibernation.
    Resumed,
    /// Click on a [`TabBar`](crate::tabbar::TabBar); `x` relative to the bar.
    TabClicked {
        bar: usize,
        x: i32,
    },
}

const HOOKED: &[(u32, WindowEvent)] = &[
    (EVENT_OBJECT_SHOW, WindowEvent::Shown),
    (EVENT_OBJECT_HIDE, WindowEvent::Hidden),
    (EVENT_OBJECT_DESTROY, WindowEvent::Destroyed),
    (EVENT_SYSTEM_FOREGROUND, WindowEvent::Foreground),
    (EVENT_SYSTEM_MOVESIZESTART, WindowEvent::MoveSizeStart),
    (EVENT_SYSTEM_MOVESIZEEND, WindowEvent::MoveSizeEnd),
    (EVENT_SYSTEM_MINIMIZESTART, WindowEvent::MinimizeStart),
    (EVENT_SYSTEM_MINIMIZEEND, WindowEvent::MinimizeEnd),
    (EVENT_OBJECT_NAMECHANGE, WindowEvent::TitleChanged),
    (EVENT_OBJECT_LOCATIONCHANGE, WindowEvent::LocationChanged),
    (EVENT_OBJECT_CLOAKED, WindowEvent::Cloaked),
    (EVENT_OBJECT_UNCLOAKED, WindowEvent::Uncloaked),
];

thread_local! {
    static QUEUE: RefCell<VecDeque<Event>> = const { RefCell::new(VecDeque::new()) };
    static WAKER: Cell<Option<Waker>> = const { Cell::new(None) };
    static WAKE_PENDING: Cell<bool> = const { Cell::new(false) };
}

/// Puts an event into the current thread's queue.
pub(crate) fn push(event: Event) {
    QUEUE.with_borrow_mut(|q| q.push_back(event));
}

/// Takes all events accumulated so far.
pub fn drain() -> Vec<Event> {
    WAKE_PENDING.set(false);
    QUEUE.with_borrow_mut(|q| q.drain(..).collect())
}

/// Installed WinEvent hooks; removed on drop. Must be created on the
/// thread running the message loop.
pub struct EventHooks(Vec<HWINEVENTHOOK>);

impl EventHooks {
    /// `waker` wakes the message loop: out-of-context callbacks run inside
    /// `GetMessageW` without a message being returned.
    pub fn install(waker: Waker) -> anyhow::Result<Self> {
        WAKER.set(Some(waker));
        let mut hooks = Vec::with_capacity(HOOKED.len());
        for &(event, _) in HOOKED {
            // SAFETY: callback is a static function; hook is removed in Drop.
            let hook = unsafe {
                SetWinEventHook(
                    event,
                    event,
                    None,
                    Some(callback),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                )
            };
            if hook.is_invalid() {
                anyhow::bail!("SetWinEventHook for event {event:#x} failed");
            }
            hooks.push(hook);
        }
        Ok(Self(hooks))
    }
}

impl Drop for EventHooks {
    fn drop(&mut self) {
        for hook in self.0.drain(..) {
            // SAFETY: hook comes from SetWinEventHook.
            unsafe {
                let _ = UnhookWinEvent(hook);
            }
        }
        WAKER.set(None);
    }
}

unsafe extern "system" fn callback(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    // Only events concerning the window itself (not cursor, caret, children).
    if hwnd.is_invalid() || id_object != OBJID_WINDOW.0 || id_child != CHILDID_SELF as i32 {
        return;
    }
    let Some(&(_, kind)) = HOOKED.iter().find(|(e, _)| *e == event) else {
        return;
    };
    push(Event::Window(kind, Window(hwnd)));
    if !WAKE_PENDING.replace(true)
        && let Some(waker) = WAKER.get()
    {
        waker.wake();
    }
}
