//! Records the CPC's cassette output line (the PPI's Port C bit 5, "MIC")
//! into timed pulses, so a program's `SAVE` can be written back out as a
//! tape file (`.cdt`/`.tzx` or `.voc`, see `zx_tape::write_tzx`/`write_voc`).
//!
//! Only time spent with the cassette motor running is recorded: a real tape
//! doesn't advance while the motor is off. The line's idle level before the
//! first edge isn't recorded (the tape starts at the first transition), and
//! each later motor-on session is separated from the previous one by
//! [`SESSION_GAP_TSTATES`] of silence -- the gap a real tape would have.

/// 4 MHz T-states of silence inserted between motor sessions: one second.
const SESSION_GAP_TSTATES: u64 = 4_000_000;
/// Safety bound on recorded pulses (a 64K `SAVE` is around a million); past
/// it recording stops rather than growing without limit.
const MAX_PULSES: usize = 4_000_000;
/// The CPU clock the durations are measured in, and the 3.5469 MHz clock
/// `zx_tape` expresses pulse lengths in.
const CPC_CLOCK_HZ: u64 = 4_000_000;
const TAPE_CLOCK_HZ: u64 = 3_546_900;

#[derive(Debug, Clone, Default)]
pub(crate) struct TapeRecorder {
    /// MIC level right now.
    level: bool,
    motor_on: bool,
    /// Absolute T-state at which the current pulse began; meaningful only
    /// while the motor runs.
    since: u64,
    /// Whether the first edge of the recording has been seen (the idle level
    /// before it isn't a pulse).
    started: bool,
    /// Completed pulses: `(duration in 4 MHz T-states, level held)`.
    pulses: Vec<(u64, bool)>,
}

impl TapeRecorder {
    fn push(&mut self, duration: u64, level: bool) {
        if duration > 0 && self.pulses.len() < MAX_PULSES {
            self.pulses.push((duration, level));
        }
    }

    /// The motor relay switched at absolute T-state `now`.
    pub(crate) fn motor(&mut self, now: u64, on: bool) {
        if on == self.motor_on {
            return;
        }
        self.motor_on = on;
        if on {
            // A later session: leave a tape's worth of gap after the last.
            if self.started {
                self.push(SESSION_GAP_TSTATES, false);
            }
            self.since = now;
        } else if self.started {
            self.push(now.saturating_sub(self.since), self.level);
        }
    }

    /// The MIC line now reads `level` at absolute T-state `now` (a Port C
    /// write, or a control-port bit-set/reset that changed bit 5).
    pub(crate) fn mic(&mut self, now: u64, level: bool) {
        if level == self.level {
            return;
        }
        if self.motor_on {
            if self.started {
                self.push(now.saturating_sub(self.since), self.level);
            }
            // The stretch before the very first edge is idle line, not tape.
            self.started = true;
            self.since = now;
        }
        self.level = level;
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.pulses.is_empty()
    }

    pub(crate) fn pulse_count(&self) -> usize {
        self.pulses.len()
    }

    pub(crate) fn clear(&mut self) {
        *self = Self {
            level: self.level,
            motor_on: self.motor_on,
            since: self.since,
            ..Self::default()
        };
    }

