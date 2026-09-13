//! Only modifier state and the T key are tracked; no typed text is retained.
#[derive(Clone, Copy)]
pub enum Key {
    WinLeft,
    WinRight,
    ShiftLeft,
    ShiftRight,
    ControlLeft,
    ControlRight,
    AltLeft,
    AltRight,
    T,
    Other,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    QuickCopy,
    Editor,
    RecordQuick,
    RecordEditor,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Decision {
    Pass,
    Suppress(Option<Action>),
}
#[derive(Default)]
pub struct ShortcutKeys {
    win: u8,
    shift: u8,
    control: u8,
    alt: u8,
    t_down: bool,
    captured: bool,
}
impl ShortcutKeys {
    pub fn resynchronize(&mut self, pressed: impl IntoIterator<Item = Key>) {
        let captured = self.captured;
        *self = Self::default();
        for key in pressed {
            self.update(key, true, false, false, false);
        }
        // Preserve suppression if the original chord is still physically held.
        self.captured = captured && self.t_down;
    }

    pub fn update(
        &mut self,
        key: Key,
        down: bool,
        enabled: bool,
        editor_default: bool,
        recording: bool,
    ) -> Decision {
        let modifier = match key {
            Key::WinLeft => Some((&mut self.win, 1)),
            Key::WinRight => Some((&mut self.win, 2)),
            Key::ShiftLeft => Some((&mut self.shift, 1)),
            Key::ShiftRight => Some((&mut self.shift, 2)),
            Key::ControlLeft => Some((&mut self.control, 1)),
            Key::ControlRight => Some((&mut self.control, 2)),
            Key::AltLeft => Some((&mut self.alt, 1)),
            Key::AltRight => Some((&mut self.alt, 2)),
            _ => None,
        };
        if let Some((state, bit)) = modifier {
            if down {
                *state |= bit;
            } else {
                *state &= !bit;
            }
        }
        if !matches!(key, Key::T) {
            return Decision::Pass;
        }
        if !down {
            self.t_down = false;
            return if std::mem::take(&mut self.captured) {
                Decision::Suppress(None)
            } else {
                Decision::Pass
            };
        }
        if std::mem::replace(&mut self.t_down, true) {
            return if self.captured {
                Decision::Suppress(None)
            } else {
                Decision::Pass
            };
        }
        if (enabled || recording) && self.win != 0 && self.shift != 0 && self.alt == 0 {
            let action = match (self.control != 0, recording) {
                (false, true) => Action::RecordQuick,
                (true, true) => Action::RecordEditor,
                (false, false) => Action::QuickCopy,
                (true, false) if editor_default => Action::Editor,
                _ => return Decision::Pass,
            };
            self.captured = true;
            return Decision::Suppress(Some(action));
        }
        Decision::Pass
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn key(keys: &mut ShortcutKeys, key: Key, down: bool) -> Decision {
        keys.update(key, down, true, true, false)
    }
    #[test]
    fn exact_chords_route_quick_copy_and_editor() {
        let mut keys = ShortcutKeys::default();
        key(&mut keys, Key::WinLeft, true);
        key(&mut keys, Key::ShiftRight, true);
        assert_eq!(
            key(&mut keys, Key::T, true),
            Decision::Suppress(Some(Action::QuickCopy))
        );
        assert_eq!(key(&mut keys, Key::T, true), Decision::Suppress(None));
        assert_eq!(key(&mut keys, Key::T, false), Decision::Suppress(None));
        key(&mut keys, Key::ControlLeft, true);
        assert_eq!(
            key(&mut keys, Key::T, true),
            Decision::Suppress(Some(Action::Editor))
        );
        key(&mut keys, Key::T, false);
        key(&mut keys, Key::AltLeft, true);
        assert_eq!(key(&mut keys, Key::T, true), Decision::Pass);
    }
    #[test]
    fn disabled_and_unrelated_keys_pass_through_and_keyup_is_balanced() {
        let mut keys = ShortcutKeys::default();
        key(&mut keys, Key::WinRight, true);
        key(&mut keys, Key::ShiftLeft, true);
        assert_eq!(
            keys.update(Key::T, true, false, true, false),
            Decision::Pass
        );
        key(&mut keys, Key::T, false);
        assert_eq!(key(&mut keys, Key::Other, true), Decision::Pass);
        key(&mut keys, Key::T, true);
        assert_eq!(
            keys.update(Key::T, false, false, true, false),
            Decision::Suppress(None)
        );
        key(&mut keys, Key::ControlRight, true);
        key(&mut keys, Key::AltRight, true);
        assert_eq!(key(&mut keys, Key::T, true), Decision::Pass);
    }
    #[test]
    fn recording_consumes_reserved_keys_without_starting_capture() {
        let mut keys = ShortcutKeys::default();
        key(&mut keys, Key::WinLeft, true);
        key(&mut keys, Key::WinRight, true);
        key(&mut keys, Key::WinLeft, false);
        key(&mut keys, Key::ShiftLeft, true);
        assert_eq!(
            keys.update(Key::T, true, false, true, true),
            Decision::Suppress(Some(Action::RecordQuick))
        );
        key(&mut keys, Key::T, false);
        key(&mut keys, Key::ControlRight, true);
        assert_eq!(
            keys.update(Key::T, true, false, true, true),
            Decision::Suppress(Some(Action::RecordEditor))
        );
    }

    #[test]
    fn closing_capture_recovers_releases_consumed_by_the_overlay() {
        let mut keys = ShortcutKeys::default();
        key(&mut keys, Key::WinLeft, true);
        key(&mut keys, Key::ShiftLeft, true);
        key(&mut keys, Key::ControlLeft, true);
        key(&mut keys, Key::T, true);
        // The focused selector handled key-up. The hook must recover before
        // another chord, including changing from the editor to Quick Copy.
        keys.resynchronize([]);
        key(&mut keys, Key::WinLeft, true);
        key(&mut keys, Key::ShiftLeft, true);
        assert_eq!(
            key(&mut keys, Key::T, true),
            Decision::Suppress(Some(Action::QuickCopy))
        );
        keys.resynchronize([Key::WinLeft, Key::ShiftLeft, Key::T]);
        assert_eq!(key(&mut keys, Key::T, true), Decision::Suppress(None));
        assert_eq!(key(&mut keys, Key::T, false), Decision::Suppress(None));
    }
}
