// Added for Bosphorus (2026-10-25): wall-clock timeline of proving stages.

//! With `R0_TL=1`, every [`span`] prints `[tl <start> <length>] <what>` to stderr when it ends,
//! times in seconds since the first span. Without it, a span costs one atomic load.

use std::{sync::OnceLock, time::Instant};

fn origin() -> Option<Instant> {
    static ORIGIN: OnceLock<Option<Instant>> = OnceLock::new();
    *ORIGIN.get_or_init(|| std::env::var("R0_TL").is_ok_and(|v| v == "1").then(Instant::now))
}

/// Whether the timeline is on.
pub fn enabled() -> bool {
    origin().is_some()
}

/// Ends (and prints) when dropped.
pub struct Span(Option<(Instant, String)>);

/// Starts a span; `what` is only evaluated when the timeline is on.
pub fn span(what: impl FnOnce() -> String) -> Span {
    Span(origin().map(|_| (Instant::now(), what())))
}

impl Drop for Span {
    fn drop(&mut self) {
        if let (Some(origin), Some((start, what))) = (origin(), self.0.take()) {
            let at = start.duration_since(origin).as_secs_f64();
            eprintln!("[tl {at:8.3} {:7.3}] {what}", start.elapsed().as_secs_f64());
        }
    }
}
