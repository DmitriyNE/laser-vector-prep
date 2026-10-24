//! Length units. Everything internal is millimetres; units only exist at import/export.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Unit {
    Millimetre,
    Micrometre,
    Centimetre,
    Metre,
    Inch,
    /// Thousandth of an inch.
    Mil,
    /// PostScript/PDF point, 1/72 in.
    Point,
}

impl Unit {
    /// Millimetres per one unit.
    pub const fn mm_per_unit(self) -> f64 {
        match self {
            Unit::Millimetre => 1.0,
            Unit::Micrometre => 0.001,
            Unit::Centimetre => 10.0,
            Unit::Metre => 1000.0,
            Unit::Inch => 25.4,
            Unit::Mil => 0.0254,
            Unit::Point => 25.4 / 72.0,
        }
    }

    pub fn to_mm(self, value: f64) -> f64 {
        value * self.mm_per_unit()
    }

    pub fn from_mm(self, mm: f64) -> f64 {
        mm / self.mm_per_unit()
    }

    /// DXF `$INSUNITS` code for this unit.
    pub const fn dxf_insunits(self) -> u16 {
        match self {
            Unit::Inch => 1,
            Unit::Millimetre => 4,
            Unit::Centimetre => 5,
            Unit::Metre => 6,
            Unit::Mil => 9,
            Unit::Micrometre => 13,
            // DXF has no point unit; callers convert to millimetres first.
            Unit::Point => 0,
        }
    }

    /// Unit for a DXF `$INSUNITS` code. `Ok(None)` is "unitless" (code 0); `Err` carries an
    /// unsupported code (feet, miles, yards, parsecs...).
    pub fn from_dxf_insunits(code: i64) -> Result<Option<Unit>, i64> {
        Ok(Some(match code {
            0 => return Ok(None),
            1 => Unit::Inch,
            4 => Unit::Millimetre,
            5 => Unit::Centimetre,
            6 => Unit::Metre,
            9 => Unit::Mil,
            13 => Unit::Micrometre,
            other => return Err(other),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const ALL: [Unit; 6] = [
        Unit::Millimetre,
        Unit::Micrometre,
        Unit::Centimetre,
        Unit::Metre,
        Unit::Inch,
        Unit::Mil,
    ];

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
