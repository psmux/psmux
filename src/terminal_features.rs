/// Client terminal capabilities. Bit assignments are part of the attach protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalFeatures(u64);

impl TerminalFeatures {
    pub const COLOR_256: Self = Self(1 << 0);
    pub const RGB: Self = Self(1 << 1);
    pub const HYPERLINKS: Self = Self(1 << 2);
    pub const TITLE: Self = Self(1 << 3);

    pub const fn from_bits(bits: u64) -> Self { Self(bits) }
    pub const fn bits(self) -> u64 { self.0 }
    pub const fn contains(self, feature: Self) -> bool { self.0 & feature.0 == feature.0 }

    pub fn from_hex(report: &str) -> Option<Self> {
        let digits = report.strip_prefix("0x")?;
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u64::from_str_radix(digits, 16).ok().map(Self::from_bits)
    }
}

impl std::fmt::Display for TerminalFeatures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut separator = "";
        for (feature, name) in [
            (Self::COLOR_256, "256"), (Self::RGB, "RGB"),
            (Self::HYPERLINKS, "hyperlinks"), (Self::TITLE, "title"),
        ] {
            if self.contains(feature) {
                write!(f, "{separator}{name}")?;
                separator = ",";
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests_issue763_mask {
    use super::*;

    #[test]
    fn hex_reports_retain_all_bits_and_round_trip() {
        for (report, bits) in [
            ("0x0", 0), ("0xf", 15), ("0x000000000000000f", 15),
            ("0x000000000000000000000000f", 15), ("0xAB", 171),
            ("0x8000000000000000", 1u64 << 63), ("0xffffffffffffffff", u64::MAX),
        ] {
            let features = TerminalFeatures::from_hex(report).unwrap();
            assert_eq!(features.bits(), bits, "{report}");
            assert_eq!(TerminalFeatures::from_hex(&format!("{:#x}", features.bits())), Some(features));
        }
    }

    #[test]
    fn malformed_reports_are_not_feature_masks() {
        for report in ["", "0x", "f", "15", "0x+f", "0x-f", "0x1_0", "0xg", "0x f", "0xf ", "0xé"] {
            assert_eq!(TerminalFeatures::from_hex(report), None, "{report:?}");
        }
    }

    #[test]
    fn one_past_u64_is_rejected_instead_of_truncated() {
        assert_eq!(TerminalFeatures::from_hex("0xffffffffffffffff").unwrap().bits(), u64::MAX);
        let overflow = u64::MAX as u128 + 1;
        assert!(overflow > u64::MAX as u128);
        assert_eq!(TerminalFeatures::from_hex(&format!("{overflow:#x}")), None);
    }

    #[test]
    fn known_bit_assignments_and_names_are_stable() {
        let all = TerminalFeatures::from_bits(15);
        for (bits, feature, name) in [
            (1, TerminalFeatures::COLOR_256, "256"), (2, TerminalFeatures::RGB, "RGB"),
            (4, TerminalFeatures::HYPERLINKS, "hyperlinks"), (8, TerminalFeatures::TITLE, "title"),
        ] {
            assert_eq!(feature.bits(), bits);
            assert!(all.contains(feature));
            assert_eq!(TerminalFeatures::from_bits(bits).to_string(), name);
        }
        assert_eq!(all.to_string(), "256,RGB,hyperlinks,title");
    }

    #[test]
    fn unknown_bits_are_retained_but_not_named_or_confused_with_known_flags() {
        let features = TerminalFeatures::from_bits((1u64 << 63) | TerminalFeatures::RGB.bits());
        assert!(features.contains(TerminalFeatures::RGB));
        assert!(!features.contains(TerminalFeatures::HYPERLINKS));
        assert_eq!(features.bits(), 0x8000000000000002);
        assert_eq!(features.to_string(), "RGB");
        assert_eq!(TerminalFeatures::from_bits(1u64 << 63).to_string(), "");
        assert_eq!(TerminalFeatures::from_bits(0).to_string(), "");
    }
}
