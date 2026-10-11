//! Fitting text into terminal columns.

/// Keeps the start of a label, ending with an ellipsis when it cannot fit.
pub(super) fn fit(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut result: String = value.chars().take(width - 1).collect();
    result.push('…');
    result
}

pub(super) fn width(value: &str) -> u16 {
    u16::try_from(value.chars().count()).unwrap_or(u16::MAX)
}

/// Milliseconds as `m:ss`.
pub fn clock(milliseconds: u32) -> String {
    let seconds = milliseconds / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_by_characters_not_bytes() {
        assert_eq!(fit("Beyoncé", 7), "Beyoncé");
        assert_eq!(fit("Beyoncé Knowles", 8), "Beyoncé…");
        assert_eq!(fit("anything", 0), "");
        assert_eq!(clock(191_368), "3:11");
        assert_eq!(clock(0), "0:00");
    }
}
