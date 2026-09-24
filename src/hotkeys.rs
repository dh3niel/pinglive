use std::collections::HashMap;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::GlobalHotKeyManager;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Let the overlay take the mouse so it can be dragged, or give it back.
    ToggleInteractive,
    Dashboard,
    ToggleMute,
    ToggleGraph,
    ToggleVisible,
    ResetStats,
    Quit,
}

/// System-wide hotkeys. The manager must stay alive for them to keep working,
/// so the app owns this struct for its whole run.
pub struct Hotkeys {
    _manager: GlobalHotKeyManager,
    map: HashMap<u32, Action>,
}

impl Hotkeys {
    pub fn register() -> Option<Self> {
        let manager = match GlobalHotKeyManager::new() {
            Ok(m) => m,
            Err(e) => {
                eprintln!("pinglive: hotkeys unavailable: {e}");
                return None;
            }
        };
        let modifiers = Some(Modifiers::CONTROL | Modifiers::ALT);
        let bindings = [
            (Code::KeyP, Action::ToggleInteractive),
            (Code::KeyD, Action::Dashboard),
            (Code::KeyM, Action::ToggleMute),
            (Code::KeyG, Action::ToggleGraph),
            (Code::KeyH, Action::ToggleVisible),
            (Code::KeyR, Action::ResetStats),
            (Code::KeyQ, Action::Quit),
        ];

        let mut map = HashMap::new();
        for (code, action) in bindings {
            let hk = HotKey::new(modifiers, code);
            // A hotkey another app already owns is skipped, not fatal.
            match manager.register(hk) {
                Ok(()) => {
                    map.insert(hk.id(), action);
                }
                Err(e) => eprintln!("pinglive: could not bind {action:?}: {e}"),
            }
        }
        Some(Self {
            _manager: manager,
            map,
        })
    }

    pub fn action_for(&self, id: u32) -> Option<Action> {
        self.map.get(&id).copied()
    }
}
