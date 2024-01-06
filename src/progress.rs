use indicatif::{style::ProgressStyle, ProgressBar};

static PROGRESS_BAR_STYLE: &str = "{msg} {bar:40.cyan/blue} {pos}/{len} | {eta} remaining";

/// Sets up the default progress bar style.
pub fn create_progress_bar(msg: &str, len: u64) -> ProgressBar {
    let pb = ProgressBar::new(len)
        .with_message(msg.to_string())
        .with_style(
            ProgressStyle::default_bar()
                .template(PROGRESS_BAR_STYLE)
                .expect("failed to parse progress bar style"),
        );
    pb.tick();
    pb
}
