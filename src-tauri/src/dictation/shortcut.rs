//! Edge detection for a single physical modifier. The native adapter supplies
//! Right Option independently of Left Option; Windows will use Right Alt.
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
}

impl HoldShortcut {
    pub fn new(already_down: bool) -> Self {
        Self {
            down: already_down,
            active: false,
        }
    }

    pub fn update(&mut self, down: bool, chord: bool) -> Option<Action> {
        let pressed = down && !self.down;
        self.down = down;
        // A chord remains cancelled until the physical modifier is released.
        if self.active && chord {
            self.active = false;
            return Some(Action::Cancel);
        }
        if !down && self.active {
            self.active = false;
            return Some(Action::Release);
        }
        if pressed && !chord {
            self.active = true;
            return Some(Action::Start);
        }
        None
    }

    pub fn interrupt(&mut self) -> Option<Action> {
        let active = self.active;
        self.active = false;
        // Do not retrigger a key still held across a sleep/permission change.
        active.then_some(Action::Cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hold_starts_once_and_release_finishes_once() {
        let mut key = HoldShortcut::default();
        assert_eq!(key.update(false, false), None); // Left Option is not Right Option.
        assert_eq!(key.update(true, false), Some(Action::Start));
        assert_eq!(key.update(true, false), None);
        assert_eq!(key.update(false, false), Some(Action::Release));
        assert_eq!(key.update(false, false), None);
    }

    #[test]
    fn chords_cancel_without_committing_or_restarting_until_release() {
        let mut key = HoldShortcut::default();
        assert_eq!(key.update(true, true), None); // Other modifier already held.
        assert_eq!(key.update(true, false), None);
        assert_eq!(key.update(false, false), None);
        assert_eq!(key.update(true, false), Some(Action::Start));
        assert_eq!(key.update(true, true), Some(Action::Cancel)); // Option + letter.
        assert_eq!(key.update(true, false), None);
        assert_eq!(key.update(false, false), None);
        assert_eq!(key.update(true, false), Some(Action::Start));
    }

    #[test]
    fn enabling_while_held_or_interrupting_never_creates_a_phantom_press() {
        let mut key = HoldShortcut::new(true);
        assert_eq!(key.update(true, false), None);
        assert_eq!(key.update(false, false), None);
        assert_eq!(key.update(true, false), Some(Action::Start));
        assert_eq!(key.interrupt(), Some(Action::Cancel));
        assert_eq!(key.update(true, false), None);
        assert_eq!(key.update(false, false), None);
        assert_eq!(key.update(true, false), Some(Action::Start));
    }
}
