//! Tap to toggle or hold a single physical modifier to dictate.
//! Timestamps come from native key events, so microphone startup cannot turn
//! a quick tap into a hold merely by delaying main-thread event processing.
#[derive(Debug, PartialEq)]
pub enum Action {
    Start,
    Release,
    Cancel,
}

#[derive(Default)]
pub struct HoldShortcut {
    down: bool,
    active: bool,
    latched: bool,
    pressed_at: f64,
}

impl HoldShortcut {
    pub fn new(already_down: bool) -> Self {
        Self {
            down: already_down,
            ..Self::default()
        }
    }

    pub fn update(&mut self, down: bool, chord: bool, timestamp: f64) -> Option<Action> {
        let pressed = down && !self.down;
        let released = !down && self.down;
        self.down = down;
        if pressed && self.latched {
            self.interrupt();
            return Some(Action::Release);
        }
        // Normal typing is allowed during hands-free dictation. A chord while
        // holding the activation key cancels and remains blocked until release.
        if self.active && !self.latched && chord {
            return self.interrupt();
        }
        if released && self.active && !self.latched {
            if timestamp - self.pressed_at >= 0.5 {
                self.interrupt();
                return Some(Action::Release);
            }
            self.latched = true;
        }
        if pressed && !chord {
            self.active = true;
            self.pressed_at = timestamp;
            return Some(Action::Start);
        }
        None
    }

    pub fn interrupt(&mut self) -> Option<Action> {
        let active = self.active;
        self.active = false;
        self.latched = false;
        // Preserve physical state so cancellation cannot retrigger a held key.
        active.then_some(Action::Cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_tap_latches_and_next_press_stops_once() {
        let mut key = HoldShortcut::default();
        assert_eq!(key.update(true, false, 1.0), Some(Action::Start));
        assert_eq!(key.update(false, false, 1.499), None);
        assert_eq!(key.update(false, true, 2.0), None); // Typing is allowed.
        assert_eq!(key.update(true, false, 3.0), Some(Action::Release));
        assert_eq!(key.update(true, false, 3.1), None);
        assert_eq!(key.update(false, false, 3.2), None);
        assert_eq!(key.update(true, false, 4.0), Some(Action::Start));
    }

    #[test]
    fn holds_finish_at_or_above_half_a_second() {
        for duration in [0.5, 0.501, 60.0] {
            let mut key = HoldShortcut::default();
            assert_eq!(key.update(true, false, 0.0), Some(Action::Start));
            assert_eq!(key.update(true, false, 0.1), None);
            assert_eq!(key.update(false, false, duration), Some(Action::Release));
            assert_eq!(key.update(false, false, duration), None);
        }
    }

    #[test]
    fn chords_cancel_and_block_until_release() {
        let mut key = HoldShortcut::default();
        assert_eq!(key.update(true, true, 0.0), None);
        assert_eq!(key.update(true, false, 0.1), None);
        assert_eq!(key.update(false, false, 0.2), None);
        assert_eq!(key.update(true, false, 1.0), Some(Action::Start));
        assert_eq!(key.update(true, true, 1.1), Some(Action::Cancel));
        assert_eq!(key.update(true, false, 1.2), None);
        assert_eq!(key.update(false, false, 1.3), None);
        assert_eq!(key.update(true, false, 2.0), Some(Action::Start));
    }

    #[test]
    fn enabling_held_and_interrupting_do_not_create_phantom_presses() {
        let mut key = HoldShortcut::new(true);
        assert_eq!(key.update(true, false, 0.0), None);
        assert_eq!(key.update(false, false, 0.1), None);
        assert_eq!(key.update(true, false, 1.0), Some(Action::Start));
        assert_eq!(key.interrupt(), Some(Action::Cancel));
        assert_eq!(key.update(true, false, 1.1), None);
        assert_eq!(key.update(false, false, 1.2), None);
        assert_eq!(key.update(true, false, 2.0), Some(Action::Start));
    }

    #[test]
    fn backend_completion_clears_hands_free_state() {
        let mut key = HoldShortcut::default();
        assert_eq!(key.update(true, false, 0.0), Some(Action::Start));
        assert_eq!(key.update(false, false, 0.1), None);
        assert_eq!(key.interrupt(), Some(Action::Cancel));
        assert_eq!(key.interrupt(), None);
        assert_eq!(key.update(true, false, 1.0), Some(Action::Start));
    }
}
