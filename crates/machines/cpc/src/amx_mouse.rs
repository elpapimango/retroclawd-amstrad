//! AMX-style mouse on the CPC's joystick-0 port (keyboard matrix line 9).
//!
//! What the sources establish (CPCWiki's "AMX Mouse" page -- the page itself is
//! blocked from this environment, so this is its description as quoted in
//! search results -- cross-checked against konCePCja's `amx_mouse.cpp`, see
//! `CREDITS.md`):
//!
//! * The interface turns the mouse's movement signals into **joystick 0**
//!   signals: a **LOW pulse on a direction line per mickey** (Up = line 9 bit 0,
//!   Down = bit 1, Left = bit 2, Right = bit 3).
//! * The hardware has **no motion counters**: software has to catch the pulses
//!   itself. AMX Art does it from the CPC's 300 Hz interrupt, which caps the
//!   resolution at "max 300 mickeys per second (or actually less, due to the
//!   missed IRQs)".
//! * Buttons are active-low on the fire lines: right button = Fire1 (bit 5),
//!   left button = Fire2 (bit 4), middle button = Fire3 (bit 6).
//!
//! What the sources do **not** give, and what is therefore this project's own
//! engineering choice: the pulse width and spacing. Each queued mickey becomes
//! one pulse of [`PULSE_PERIOD_TSTATES`] (exactly one 300 Hz polling interval)
//! that is low for all but a short [`PULSE_GAP_TSTATES`], so a 300 Hz poller
//! sees about one mickey per poll -- the documented 300/s ceiling -- while a
//! faster poller still sees distinct falling edges. Motion beyond
//! [`MAX_PENDING`] mickeys per axis is dropped rather than queued, so a flick of
//! the host mouse can't leave the pointer coasting for seconds afterwards.
//!
//! Because the mouse occupies the same port as digital joystick 1, it is opt-in
//! (`Cpc::set_amx_mouse`); while disabled it contributes nothing to the matrix.
//! Pulses and buttons are transient host input, so they are not saved in a
//! save state.

/// One pulse's length in Z80 T-states: a 300 Hz polling interval at 4 MHz.
pub(crate) const PULSE_PERIOD_TSTATES: u32 = 4_000_000 / 300;
/// How much of each period the line spends high, so back-to-back pulses are
/// separate falling edges.
pub(crate) const PULSE_GAP_TSTATES: u32 = PULSE_PERIOD_TSTATES / 20;
/// Per-axis cap on queued mickeys.
pub(crate) const MAX_PENDING: i16 = 255;

/// A host mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, Default)]
struct Axis {
    /// Net mickeys still to be sent (sign = direction), not counting a pulse
    /// already in flight.
    pending: i16,
    /// Whether a pulse is in flight, and which way it points (`-1`/`1`).
    active: bool,
    direction: i8,
    /// T-states into the current pulse.
    phase: u32,
}

impl Axis {
    fn push(&mut self, mickeys: i8) {
        self.pending = (self.pending + i16::from(mickeys)).clamp(-MAX_PENDING, MAX_PENDING);
        self.arm();
    }

    /// Starts the next pulse if the axis is idle and motion is queued.
    fn arm(&mut self) {
        if !self.active && self.pending != 0 {
            self.direction = self.pending.signum() as i8;
            self.pending -= i16::from(self.direction);
            self.active = true;
            self.phase = 0;
        }
    }

    fn advance(&mut self, mut tstates: u32) {
        while tstates > 0 && self.active {
            let step = tstates.min(PULSE_PERIOD_TSTATES - self.phase);
            self.phase += step;
            tstates -= step;
            if self.phase >= PULSE_PERIOD_TSTATES {
                self.active = false;
                self.arm();
            }
        }
    }

    /// The direction this axis is currently pulling low, if any.
    fn low(&self) -> Option<i8> {
        (self.active && self.phase < PULSE_PERIOD_TSTATES - PULSE_GAP_TSTATES)
            .then_some(self.direction)
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AmxMouse {
    enabled: bool,
    x: Axis,
    y: Axis,
    /// Active-high button state: bit 0 left, bit 1 right, bit 2 middle.
    buttons: u8,
}

impl AmxMouse {
    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            *self = Self::default();
        }
    }

    /// Relative host motion in mickeys; `dy < 0` is up. Ignored while disabled.
    pub(crate) fn motion(&mut self, dx: i8, dy: i8) {
        if self.enabled {
            self.x.push(dx);
            self.y.push(dy);
        }
    }

    pub(crate) fn button(&mut self, button: MouseButton, pressed: bool) {
        if !self.enabled {
            return;
        }
        let bit = match button {
            MouseButton::Left => 1 << 0,
            MouseButton::Right => 1 << 1,
            MouseButton::Middle => 1 << 2,
        };
        if pressed {
            self.buttons |= bit;
        } else {
            self.buttons &= !bit;
        }
    }

    /// Advances the pulse generators by `tstates` Z80 T-states.
    pub(crate) fn tick(&mut self, tstates: u32) {
        if self.enabled {
            self.x.advance(tstates);
            self.y.advance(tstates);
        }
    }

    /// What the interface pulls low on matrix line 9, as an active-low mask to
    /// AND into the line (`0xFF` = nothing).
    pub(crate) fn line9_mask(&self) -> u8 {
        let mut mask = 0xFF;
        match self.y.low() {
            Some(d) if d < 0 => mask &= !(1 << 0), // up
            Some(_) => mask &= !(1 << 1),          // down
            None => {}
        }
        match self.x.low() {
            Some(d) if d < 0 => mask &= !(1 << 2), // left
            Some(_) => mask &= !(1 << 3),          // right
            None => {}
        }
        if self.buttons & 0b001 != 0 {
            mask &= !(1 << 4); // left button -> Fire2
        }
        if self.buttons & 0b010 != 0 {
            mask &= !(1 << 5); // right button -> Fire1
        }
        if self.buttons & 0b100 != 0 {
            mask &= !(1 << 6); // middle button -> Fire3
        }
        mask
    }

