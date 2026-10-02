//! Python-compatible `repr(float)` formatting.
//!
//! The MoeFlow backend writes label coordinates with Python's `str(float)`, which is
//! `repr()`. Rust's `{}` for `f64` is also shortest-round-trip, but the two disagree on
//! *notation*: Python switches to exponential when the decimal exponent is `<= -4` or
//! `> 16`, while Rust never does. A local export that formats `1e-05` as `0.00001`
//! would not be byte-identical to the server's output.
//!
//! This implements CPython's rule as found in `Python/pystrtod.c` (`format_float_short`,
//! `case 'r'`).

/// Format an `f64` exactly like CPython's `repr()`.
pub fn py_float_str(v: f64) -> String {
    if v.is_nan() {
        return "nan".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 { "inf" } else { "-inf" }.to_string();
    }

    let negative = v.is_sign_negative();
    let magnitude = v.abs();

    // Shortest round-trip digits + decimal exponent, via Rust's LowerExp which uses the
    // same shortest-representation algorithm. Rendered as e.g. "1.2345e3", "5e-324".
    let sci = format!("{:e}", magnitude);
    let (mantissa, exp) = sci
        .split_once('e')
        .expect("LowerExp always emits an exponent");
    let exp: i32 = exp.parse().expect("exponent is an integer");

    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    // value == 0.digits * 10^decpt
    let decpt = exp + 1;

    // CPython: use exponential notation if decpt <= -4 || decpt > 16.
    let body = if decpt <= -4 || decpt > 16 {
        format_exp(&digits, decpt)
    } else {
        format_fixed(&digits, decpt)
    };

    if negative {
        format!("-{}", body)
    } else {
        body
    }
}

/// `d[.ddd]e±XX` — exponent is `decpt - 1`, always signed, zero-padded to 2 digits.
fn format_exp(digits: &str, decpt: i32) -> String {
    let exp = decpt - 1;
    let mantissa = if digits.len() == 1 {
        digits.to_string()
    } else {
        format!("{}.{}", &digits[..1], &digits[1..])
    };
    let sign = if exp < 0 { '-' } else { '+' };
    format!("{}e{}{:02}", mantissa, sign, exp.abs())
}

/// Plain decimal notation.
fn format_fixed(digits: &str, decpt: i32) -> String {
    if decpt <= 0 {
        // 0.00ddd
        let zeros = "0".repeat((-decpt) as usize);
        format!("0.{}{}", zeros, digits)
    } else if (decpt as usize) >= digits.len() {
        // ddd000.0
        let zeros = "0".repeat(decpt as usize - digits.len());
        format!("{}{}.0", digits, zeros)
    } else {
        let (int_part, frac_part) = digits.split_at(decpt as usize);
        format!("{}.{}", int_part, frac_part)
    }
}

#[cfg(test)]
mod tests {
    use super::py_float_str;

    /// Ground truth generated with CPython 3.11 `repr()`.
    const CASES: &[(f64, &str)] = &[
        (0.0, "0.0"),
        (0.5, "0.5"),
        (0.1, "0.1"),
        (0.25, "0.25"),
        (0.3333333333333333, "0.3333333333333333"),
        (1.0, "1.0"),
        (0.19999999999999998, "0.19999999999999998"),
        (1e-05, "1e-05"),
        (0.0001, "0.0001"),
        (0.0001234, "0.0001234"),
        (1.5e-07, "1.5e-07"),
        (1234567890123456.0, "1234567890123456.0"),
        (1e16, "1e+16"),
        (1e17, "1e+17"),
        (0.9999999999999999, "0.9999999999999999"),
        (2.220446049250313e-16, "2.220446049250313e-16"),
        (5e-324, "5e-324"),
        (1.7976931348623157e308, "1.7976931348623157e+308"),
        (100.0, "100.0"),
        (0.05, "0.05"),
        (1234.5678, "1234.5678"),
    ];

    #[test]
    fn matches_cpython_repr() {
        for (input, expected) in CASES {
            assert_eq!(
                py_float_str(*input),
                *expected,
                "mismatch for input {:?}",
                input
            );
        }
    }

    #[test]
    fn negative_zero_keeps_sign() {
        assert_eq!(py_float_str(-0.0), "-0.0");
    }

    #[test]
    fn non_finite_values() {
        assert_eq!(py_float_str(f64::NAN), "nan");
        assert_eq!(py_float_str(f64::INFINITY), "inf");
        assert_eq!(py_float_str(f64::NEG_INFINITY), "-inf");
    }

    /// The `0.3 - 0.1` case: shortest repr must survive the round trip.
    #[test]
    fn round_trips_through_parse() {
        for (input, _) in CASES {
            let text = py_float_str(*input);
            let parsed: f64 = text.parse().expect("output must re-parse");
            assert_eq!(parsed.to_bits(), input.to_bits(), "round trip failed: {}", text);
        }
    }
}
