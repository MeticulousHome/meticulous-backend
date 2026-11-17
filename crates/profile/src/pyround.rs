//! CPython's `round(x, n)` and `format(x, ".nf")` for floats.

/// `round(x, ndigits)` for a float and `ndigits >= 0`: the decimal rounding
/// of the exact binary value, ties to even, read back as the nearest float.
/// CPython gets there through `_Py_dg_dtoa` mode 3 and `_Py_dg_strtod`;
/// Rust's fixed-precision formatting is the same exact, ties-to-even
/// rounding, and its parser is correctly rounded.
pub fn py_round(x: f64, ndigits: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.ndigits$}").parse::<f64>().unwrap_or(x)
}

/// `f"{x:.{ndigits}f}"`.
pub fn py_format_fixed(x: f64, ndigits: usize) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf" } else { "-inf" }.into();
    }
    format!("{x:.ndigits$}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ties_go_to_even_on_the_exact_value() {
        assert_eq!(py_round(0.125, 2), 0.12);
        assert_eq!(py_round(0.375, 2), 0.38);
        // 2.675 is stored as 2.67499999...
        assert_eq!(py_round(2.675, 2), 2.67);
        assert_eq!(py_round(8.456, 2), 8.46);
        assert_eq!(py_round(36.24, 1), 36.2);
        assert!(py_round(-0.001, 2).is_sign_negative());
        assert_eq!(py_format_fixed(2.5, 0), "2");
        assert_eq!(py_format_fixed(3.5, 0), "4");
        assert_eq!(py_format_fixed(0.05, 1), "0.1");
    }
}
