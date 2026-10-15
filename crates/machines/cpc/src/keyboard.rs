//! Real Amstrad CPC 10x8 keyboard matrix, scanned through the PPI (Port C
//! bits 3:0 select one of 10 real matrix lines via a 74LS145 decoder,
//! Port A reads the selected line's 8 key states back -- via the AY's
//! own register 14, not a direct PPI pin, see `crate::bus::CpcBus::
//! write_port_c`'s own doc comment). Genuinely different shape from
//! `sinclair-keyboard` (an 8x5 Z80-`IN`-port design) and from `c64::
//! keyboard` (an 8x8 CIA-Port-A/B design) -- kept local here rather than
//! forcing a shared crate from a third, differently-shaped example.
//!
//! Matrix layout (line, bit) per key -- the real, complete CPC keyboard
//! matrix table, fetched directly from CPCTech's own keyboard
//! documentation <https://cpctech.cpcwiki.de/docs/keyboard.html> (see
//! `CREDITS.md`), not guessed. Active low on real hardware (a `0` bit
//! means pressed), same convention `sinclair-keyboard`/`c64::keyboard`
//! already use. Only the unshifted meaning of dual-label keys (e.g. the
//! real key printed "6" below "&") is named here -- the shifted symbol
//! comes from the real firmware/Shift combination, same "map the base
//! key, let Shift do the rest" approach this project's other machines
//! already use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpcKey {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    Space,
    /// The CPC's small Enter key (line 0, bit 6).
    Enter,
    /// The main typing Return key (line 2, bit 2).
    Return,
    Shift,
    Control,
    CapsLock,
    Tab,
    Escape,
    Delete,
    Copy,
    Clr,
    CursorUp,
    CursorDown,
    CursorLeft,
    CursorRight,
    Comma,
    Period,
    Slash,
    /// Real CPC joystick 1, read through keyboard matrix line 9 alongside
    /// `Delete` -- genuinely a separate physical device from the cursor
    /// keys above (unlike ZX Spectrum's Sinclair/Cursor joysticks, which
    /// are literally wired onto keyboard-matrix keys on real hardware),
    /// but multiplexed onto the same 10x8 matrix scan mechanism. Bit
    /// order per CPCTech's own keyboard table (see module doc comment):
    /// DEL(7)/Fire3(6)/Fire2(5)/Fire1(4)/Right(3)/Left(2)/Down(1)/Up(0).
    Joy1Up,
    Joy1Down,
    Joy1Left,
    Joy1Right,
    Joy1Fire1,
    Joy1Fire2,
    Joy1Fire3,
    /// Real CPC joystick 2 (the second stick, daisy-chained through the
    /// first's socket), wired onto keyboard matrix line 6 -- the same
    /// physical lines as the `6 5 R T G F B` keys, so software reading
    /// either sees the other. Bit order: Up(0)/Down(1)/Left(2)/Right(3)/
    /// Fire1(4)/Fire2(5)/Fire3(6). Tracked separately from the typed keys
    /// (`KeyMatrix::joystick2`) so releasing one source never clears a line
    /// the other still holds.
    Joy2Up,
    Joy2Down,
    Joy2Left,
    Joy2Right,
    Joy2Fire1,
    Joy2Fire2,
    Joy2Fire3,
}

impl CpcKey {
    /// Whether this key is one of joystick 2's wires (tracked apart from the
    /// typed keys that share its matrix line).
    const fn is_joystick2(self) -> bool {
        matches!(
            self,
            Self::Joy2Up
                | Self::Joy2Down
                | Self::Joy2Left
                | Self::Joy2Right
                | Self::Joy2Fire1
                | Self::Joy2Fire2
                | Self::Joy2Fire3
        )
    }

    fn position(self) -> (u8, u8) {
        use CpcKey::*;
        match self {
            CursorUp => (0, 0),
            CursorRight => (0, 1),
            CursorDown => (0, 2),
            Enter => (0, 6),

            CursorLeft => (1, 0),
            Copy => (1, 1),

            Clr => (2, 0),
            Return => (2, 2),
            Shift => (2, 5),
            Control => (2, 7),

            P => (3, 3),
            Slash => (3, 6),
            Comma => (3, 7),

            Digit0 => (4, 0),
            Digit9 => (4, 1),
            O => (4, 2),
            I => (4, 3),
            L => (4, 4),
            K => (4, 5),
            M => (4, 6),
            Period => (4, 7),

            Digit8 => (5, 0),
            Digit7 => (5, 1),
            U => (5, 2),
            Y => (5, 3),
            H => (5, 4),
            J => (5, 5),
            N => (5, 6),
            Space => (5, 7),

            Digit6 => (6, 0),
            Digit5 => (6, 1),
            R => (6, 2),
            T => (6, 3),
            G => (6, 4),
            F => (6, 5),
            B => (6, 6),
            V => (6, 7),

            Digit4 => (7, 0),
            Digit3 => (7, 1),
            E => (7, 2),
            W => (7, 3),
            S => (7, 4),
            D => (7, 5),
            C => (7, 6),
            X => (7, 7),

            Digit1 => (8, 0),
            Digit2 => (8, 1),
            Escape => (8, 2),
            Q => (8, 3),
            Tab => (8, 4),
            A => (8, 5),
            CapsLock => (8, 6),
            Z => (8, 7),

            Delete => (9, 7),
            Joy1Fire3 => (9, 6),
            Joy1Fire2 => (9, 5),
            Joy1Fire1 => (9, 4),
            Joy1Right => (9, 3),
            Joy1Left => (9, 2),
            Joy1Down => (9, 1),
            Joy1Up => (9, 0),

            Joy2Up => (6, 0),
            Joy2Down => (6, 1),
            Joy2Left => (6, 2),
            Joy2Right => (6, 3),
            Joy2Fire1 => (6, 4),
            Joy2Fire2 => (6, 5),
            Joy2Fire3 => (6, 6),
        }
    }
}

