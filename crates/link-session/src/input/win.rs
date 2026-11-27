//! The phone as touchpad and keyboard through `SendInput`. Windows applies the user's pointer speed to relative motion,
//! as for a real touchpad, and silently drops input to a window of a higher integrity level (an elevated app).

#![allow(unsafe_code, reason = "FFI: user32 SendInput with arrays that live for the call")]

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput,
    VIRTUAL_KEY, VK_BACK, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F5, VK_HOME, VK_LCONTROL, VK_LEFT, VK_LMENU,
    VK_LSHIFT, VK_LWIN, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB, VK_UP, VkKeyScanW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::WHEEL_DELTA;

use super::{Action, Modifier, RETURN, TAB};

/// The phone scrolls in Wayland axis units, 15 to a wheel notch; Windows counts `WHEEL_DELTA` to a notch.
const AXIS_PER_NOTCH: f64 = 15.0;
/// keysyms of F1 to F12.
const F_KEYS: std::ops::RangeInclusive<u32> = 0xffbe..=0xffc9;
/// The keys the phone names, as (keysym, virtual key, extended); an extended key is not its numpad twin.
const KEYCODES: [(u32, VIRTUAL_KEY, bool); 19] = [
    (RETURN, VK_RETURN, false),
    (0xff08, VK_BACK, false),
    (0xffff, VK_DELETE, true),
    (0xff1b, VK_ESCAPE, false),
    (TAB, VK_TAB, false),
    (0x20, VK_SPACE, false),
    (0xff51, VK_LEFT, true),
    (0xff53, VK_RIGHT, true),
    (0xff52, VK_UP, true),
    (0xff54, VK_DOWN, true),
    (0xff50, VK_HOME, true),
    (0xff57, VK_END, true),
    (0xff55, VK_PRIOR, true),
    (0xff56, VK_NEXT, true),
    (0xffc2, VK_F5, false),
    (0xffeb, VK_LWIN, true),
    (0xffe3, VK_LCONTROL, false),
    (0xffe9, VK_LMENU, false),
    (0xffe1, VK_LSHIFT, false),
];

#[derive(Default)]
pub struct Win;

impl Win {
    #[expect(clippy::unused_self, reason = "the other backends' interface")]
    pub fn run(&mut self, action: Action) -> anyhow::Result<()> {
        let inputs = match action {
            Action::Move(dx, dy) => vec![mouse(MOUSEEVENTF_MOVE, round(dx), round(dy), 0)],
            Action::Point(_) => anyhow::bail!("no screen sharing on Windows, so no shared monitor to point at"),
            Action::Scroll(dx, dy) => {
                // A positive wheel value scrolls up and a positive Wayland axis value scrolls down; both go right.
                let notches = |axis: f64| round(axis * f64::from(WHEEL_DELTA) / AXIS_PER_NOTCH);
                let mut inputs = Vec::with_capacity(2);
                if dy != 0.0 {
                    inputs.push(mouse(MOUSEEVENTF_WHEEL, 0, 0, notches(-dy)));
                }
                if dx != 0.0 {
                    inputs.push(mouse(MOUSEEVENTF_HWHEEL, 0, 0, notches(dx)));
                }
                inputs
            }
            Action::Button { code, pressed } => vec![button(code, pressed)?],
            Action::Click(code) => vec![button(code, true)?, button(code, false)?],
            Action::Key(keysym) => {
                let (key, extended) = virtual_key(keysym).ok_or_else(|| anyhow::anyhow!("no key for {keysym:#x}"))?;
                vec![key_input(key, extended, true), key_input(key, extended, false)]
            }
            Action::Text(text) => text_inputs(&text),
            Action::Shortcut(modifiers, keysym) => shortcut(&modifiers, keysym)?,
        };
        send(&inputs);
        Ok(())
    }
}

/// Sends `inputs` as one batch, so the user's own input cannot interleave.
fn send(inputs: &[INPUT]) {
    let (Ok(count), Ok(size)) = (u32::try_from(inputs.len()), i32::try_from(size_of::<INPUT>())) else { return };
    // SAFETY: `inputs` holds `count` initialized INPUTs of `size` bytes for the call.
    let sent = unsafe { SendInput(count, inputs.as_ptr(), size) };
    // The secure desktop (lock screen, UAC) blocks input; the phone's next action tries again.
    if sent < count {
        log::warn!("Windows took {sent} of {count} input events: {}", std::io::Error::last_os_error());
    }
}

#[expect(clippy::cast_possible_truncation, reason = "a phone's motion or scroll is far inside i32")]
fn round(value: f64) -> i32 {
    value.round() as i32
}

