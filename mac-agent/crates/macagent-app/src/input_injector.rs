//! Mac InputInjector — CGEvent click/scroll/keyboard for supervised windows.

use macagent_core::ctrl_msg::KeyMod;

/// Global-screen rectangle of a supervised window (top-left origin, points).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowFrame {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Carbon virtual keycodes for ANSI US layout. See HIToolbox/Events.h.
const KEYCODES: &[(&str, u16)] = &[
    // Letters
    ("a", 0x00), ("s", 0x01), ("d", 0x02), ("f", 0x03), ("h", 0x04),
    ("g", 0x05), ("z", 0x06), ("x", 0x07), ("c", 0x08), ("v", 0x09),
    ("b", 0x0B), ("q", 0x0C), ("w", 0x0D), ("e", 0x0E), ("r", 0x0F),
    ("y", 0x10), ("t", 0x11),
    ("o", 0x1F), ("u", 0x20), ("i", 0x22), ("p", 0x23),
    ("l", 0x25), ("j", 0x26), ("k", 0x28),
    ("n", 0x2D), ("m", 0x2E),
    // Numbers
    ("1", 0x12), ("2", 0x13), ("3", 0x14), ("4", 0x15),
    ("6", 0x16), ("5", 0x17), ("9", 0x19), ("7", 0x1A),
    ("8", 0x1C), ("0", 0x1D),
    // Symbols (US ANSI)
    ("=", 0x18), ("-", 0x1B), ("]", 0x1E), ("[", 0x21),
    ("'", 0x27), (";", 0x29), ("\\", 0x2A), (",", 0x2B),
    ("/", 0x2C), (".", 0x2F), ("`", 0x32),
    // Whitespace / control
    ("space", 0x31), ("return", 0x24), ("tab", 0x30),
    ("delete", 0x33), ("esc", 0x35),
    // Arrows
    ("left", 0x7B), ("right", 0x7C), ("down", 0x7D), ("up", 0x7E),
];

/// Resolve a Carbon virtual keycode for a logical key name (ANSI US layout).
pub fn lookup_keycode(name: &str) -> Option<u16> {
    if name.is_empty() {
        return None;
    }
    KEYCODES.iter().find(|(n, _)| *n == name).map(|(_, k)| *k)
}

/// Map normalized [0,1] window-relative coords to global-screen integer points.
pub fn normalize_to_global(frame: &WindowFrame, x: f32, y: f32) -> (i32, i32) {
    let gx = frame.x + (x.clamp(0.0, 1.0) * frame.w as f32) as i32;
    let gy = frame.y + (y.clamp(0.0, 1.0) * frame.h as f32) as i32;
    (gx, gy)
}

/// Pack a `KeyMod` slice into CGEventFlags bits.
pub fn pack_modifier_flags(mods: &[KeyMod]) -> u64 {
    let mut out: u64 = 0;
    for m in mods {
        out |= match m {
            KeyMod::Cmd   => 1 << 20,  // kCGEventFlagMaskCommand
            KeyMod::Shift => 1 << 17,  // kCGEventFlagMaskShift
            KeyMod::Opt   => 1 << 19,  // kCGEventFlagMaskAlternate
            KeyMod::Ctrl  => 1 << 18,  // kCGEventFlagMaskControl
        };
    }
    out
}

/// Split text into chunks of at most `max_chars` characters.
/// CGEventKeyboardSetUnicodeString limits each call to ~20 UTF-16 units in practice.
pub fn chunk_unicode(text: &str, max_chars: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut count = 0usize;
    for ch in text.chars() {
        buf.push(ch);
        count += 1;
        if count >= max_chars {
            out.push(std::mem::take(&mut buf));
            count = 0;
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use macagent_core::ctrl_msg::KeyMod;

    #[test]
    fn keycode_table_known() {
        assert_eq!(lookup_keycode("a"), Some(0x00));
        assert_eq!(lookup_keycode("="), Some(0x18));
        assert_eq!(lookup_keycode("-"), Some(0x1B));
        assert_eq!(lookup_keycode("0"), Some(0x1D));
        assert_eq!(lookup_keycode("esc"), Some(0x35));
        assert_eq!(lookup_keycode("return"), Some(0x24));
        assert_eq!(lookup_keycode("up"), Some(0x7E));
    }

    #[test]
    fn keycode_table_unknown() {
        assert_eq!(lookup_keycode("zzz"), None);
        assert_eq!(lookup_keycode(""), None);
    }

    #[test]
    fn normalize_coords_center() {
        let frame = WindowFrame { x: 100, y: 200, w: 800, h: 600 };
        assert_eq!(normalize_to_global(&frame, 0.5, 0.5), (500, 500));
        assert_eq!(normalize_to_global(&frame, 0.0, 0.0), (100, 200));
        assert_eq!(normalize_to_global(&frame, 1.0, 1.0), (900, 800));
    }

    #[test]
    fn modifier_flags_packing() {
        let cmd_shift = pack_modifier_flags(&[KeyMod::Cmd, KeyMod::Shift]);
        // CGEventFlags::CGEventFlagCommand = 1<<20, CGEventFlagShift = 1<<17
        assert_eq!(cmd_shift, (1u64 << 20) | (1u64 << 17));
        let none = pack_modifier_flags(&[]);
        assert_eq!(none, 0);
        let opt_ctrl = pack_modifier_flags(&[KeyMod::Opt, KeyMod::Ctrl]);
        // CGEventFlagAlternate=1<<19, CGEventFlagControl=1<<18
        assert_eq!(opt_ctrl, (1u64 << 19) | (1u64 << 18));
    }

    #[test]
    fn chunk_unicode_text_basic() {
        // ASCII, well under chunk limit
        let chunks = chunk_unicode("hello", 20);
        assert_eq!(chunks, vec!["hello"]);
        // Exactly at limit
        let s20: String = "a".repeat(20);
        assert_eq!(chunk_unicode(&s20, 20), vec![s20.clone()]);
        // Over limit: split into two
        let s30: String = "a".repeat(30);
        let chunks = chunk_unicode(&s30, 20);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 20);
        assert_eq!(chunks[1].len(), 10);
    }

    #[test]
    fn chunk_unicode_chinese() {
        // 50 Chinese chars, chunk size 20 (UTF-16 units; Chinese in BMP is 1 unit each)
        let zh: String = "中".repeat(50);
        let chunks = chunk_unicode(&zh, 20);
        assert_eq!(chunks.len(), 3);
        // First two chunks have 20 chars each
        assert_eq!(chunks[0].chars().count(), 20);
        assert_eq!(chunks[1].chars().count(), 20);
        assert_eq!(chunks[2].chars().count(), 10);
    }
}