    /// Mickeys not yet delivered, including a pulse in flight: `(x, y)`.
    #[cfg(test)]
    pub(crate) fn outstanding(&self) -> (i16, i16) {
        let count = |axis: &Axis| {
            axis.pending
                + if axis.active {
                    i16::from(axis.direction)
                } else {
                    0
                }
        };
        (count(&self.x), count(&self.y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled() -> AmxMouse {
        let mut mouse = AmxMouse::default();
        mouse.set_enabled(true);
        mouse
    }

    const LOW_TSTATES: u32 = PULSE_PERIOD_TSTATES - PULSE_GAP_TSTATES;

    #[test]
    fn a_disabled_mouse_contributes_nothing() {
        let mut mouse = AmxMouse::default();
        mouse.motion(5, -5);
        mouse.button(MouseButton::Left, true);
        mouse.tick(10_000);
        assert_eq!(mouse.line9_mask(), 0xFF);
        assert_eq!(mouse.outstanding(), (0, 0));
    }

    #[test]
    fn direction_bits_follow_the_documented_line_9_layout() {
        for (dx, dy, low_bit) in [(0, -1, 0), (0, 1, 1), (-1, 0, 2), (1, 0, 3)] {
            let mut mouse = enabled();
            mouse.motion(dx, dy);
            assert_eq!(
                mouse.line9_mask(),
                !(1u8 << low_bit),
                "dx={dx} dy={dy}: only bit {low_bit} should be pulled low"
            );
        }
    }

    #[test]
    fn one_mickey_is_exactly_one_low_pulse_then_idle() {
        let mut mouse = enabled();
        mouse.motion(1, 0);
        assert_eq!(mouse.line9_mask() & 0b1000, 0, "pulse starts low at once");

        mouse.tick(LOW_TSTATES - 1);
        assert_eq!(
            mouse.line9_mask() & 0b1000,
            0,
            "still low just before the gap"
        );
        mouse.tick(1);
        assert_ne!(mouse.line9_mask() & 0b1000, 0, "high during the gap");

        mouse.tick(PULSE_GAP_TSTATES);
        assert_eq!(mouse.line9_mask(), 0xFF, "idle once the pulse is over");
        assert_eq!(mouse.outstanding(), (0, 0));
        mouse.tick(PULSE_PERIOD_TSTATES * 3);
        assert_eq!(mouse.line9_mask(), 0xFF, "and it stays idle");
    }

    #[test]
    fn n_mickeys_are_n_separate_pulses_with_a_high_gap_between() {
        let mut mouse = enabled();
        mouse.motion(0, 3);
        let mut falling_edges = 0;
        let mut was_low = false;
        // Sample finely enough to see every gap.
        for _ in 0..(PULSE_PERIOD_TSTATES * 4 / 100) {
            let low = mouse.line9_mask() & 0b10 == 0;
            if low && !was_low {
                falling_edges += 1;
            }
            was_low = low;
            mouse.tick(100);
        }
        assert_eq!(falling_edges, 3);
        assert_eq!(mouse.line9_mask(), 0xFF);
    }

    #[test]
    fn opposite_motion_cancels_in_the_queue() {
        let mut mouse = enabled();
        mouse.motion(4, 0);
        mouse.motion(-4, 0);
        // One pulse (+1) was already in flight when the cancelling motion
        // arrived, so the queue now holds -1: the net is zero mickeys, though
        // the pointer still gets one pulse each way.
        assert_eq!(mouse.outstanding().0, 0);
    }

    #[test]
    fn queued_motion_is_capped_per_axis() {
        let mut mouse = enabled();
        for _ in 0..10 {
            mouse.motion(127, -127);
        }
        let (x, y) = mouse.outstanding();
        // The queue is capped; a pulse already in flight is on top of that.
        assert_eq!(x, MAX_PENDING + 1, "x capped");
        assert_eq!(y, -(MAX_PENDING + 1), "y capped");
    }

    #[test]
    fn buttons_map_to_the_fire_lines_and_release_cleanly() {
        let mut mouse = enabled();
        mouse.button(MouseButton::Left, true);
        assert_eq!(mouse.line9_mask(), !(1 << 4), "left = Fire2");
        mouse.button(MouseButton::Right, true);
        assert_eq!(mouse.line9_mask(), !((1 << 4) | (1 << 5)), "right = Fire1");
        mouse.button(MouseButton::Middle, true);
        assert_eq!(
            mouse.line9_mask(),
            !((1 << 4) | (1 << 5) | (1 << 6)),
            "middle = Fire3"
        );
        mouse.button(MouseButton::Left, false);
        mouse.button(MouseButton::Right, false);
        mouse.button(MouseButton::Middle, false);
        assert_eq!(mouse.line9_mask(), 0xFF);
    }

    #[test]
    fn disabling_clears_pending_motion_and_buttons() {
        let mut mouse = enabled();
        mouse.motion(9, 9);
        mouse.button(MouseButton::Left, true);
        mouse.set_enabled(false);
        mouse.set_enabled(true);
        assert_eq!(mouse.line9_mask(), 0xFF);
        assert_eq!(mouse.outstanding(), (0, 0));
    }

    #[test]
    fn a_long_tick_drains_the_whole_queue() {
        let mut mouse = enabled();
        mouse.motion(10, -10);
        mouse.tick(PULSE_PERIOD_TSTATES * 11);
        assert_eq!(mouse.outstanding(), (0, 0));
        assert_eq!(mouse.line9_mask(), 0xFF);
    }
}
