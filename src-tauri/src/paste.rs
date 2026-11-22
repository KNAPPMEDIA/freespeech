//! Text ins aktive Fenster bringen: über die Zwischenablage (Strg+V bzw. Command+V)
//! oder direkt getippt. Dazu zwei kurze Signaltöne für Start und Stopp.
//! Der gemeinsame Ablauf steht hier, das Senden der Tasten je System in `os`.

use std::time::Duration;

#[cfg(windows)]
mod os {
    use crate::hotkey::MAGIC;
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    };

    fn key(vk: u16, scan: u16, flags: u32) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: MAGIC } },
        }
    }

    fn send(inputs: &[INPUT]) {
        unsafe {
            SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32);
        }
    }

    /// Wartet, bis Strg/Umschalt/Alt/Win losgelassen sind. Sonst würde aus dem
    /// Einfügen z. B. Win+V (Zwischenablage-Verlauf).
    pub fn wait_modifiers_released() {
        let start = Instant::now();
        let mods = [0x10, 0x11, 0x12, 0x5B, 0x5C];
        while start.elapsed() < Duration::from_millis(1500) {
            let any = mods.iter().any(|vk| unsafe { GetAsyncKeyState(*vk) as u16 & 0x8000 != 0 });
            if !any {
                return;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    }

    /// Strg+V.
    pub fn press_paste() {
        send(&[
            key(0x11, 0, 0),
            key(0x56, 0, 0),
            key(0x56, 0, KEYEVENTF_KEYUP),
            key(0x11, 0, KEYEVENTF_KEYUP),
        ]);
    }

    pub fn type_text(text: &str) {
        let mut batch: Vec<INPUT> = Vec::new();
        for ch in text.chars() {
            if ch == '\n' {
                batch.push(key(0x0D, 0, 0));
                batch.push(key(0x0D, 0, KEYEVENTF_KEYUP));
            } else if ch != '\r' {
                let mut units = [0u16; 2];
                for u in ch.encode_utf16(&mut units) {
                    batch.push(key(0, *u, KEYEVENTF_UNICODE));
                    batch.push(key(0, *u, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
                }
            }
            if batch.len() >= 64 {
                send(&batch);
                batch.clear();
                std::thread::sleep(Duration::from_millis(4));
            }
        }
        if !batch.is_empty() {
            send(&batch);
        }
    }

    /// Kurzer Zweiklang als WAV im Speicher (16 Bit, 22,05 kHz, mono).
    fn tone(freqs: &[f32]) -> Vec<u8> {
        let rate = 22050u32;
        let per = (rate as f32 * 0.075) as usize;
        let mut pcm: Vec<i16> = Vec::new();
        for f in freqs {
            for i in 0..per {
                let t = i as f32 / rate as f32;
                let env = (i as f32 / per as f32 * std::f32::consts::PI).sin().powf(0.7);
                pcm.push(((t * f * std::f32::consts::TAU).sin() * env * 0.22 * 32767.0) as i16);
            }
        }
        let data_len = (pcm.len() * 2) as u32;
        let mut w = Vec::with_capacity(44 + data_len as usize);
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data_len).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&rate.to_le_bytes());
        w.extend_from_slice(&(rate * 2).to_le_bytes());
        w.extend_from_slice(&2u16.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data_len.to_le_bytes());
        for s in pcm {
            w.extend_from_slice(&s.to_le_bytes());
        }
        w
    }

    pub fn play(start: bool) {
        static START: OnceLock<Vec<u8>> = OnceLock::new();
        static STOP: OnceLock<Vec<u8>> = OnceLock::new();
        let wav = if start {
            START.get_or_init(|| tone(&[587.3, 880.0]))
        } else {
            STOP.get_or_init(|| tone(&[880.0, 587.3]))
        };
        unsafe {
            PlaySoundW(wav.as_ptr() as *const u16, std::ptr::null_mut(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
        }
    }
}

#[cfg(target_os = "macos")]
mod os {
    pub use fs_mac::paste::{press_paste, type_text, wait_modifiers_released};

    /// Signaltöne über die mitgelieferten Systemklänge.
    pub fn play(start: bool) {
        let sound = if start { "/System/Library/Sounds/Tink.aiff" } else { "/System/Library/Sounds/Pop.aiff" };
        let _ = std::process::Command::new("afplay").arg(sound).spawn();
    }
}

pub fn copy(text: &str) -> Result<(), String> {
    arboard::Clipboard::new()
        .and_then(|mut c| c.set_text(text.to_string()))
        .map_err(|e| format!("Zwischenablage nicht erreichbar: {e}"))
}

pub fn deliver(text: &str, method: &str, restore: bool) -> Result<(), String> {
    match method {
        "none" => Ok(()),
        "type" => {
            os::wait_modifiers_released();
            os::type_text(text);
            Ok(())
        }
        _ => {
            os::wait_modifiers_released();
            let mut cb = arboard::Clipboard::new().map_err(|e| format!("Zwischenablage nicht erreichbar: {e}"))?;
            let prev_text = cb.get_text().ok();
            let prev_image = if prev_text.is_none() { cb.get_image().ok() } else { None };
            cb.set_text(text.to_string()).map_err(|e| format!("Zwischenablage nicht erreichbar: {e}"))?;
            std::thread::sleep(Duration::from_millis(60));
            os::press_paste();
            if restore {
                // Der Zielanwendung Zeit geben, die Zwischenablage zu lesen.
                std::thread::sleep(Duration::from_millis(250));
                if let Some(t) = prev_text {
                    let _ = cb.set_text(t);
                } else if let Some(img) = prev_image {
                    let _ = cb.set_image(img);
                }
            }
            Ok(())
        }
    }
}

pub fn play(start: bool) {
    os::play(start)
}
