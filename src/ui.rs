use indicatif::{style::ProgressStyle, ProgressBar};

static PROGRESS_BAR_STYLE: &str = "{bar:40.cyan/blue} {pos}/{len} {msg}";

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

#[macro_export]
macro_rules! join_set_with_progress_bar {
    ($msg:expr, $iter:expr, $func:expr) => {{
        let pb = create_progress_bar($msg, $iter.len() as u64);
        let mut set = JoinSet::new();

        #[allow(clippy::redundant_closure_call)]
        $func(&mut set);

        let mut results = vec![];
        while let Some(result) = set.join_next().await {
            pb.inc(1);
            results.push(result?);
        }

        pb.finish_and_clear();
        results
    }};
}
