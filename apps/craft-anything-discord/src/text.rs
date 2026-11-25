//! Text cut to Discord's limits. Pure: no Discord types.

/// Cuts `text` to `max` characters, ending with `…` when it was longer.
pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipped_text_fits_the_limit() {
        let long = "é".repeat(2_000);
        let cut = clip(&long, 1_024);
        assert!(cut.chars().count() <= 1_024 && cut.ends_with('…'));
        assert_eq!(clip("short", 1_024), "short");
    }

    #[test]
    fn a_limit_of_zero_still_yields_no_more_than_the_ellipsis() {
        assert_eq!(clip("abc", 0), "…");
    }
}
