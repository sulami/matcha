/// Returns if the given string is safe to use in a file system path.
pub fn is_file_system_safe(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}
