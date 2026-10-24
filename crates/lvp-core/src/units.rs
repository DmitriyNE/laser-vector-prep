//! Length units. Everything internal is millimetres; units only exist at import/export.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Unit {
    Millimetre,
    Micrometre,
    Inch,
    /// Thousandth of an inch.
    Mil,
}

impl Unit {
    /// Millimetres per one unit.
    pub const fn mm_per_unit(self) -> f64 {
        match self {
            Unit::Millimetre => 1.0,
            Unit::Micrometre => 0.001,
            Unit::Inch => 25.4,
            Unit::Mil => 0.0254,
        }
    }

    pub fn to_mm(self, value: f64) -> f64 {
        value * self.mm_per_unit()
    }

    pub fn from_mm(self, mm: f64) -> f64 {
        mm / self.mm_per_unit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const ALL: [Unit; 4] = [Unit::Millimetre, Unit::Micrometre, Unit::Inch, Unit::Mil];

    #[test]
    fn validated_pdf_stroke_is_one_mil() {
        assert_eq!(Unit::Mil.to_mm(1.0), 0.0254);
        assert_eq!(Unit::Inch.to_mm(1.0), 25.4);
    }

    proptest! {
        #[test]
        fn roundtrip_through_mm(v in -1e6..1e6f64, i in 0usize..ALL.len()) {
            let u = ALL[i];
            let back = u.from_mm(u.to_mm(v));
            prop_assert!((back - v).abs() <= 1e-9 * v.abs().max(1.0));
        }

        #[test]
        fn mil_is_a_thousandth_of_an_inch(v in -1e6..1e6f64) {
            let a = Unit::Mil.to_mm(v);
            let b = Unit::Inch.to_mm(v / 1000.0);
            prop_assert!((a - b).abs() <= 1e-9 * a.abs().max(1.0));
        }
    }
}