fn mouse(flags: u32, dx: i32, dy: i32, data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            // `mouseData` carries a signed wheel value in its u32.
            mi: MOUSEINPUT { dx, dy, mouseData: data.cast_unsigned(), dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

/// An evdev button code's press or release.
fn button(code: u32, pressed: bool) -> anyhow::Result<INPUT> {
    let flags = match (code, pressed) {
        (0x110, true) => MOUSEEVENTF_LEFTDOWN,
        (0x110, false) => MOUSEEVENTF_LEFTUP,
        (0x111, true) => MOUSEEVENTF_RIGHTDOWN,
        (0x111, false) => MOUSEEVENTF_RIGHTUP,
        (0x112, true) => MOUSEEVENTF_MIDDLEDOWN,
        (0x112, false) => MOUSEEVENTF_MIDDLEUP,
        _ => anyhow::bail!("no mouse button {code:#x}"),
    };
    Ok(mouse(flags, 0, 0, 0))
}

fn keyboard(key: VIRTUAL_KEY, scan: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: key, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

fn key_input(key: VIRTUAL_KEY, extended: bool, pressed: bool) -> INPUT {
    let flags = if extended { KEYEVENTF_EXTENDEDKEY } else { 0 } | if pressed { 0 } else { KEYEVENTF_KEYUP };
    keyboard(key, 0, flags)
}

/// A key the phone names or F1 to F12, as (virtual key, extended).
fn virtual_key(keysym: u32) -> Option<(VIRTUAL_KEY, bool)> {
    if F_KEYS.contains(&keysym) {
        return Some((VK_F1 + u16::try_from(keysym - F_KEYS.start()).ok()?, false));
    }
    KEYCODES.iter().find(|(sym, _, _)| *sym == keysym).map(|&(_, key, extended)| (key, extended))
}

/// Each UTF-16 unit typed as a Unicode key; new lines and tabs press their keys, as apps read those from keys.
fn text_inputs(text: &str) -> Vec<INPUT> {
    let mut inputs = Vec::with_capacity(text.len() * 2);
    for c in text.chars() {
        match c {
            '\n' | '\r' => inputs.extend([key_input(VK_RETURN, false, true), key_input(VK_RETURN, false, false)]),
            '\t' => inputs.extend([key_input(VK_TAB, false, true), key_input(VK_TAB, false, false)]),
            _ => {
                let mut units = [0; 2];
                for &mut unit in c.encode_utf16(&mut units) {
                    inputs.extend([
                        keyboard(0, unit, KEYEVENTF_UNICODE),
                        keyboard(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
                    ]);
                }
            }
        }
    }
    inputs
}

/// The modifiers held, the key pressed and released, then the modifiers released in reverse order. A character key
/// is found in the current layout, which may add Shift, Ctrl or Alt (`+` is Shift and `=` on a US layout).
fn shortcut(modifiers: &[Modifier], keysym: u32) -> anyhow::Result<Vec<INPUT>> {
    let mut held: Vec<Modifier> = modifiers.to_vec();
    let (key, extended) =
        if let Some(key) = virtual_key(keysym) { key } else { (layout_key(keysym, &mut held)?, false) };
    let held: Vec<(VIRTUAL_KEY, bool)> = held.iter().map(|&modifier| modifier_key(modifier)).collect();
    let mut inputs: Vec<INPUT> = held.iter().map(|&(key, extended)| key_input(key, extended, true)).collect();
    inputs.extend([key_input(key, extended, true), key_input(key, extended, false)]);
    inputs.extend(held.iter().rev().map(|&(key, extended)| key_input(key, extended, false)));
    Ok(inputs)
}

/// The key that types Latin-1 `keysym` in the current layout; adds the modifiers it needs to `held`.
fn layout_key(keysym: u32, held: &mut Vec<Modifier>) -> anyhow::Result<VIRTUAL_KEY> {
    // Latin-1 keysyms are their code points; the phone's parser makes no other single-character keysym.
    let unit = u16::try_from(keysym).ok().filter(|&unit| unit <= 0xff);
    let unit = unit.ok_or_else(|| anyhow::anyhow!("no key for {keysym:#x}"))?;
    // SAFETY: a plain lookup in the current layout.
    let scan = unsafe { VkKeyScanW(unit) };
    anyhow::ensure!(scan != -1, "no key types {keysym:#x} in this keyboard layout");
    let [key, shift_state] = scan.to_le_bytes();
    for (bit, modifier) in [(1, Modifier::Shift), (2, Modifier::Ctrl), (4, Modifier::Alt)] {
        if shift_state & bit != 0 && !held.contains(&modifier) {
            held.push(modifier);
        }
    }
    Ok(VIRTUAL_KEY::from(key))
}

const fn modifier_key(modifier: Modifier) -> (VIRTUAL_KEY, bool) {
    match modifier {
        Modifier::Shift => (VK_LSHIFT, false),
        Modifier::Ctrl => (VK_LCONTROL, false),
        Modifier::Alt => (VK_LMENU, false),
        Modifier::Super => (VK_LWIN, true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::KEYS;

    #[test]
    fn every_key_the_phone_names_has_a_windows_key() {
        let missing: Vec<&str> =
            KEYS.iter().filter(|(_, keysym)| virtual_key(*keysym).is_none()).map(|(name, _)| *name).collect();
        assert!(missing.is_empty(), "no Windows key for {missing:?}");
    }
}
