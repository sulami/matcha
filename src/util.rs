/// Returns if the given string is safe to use in a file system path.
pub fn is_file_system_safe(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_file_system_safe() {
        assert!(is_file_system_safe("foo"));
        assert!(is_file_system_safe("foo-bar"));
        assert!(is_file_system_safe("foo_bar"));
        assert!(is_file_system_safe("foo.bar"));

        assert!(!is_file_system_safe("foo/bar"));
        assert!(!is_file_system_safe("foo bar"));
        assert!(!is_file_system_safe(r"foo\bar"));
    }
}
