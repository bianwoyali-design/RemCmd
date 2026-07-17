use gpui::{Keystroke, Modifiers};
use remcmd_terminal::TerminalModes;

pub(crate) fn encode_key(keystroke: &Keystroke, modes: TerminalModes) -> Option<Vec<u8>> {
    if keystroke.modifiers.platform {
        return None;
    }

    if keystroke.modifiers.control
        && let Some(byte) = control_byte(&keystroke.key, keystroke.modifiers.shift)
    {
        return Some(with_alt_prefix(vec![byte], keystroke.modifiers.alt));
    }

    let modifiers = keystroke.modifiers;
    let bytes = match keystroke.key.as_str() {
        "enter" => {
            if modes.contains(TerminalModes::LINE_FEED_NEW_LINE) {
                b"\r\n".to_vec()
            } else {
                b"\r".to_vec()
            }
        }
        "backspace" => vec![0x7f],
        "tab" if modifiers.shift => b"\x1b[Z".to_vec(),
        "tab" => b"\t".to_vec(),
        "escape" => b"\x1b".to_vec(),
        "up" => return Some(cursor_key(b'A', modes, modifiers)),
        "down" => return Some(cursor_key(b'B', modes, modifiers)),
        "right" => return Some(cursor_key(b'C', modes, modifiers)),
        "left" => return Some(cursor_key(b'D', modes, modifiers)),
        "home" => return Some(cursor_key(b'H', modes, modifiers)),
        "end" => return Some(cursor_key(b'F', modes, modifiers)),
        "insert" => return Some(tilde_key(2, modifiers)),
        "delete" => return Some(tilde_key(3, modifiers)),
        "pageup" => return Some(tilde_key(5, modifiers)),
        "pagedown" => return Some(tilde_key(6, modifiers)),
        "f1" => return Some(function_key(b'P', modifiers)),
        "f2" => return Some(function_key(b'Q', modifiers)),
        "f3" => return Some(function_key(b'R', modifiers)),
        "f4" => return Some(function_key(b'S', modifiers)),
        "f5" => return Some(tilde_key(15, modifiers)),
        "f6" => return Some(tilde_key(17, modifiers)),
        "f7" => return Some(tilde_key(18, modifiers)),
        "f8" => return Some(tilde_key(19, modifiers)),
        "f9" => return Some(tilde_key(20, modifiers)),
        "f10" => return Some(tilde_key(21, modifiers)),
        "f11" => return Some(tilde_key(23, modifiers)),
        "f12" => return Some(tilde_key(24, modifiers)),
        _ if modifiers.alt => {
            // Terminal Meta uses the base key; macOS Option may put a symbol in key_char.
            let text = (keystroke.key.chars().count() == 1).then(|| {
                if modifiers.shift {
                    keystroke.key.to_uppercase()
                } else {
                    keystroke.key.clone()
                }
            })?;
            let mut bytes = Vec::with_capacity(text.len() + 1);
            bytes.push(0x1b);
            bytes.extend_from_slice(text.as_bytes());
            return Some(bytes);
        }
        _ => return None,
    };

    Some(with_alt_prefix(bytes, modifiers.alt))
}

pub(crate) fn encode_paste(text: &str, modes: TerminalModes) -> Vec<u8> {
    if !modes.contains(TerminalModes::BRACKETED_PASTE) {
        return text.as_bytes().to_vec();
    }

    let mut bytes = Vec::with_capacity(text.len() + 12);
    bytes.extend_from_slice(b"\x1b[200~");
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(b"\x1b[201~");
    bytes
}

pub(crate) fn encode_focus(focused: bool, modes: TerminalModes) -> Option<Vec<u8>> {
    modes.contains(TerminalModes::FOCUS_REPORTING).then(|| {
        if focused {
            b"\x1b[I".to_vec()
        } else {
            b"\x1b[O".to_vec()
        }
    })
}

pub(crate) fn encode_alternate_scroll(lines: i32, modes: TerminalModes) -> Vec<u8> {
    let final_byte = if lines > 0 { b'A' } else { b'B' };
    let sequence = cursor_key(final_byte, modes, Modifiers::none());
    let repetitions = lines.unsigned_abs().min(64) as usize;

    sequence.repeat(repetitions)
}

fn cursor_key(final_byte: u8, modes: TerminalModes, modifiers: Modifiers) -> Vec<u8> {
    let modifier = modifier_parameter(modifiers);
    if modifier != 1 {
        return format!("\x1b[1;{modifier}{}", char::from(final_byte)).into_bytes();
    }

    let prefix = if modes.contains(TerminalModes::APPLICATION_CURSOR) {
        b"\x1bO".as_slice()
    } else {
        b"\x1b[".as_slice()
    };
    let mut bytes = Vec::with_capacity(prefix.len() + 1);
    bytes.extend_from_slice(prefix);
    bytes.push(final_byte);
    bytes
}