    /// The recording as a tape (durations converted from the CPU clock to
    /// `zx_tape`'s, carrying the sub-T-state remainder so long recordings
    /// don't drift), or `None` when nothing was recorded.
    pub(crate) fn to_tape(&self) -> Option<zx_tape::Tape> {
        if self.pulses.is_empty() {
            return None;
        }
        let mut remainder = 0u64;
        let pulses = self.pulses.iter().map(|&(duration, level)| {
            let scaled = duration * TAPE_CLOCK_HZ + remainder;
            remainder = scaled % CPC_CLOCK_HZ;
            zx_tape::Pulse {
                cycles: u32::try_from(scaled / CPC_CLOCK_HZ).unwrap_or(u32::MAX),
                level,
            }
        });
        Some(zx_tape::Tape::from_pulses(pulses))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recorded(recorder: &TapeRecorder) -> Vec<(u64, bool)> {
        recorder.pulses.clone()
    }

    fn edges_with_the_motor_off_record_nothing() {
        let mut recorder = TapeRecorder::default();
        recorder.mic(100, true);
        recorder.mic(200, false);
        assert!(recorder.is_empty());
        assert!(recorder.to_tape().is_none());
    }

    #[test]
    fn pulses_are_measured_between_edges_and_the_idle_lead_in_is_dropped() {
        let mut recorder = TapeRecorder::default();
        recorder.motor(1_000, true);
        recorder.mic(5_000, true); // first edge: the 4000 T-states of idle before it are not tape
        recorder.mic(5_800, false);
        recorder.mic(7_400, true);
        recorder.motor(8_000, false);
        // high 800, low 1600, then the closing high pulse 600 up to motor-off.
        assert_eq!(
            recorded(&recorder),
            vec![(800, true), (1_600, false), (600, true)]
        );
    }

    fn time_with_the_motor_off_is_not_tape_and_sessions_get_a_one_second_gap() {
        let mut recorder = TapeRecorder::default();
        recorder.motor(0, true);
        recorder.mic(100, true);
        recorder.mic(300, false);
        recorder.motor(500, false);
        // An hour passes with the motor off.
        recorder.motor(14_400_000_000, true);
        recorder.mic(14_400_000_250, true);
        recorder.mic(14_400_000_650, false);
        let pulses = recorded(&recorder);
        assert_eq!(
            pulses,
            vec![
                (200, true),
                (200, false),
                (SESSION_GAP_TSTATES, false),
                // The line stayed low through the gap and the idle stretch
                // before the second session's first edge.
                (250, false),
                (400, true),
            ]
        );
    }
    /// Merged (every check kept): `edges_with_the_motor_off_record_nothing`, `time_with_the_motor_off_is_not_tape_and_sessions_get_a_one_second_gap`.
    #[test]
    fn recorder_motor_rules() {
        edges_with_the_motor_off_record_nothing();
        time_with_the_motor_off_is_not_tape_and_sessions_get_a_one_second_gap();
    }

    fn conversion_to_the_tape_clock_keeps_total_time() {
        let mut recorder = TapeRecorder::default();
        recorder.motor(0, true);
        recorder.mic(10, true);
        // 100_000 pulses of 1001 T-states: the per-pulse rounding must not
        // accumulate.
        let mut now = 10;
        for i in 0..100_000 {
            now += 1_001;
            recorder.mic(now, i % 2 == 1);
        }
        let tape = recorder.to_tape().unwrap();
        let total: u64 = tape.total_cycles();
        let expected = 100_000u64 * 1_001 * 3_546_900 / 4_000_000;
        assert!(total.abs_diff(expected) <= 1, "{total} vs {expected}");
    }

    fn clearing_forgets_the_pulses_but_not_the_line_state() {
        let mut recorder = TapeRecorder::default();
        recorder.motor(0, true);
        recorder.mic(10, true);
        recorder.mic(30, false);
        assert!(!recorder.is_empty());
        recorder.clear();
        assert!(recorder.is_empty());
        // Still running: the next edge records again, and (a fresh
        // recording) the stretch up to it is idle lead-in.
        recorder.mic(100, true);
        recorder.mic(150, false);
        assert_eq!(recorded(&recorder), vec![(50, true)]);
    }
    /// Merged (every check kept): `conversion_to_the_tape_clock_keeps_total_time`, `clearing_forgets_the_pulses_but_not_the_line_state`.
    #[test]
    fn recorder_conversion_and_clear() {
        conversion_to_the_tape_clock_keeps_total_time();
        clearing_forgets_the_pulses_but_not_the_line_state();
    }
}
