//! Mac-Teile von FreeSpeech. Auf anderen Systemen ist dieses Paket leer.
#![cfg(target_os = "macos")]

pub mod hotkey;
pub mod paste;
pub mod system;

/// Kennzeichnet Tastendrücke, die FreeSpeech selbst erzeugt (Einfügen), damit
/// der eigene Tastatur-Abgriff sie nicht als Kürzel wertet.
pub const MAGIC: i64 = 0x4653_5045;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    /// Ist diese Taste laut System gerade gedrückt? (stateID 1 = Hardware-Zustand)
    fn CGEventSourceKeyState(state_id: i32, key: u16) -> bool;
}

pub(crate) fn key_is_down(keycode: u16) -> bool {
    unsafe { CGEventSourceKeyState(1, keycode) }
}