fn function_key(final_byte: u8, modifiers: Modifiers) -> Vec<u8> {
    let modifier = modifier_parameter(modifiers);
    if modifier == 1 {
        return vec![0x1b, b'O', final_byte];
    }

    format!("\x1b[1;{modifier}{}", char::from(final_byte)).into_bytes()
}

fn tilde_key(code: u8, modifiers: Modifiers) -> Vec<u8> {
    let modifier = modifier_parameter(modifiers);
    if modifier == 1 {
        format!("\x1b[{code}~").into_bytes()
    } else {
        format!("\x1b[{code};{modifier}~").into_bytes()
    }
}

fn modifier_parameter(modifiers: Modifiers) -> u8 {
    1 + u8::from(modifiers.shift)
        + (u8::from(modifiers.alt) * 2)
        + (u8::from(modifiers.control) * 4)
}

fn control_byte(key: &str, shifted: bool) -> Option<u8> {
    let key = key.as_bytes();
    if key.len() != 1 {
        return match key {
            b"space" => Some(0),
            _ => None,
        };
    }

    let key = if shifted {
        key[0].to_ascii_uppercase()
    } else {
        key[0]
    };

    match key {
        b'a'..=b'z' => Some(key - b'a' + 1),
        b'A'..=b'Z' => Some(key - b'A' + 1),
        b'@' | b'2' => Some(0),
        b'[' | b'3' => Some(0x1b),
        b'\\' | b'4' => Some(0x1c),
        b']' | b'5' => Some(0x1d),
        b'^' | b'6' => Some(0x1e),
        b'_' | b'7' | b'/' => Some(0x1f),
        b'?' | b'8' => Some(0x7f),
        _ => None,
    }
}

fn with_alt_prefix(mut bytes: Vec<u8>, alt: bool) -> Vec<u8> {
    if alt {
        bytes.insert(0, 0x1b);
    }
    bytes
}

#[cfg(test)]
mod tests {
    use gpui::Keystroke;

    use super::*;

    fn key(source: &str) -> Keystroke {
        Keystroke::parse(source).unwrap()
    }

    #[test]
    fn printable_text_is_left_for_the_platform_input_handler() {
        assert_eq!(encode_key(&key("a"), TerminalModes::NONE), None);
    }

    #[test]
    fn encodes_control_and_meta_characters() {
        assert_eq!(
            encode_key(&key("ctrl-c"), TerminalModes::NONE),
            Some(vec![3])
        );
        assert_eq!(
            encode_key(&key("ctrl-alt-c"), TerminalModes::NONE),
            Some(vec![0x1b, 3])
        );
        assert_eq!(
            encode_key(&key("alt-s->\u{df}"), TerminalModes::NONE),
            Some(b"\x1bs".to_vec())
        );
    }

    #[test]
    fn cursor_keys_respect_application_and_modifier_modes() {
        assert_eq!(
            encode_key(&key("up"), TerminalModes::NONE),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            encode_key(&key("up"), TerminalModes::APPLICATION_CURSOR),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            encode_key(&key("ctrl-shift-left"), TerminalModes::NONE),
            Some(b"\x1b[1;6D".to_vec())
        );
        assert_eq!(
            encode_key(&key("alt-up"), TerminalModes::NONE),
            Some(b"\x1b[1;3A".to_vec())
        );
    }

    #[test]
    fn paste_uses_bracketed_mode_when_requested() {
        assert_eq!(encode_paste("echo hi", TerminalModes::NONE), b"echo hi");
        assert_eq!(
            encode_paste("echo hi", TerminalModes::BRACKETED_PASTE),
            b"\x1b[200~echo hi\x1b[201~"
        );
    }

    #[test]
    fn focus_reports_only_when_the_terminal_enables_them() {
        assert_eq!(encode_focus(true, TerminalModes::NONE), None);
        assert_eq!(
            encode_focus(true, TerminalModes::FOCUS_REPORTING),
            Some(b"\x1b[I".to_vec())
        );
        assert_eq!(
            encode_focus(false, TerminalModes::FOCUS_REPORTING),
            Some(b"\x1b[O".to_vec())
        );
    }

    #[test]
    fn alternate_scroll_repeats_cursor_sequences() {
        assert_eq!(
            encode_alternate_scroll(2, TerminalModes::APPLICATION_CURSOR),
            b"\x1bOA\x1bOA"
        );
        assert_eq!(encode_alternate_scroll(-1, TerminalModes::NONE), b"\x1b[B");
    }
}
