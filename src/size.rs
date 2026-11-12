//! Byte sizes and deltas as text (spec 7.2).

const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];

/// `812 B`, `1.2 GiB`: under 1024 bytes an integer, otherwise one decimal in steps of 1024,
/// rounded half away from zero; a value that rounds to `1024.0` moves up one unit.
pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut unit = 1024u128;
    for (i, name) in UNITS.iter().enumerate() {
        // Tenths of the unit, rounded half up; the value is positive, so that is away from zero.
        let tenths = (u128::from(bytes) * 20 + unit) / (2 * unit);
        if tenths < 10240 || i == UNITS.len() - 1 {
            return format!("{}.{} {name}", tenths / 10, tenths % 10);
        }
        unit *= 1024;
    }
    unreachable!()
}

/// `+41.0 MiB`, `-300 B`, and `0 B` for no change.
pub fn format_delta(delta: i64) -> String {
    match delta.signum() {
        0 => "0 B".to_owned(),
        1 => format!("+{}", format_size(delta.unsigned_abs())),
        _ => format!("-{}", format_size(delta.unsigned_abs())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KIB: u64 = 1024;
    const MIB: u64 = KIB * KIB;
    const GIB: u64 = MIB * KIB;
    const TIB: u64 = GIB * KIB;

    #[test]
    fn sizes() {
        let cases = [
            (0, "0 B"),
            (812, "812 B"),
            (1023, "1023 B"),
            (1024, "1.0 KiB"),
            (1075, "1.0 KiB"), // 1.0498…
            (1076, "1.1 KiB"), // 1.0508…, half-way is 1075.2
            (1536, "1.5 KiB"), // exact half of a unit
            (KIB * 1023, "1023.0 KiB"),
            (MIB - 52, "1023.9 KiB"), // 1023.949…
            (MIB - 51, "1.0 MiB"),    // 1023.950… rounds to 1024.0
            (MIB, "1.0 MiB"),
            (1_288_490_188, "1.2 GiB"),
            (1_331_439_861, "1.2 GiB"),
            (GIB * 1024 - 1, "1.0 TiB"),
            (TIB * 2048, "2048.0 TiB"),
            (u64::MAX, "16777216.0 TiB"),
        ];
        for (bytes, expected) in cases {
            assert_eq!(format_size(bytes), expected, "{bytes}");
        }
    }

    #[test]
    fn half_way_rounds_away_from_zero() {
        // 1.25 MiB lies exactly half-way between two tenths.
        assert_eq!(format_size(MIB + MIB / 4), "1.3 MiB");
        assert_eq!(format_delta(-((MIB + MIB / 4) as i64)), "-1.3 MiB");
    }

    #[test]
    fn deltas() {
        let cases = [
            (0, "0 B"),
            (100, "+100 B"),
            (-300 * KIB as i64, "-300.0 KiB"),
            (3_250_586, "+3.1 MiB"),
            (42_949_673, "+41.0 MiB"),
            (i64::MIN, "-8388608.0 TiB"),
        ];
        for (delta, expected) in cases {
            assert_eq!(format_delta(delta), expected, "{delta}");
        }
    }
}
