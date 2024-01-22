use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

/// Creates a default style spinnner, optionally adding it to a multi-progress bar.
pub fn create_spinner(msg: &str, mpb: Option<&MultiProgress>) -> ProgressBar {
    let spinner = if let Some(mpb) = mpb {
        mpb.add(ProgressBar::new_spinner())
    } else {
        ProgressBar::new_spinner()
    };
    spinner.enable_steady_tick(Duration::from_millis(100));
    spinner.set_style(ProgressStyle::with_template("{spinner:.green} {msg}").unwrap());
    spinner.set_message(msg.to_string());
    spinner
}

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
        assert!(!is_file_system_safe(r#"foo"bar"#));
    }
}
