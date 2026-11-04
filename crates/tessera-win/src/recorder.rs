//! Recording a key combination for the shortcuts window. A keyboard hook of
//! its own captures the next key press; installed after Tessera's hook, it
//! sees keys first, so combinations that Tessera or Windows use can be
//! recorded too. The hook's capture ends by itself after a few seconds.

use std::sync::mpsc::{self, Receiver};

use tessera_core::keys::KeyCombo;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;

use crate::hotkeys::{HookEvent, Hotkeys, key_for_vk};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded {
    /// The pressed combination; not checked yet (it may lack a modifier).
    Combo(KeyCombo),
    /// Escape without modifiers.
    Cancelled,
}

/// Records while the value lives.
pub struct Recorder {
    _hotkeys: Hotkeys,
    events: Receiver<Recorded>,
}

impl Recorder {
    /// `wake` runs on the hook thread after a key press, e.g. to repaint.
    pub fn start(wake: impl Fn() + Send + 'static) -> anyhow::Result<Self> {
        let (tx, events) = mpsc::channel();
        let hotkeys = Hotkeys::start(move |event| {
            if let HookEvent::Key { vk, mods } = event {
                let recorded = if vk == VK_ESCAPE.0 && mods.is_empty() {
                    Recorded::Cancelled
                } else {
                    Recorded::Combo(KeyCombo {
                        mods,
                        key: key_for_vk(vk),
                    })
                };
                let _ = tx.send(recorded);
                wake();
            }
        })?;
        hotkeys.set_capture(true);
        Ok(Self {
            _hotkeys: hotkeys,
            events,
        })
    }

    /// The first key press since the start, if there was one.
    pub fn poll(&self) -> Option<Recorded> {
        self.events.try_recv().ok()
    }
}
