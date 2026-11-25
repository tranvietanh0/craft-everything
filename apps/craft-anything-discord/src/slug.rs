//! App name → slug → channel name. A slug is `[a-z0-9-]` only, so it is safe as a folder name too.

/// Discord's channel-name limit.
pub const MAX_CHANNEL_NAME: usize = 90;
const CHANNEL_PREFIX: &str = "port-";
const FALLBACK_SLUG: &str = "app";

/// Lowercase ASCII letters and digits; every other run of characters becomes one `-`. Leading and
/// trailing `-` are dropped. A name with no ASCII letter or digit gives `app`.
pub fn slugify(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() { FALLBACK_SLUG.to_owned() } else { slug }
}

/// `port-<slug>` that is not in `taken`, at most [`MAX_CHANNEL_NAME`] characters. A clash gets
/// `-2`, `-3`, … and the base is cut so the suffix still fits.
pub fn channel_name(slug: &str, taken: &[String]) -> String {
    let base = format!("{CHANNEL_PREFIX}{slug}");
    let mut n = 1_usize;
    loop {
        let suffix = if n == 1 { String::new() } else { format!("-{n}") };
        let candidate = with_suffix(&base, &suffix);
        if !taken.contains(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

fn with_suffix(base: &str, suffix: &str) -> String {
    let keep = MAX_CHANNEL_NAME.saturating_sub(suffix.chars().count());
    let head: String = base.chars().take(keep).collect();
    format!("{}{suffix}", head.trim_end_matches('-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn punctuation_and_case_collapse_to_single_dashes() {
        assert_eq!(slugify("  Hill Climb: Racing 2!! "), "hill-climb-racing-2");
    }

    #[test]
    fn non_ascii_letters_are_separators_not_transliterated() {
        // "Đ" is not ASCII, so it separates; leading separators are dropped. No transliteration.
        assert_eq!(slugify("Đua Xe"), "ua-xe");
        assert_eq!(slugify("Gà Rán Vui"), "g-r-n-vui");
    }

    #[test]
    fn a_name_without_ascii_letters_or_digits_falls_back() {
        assert_eq!(slugify("🎮 ★★"), "app");
    }

    #[test]
    fn channel_name_is_prefixed() {
        assert_eq!(channel_name("hill-climb", &[]), "port-hill-climb");
    }

    #[test]
    fn a_clash_gets_a_numeric_suffix() {
        let taken = vec!["port-hill-climb".to_owned()];
        assert_eq!(channel_name("hill-climb", &taken), "port-hill-climb-2");
        let taken = vec!["port-hill-climb".to_owned(), "port-hill-climb-2".to_owned()];
        assert_eq!(channel_name("hill-climb", &taken), "port-hill-climb-3");
    }

    #[test]
    fn a_long_name_is_cut_to_the_limit_and_keeps_its_suffix() {
        let slug = "a".repeat(200);
        let first = channel_name(&slug, &[]);
        assert!(first.chars().count() <= MAX_CHANNEL_NAME);
        let taken = vec![first.clone()];
        let second = channel_name(&slug, &taken);
        assert!(second.ends_with("-2"));
        assert!(second.chars().count() <= MAX_CHANNEL_NAME);
        assert_ne!(first, second);
    }
}