const LINE_COUNT: usize = 10;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct KeyMatrix {
    /// One bit per column, indexed by matrix line; `1` = pressed (stored
    /// active-high internally, same convention `c64::keyboard::KeyMatrix`
    /// uses, inverted to active-low only in `read`'s own return value to
    /// match what real hardware electrically presents).
    lines: [u8; LINE_COUNT],
    /// Joystick 2's wires, one bit per column of matrix line 6 (see
    /// `CpcKey::Joy2Up`): OR-ed with `lines[6]` when read, so a typed `6` and
    /// a pushed stick are independent sources of the same wired-AND line.
    joystick2: u8,
}

impl KeyMatrix {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn press(&mut self, key: CpcKey) {
        let (line, bit) = key.position();
        if key.is_joystick2() {
            self.joystick2 |= 1 << bit;
        } else {
            self.lines[line as usize] |= 1 << bit;
        }
    }

    pub fn release(&mut self, key: CpcKey) {
        let (line, bit) = key.position();
        if key.is_joystick2() {
            self.joystick2 &= !(1 << bit);
        } else {
            self.lines[line as usize] &= !(1 << bit);
        }
    }

    /// `line`: which of the 10 real matrix lines is currently selected
    /// (already masked to `0..10` by the caller). Returns the real
    /// active-low 8-bit key state for that line -- a `0` bit means that
    /// column's key is pressed. An out-of-range line (10-15, the decoder
    /// only drives 10 of its 16 possible outputs) reads all-high
    /// ("nothing pressed"), matching no real matrix line being selected.
    pub fn read(&self, line: u8) -> u8 {
        match self.lines.get(line as usize) {
            Some(&pressed) => {
                let stick = if line == 6 { self.joystick2 } else { 0 };
                !(pressed | stick)
            }
            None => 0xFF,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressing_a_key_pulls_its_bit_low_only_on_its_own_line() {
        let mut matrix = KeyMatrix::new();
        matrix.press(CpcKey::A); // line 8, bit 5

        assert_eq!(
            matrix.read(8) & (1 << 5),
            0,
            "A's bit should read low on line 8"
        );
        assert_eq!(
            matrix.read(7),
            0xFF,
            "A shouldn't show up on a different line"
        );
    }

    #[test]
    fn release_clears_the_key() {
        let mut matrix = KeyMatrix::new();
        matrix.press(CpcKey::Space);
        matrix.release(CpcKey::Space);
        let (line, _) = CpcKey::Space.position();
        assert_eq!(matrix.read(line), 0xFF);
    }

    #[test]
    fn enter_and_return_are_distinct_matrix_keys() {
        let mut matrix = KeyMatrix::new();
        matrix.press(CpcKey::Enter);

        assert_eq!(
            matrix.read(0) & (1 << 6),
            0,
            "Enter should pull line 0 bit 6 low"
        );
        assert_ne!(
            matrix.read(2) & (1 << 2),
            0,
            "Enter must not pull Return's bit"
        );

        matrix.release(CpcKey::Enter);
        matrix.press(CpcKey::Return);

        assert_eq!(
            matrix.read(2) & (1 << 2),
            0,
            "Return should pull line 2 bit 2 low"
        );
        assert_ne!(
            matrix.read(0) & (1 << 6),
            0,
            "Return must not pull Enter's bit"
        );
    }

    #[test]
    fn joystick_2_shares_line_6_with_the_typed_keys_without_fighting_over_bits() {
        let mut matrix = KeyMatrix::new();
        matrix.press(CpcKey::Joy2Up); // line 6, bit 0 -- the same wire as `6`
        assert_eq!(matrix.read(6) & 0x01, 0, "stick up pulls the '6' wire low");
        assert_eq!(matrix.read(6) | 0x01, 0xFF, "and nothing else");

        // A typed '6' pressed and released while the stick is held must not
        // clear the stick's line...
        matrix.press(CpcKey::Digit6);
        matrix.release(CpcKey::Digit6);
        assert_eq!(matrix.read(6) & 0x01, 0, "stick still holds the line");
        // ... and releasing the stick must not clear a held typed key.
        matrix.press(CpcKey::Digit6);
        matrix.release(CpcKey::Joy2Up);
        assert_eq!(matrix.read(6) & 0x01, 0, "typed key still holds the line");
        matrix.release(CpcKey::Digit6);
        assert_eq!(matrix.read(6), 0xFF);
    }

    #[test]
    fn joystick_2_bits_follow_up_down_left_right_fire1_fire2_fire3() {
        for (key, bit) in [
            (CpcKey::Joy2Up, 0),
            (CpcKey::Joy2Down, 1),
            (CpcKey::Joy2Left, 2),
            (CpcKey::Joy2Right, 3),
            (CpcKey::Joy2Fire1, 4),
            (CpcKey::Joy2Fire2, 5),
            (CpcKey::Joy2Fire3, 6),
        ] {
            let mut matrix = KeyMatrix::new();
            matrix.press(key);
            assert_eq!(matrix.read(6), !(1u8 << bit), "{key:?}");
            for other in (0..10).filter(|&line| line != 6) {
                assert_eq!(matrix.read(other), 0xFF, "{key:?} leaked to line {other}");
            }
        }
    }

    #[test]
    fn out_of_range_line_reads_nothing_pressed() {
        let matrix = KeyMatrix::new();
        assert_eq!(matrix.read(15), 0xFF);
    }
}
