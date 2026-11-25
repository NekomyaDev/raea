//! An error that repeats, said once (#239): a failing audio device calls its stream's error callback without pause.

/// A device's error said once, then only how often it came back (at its 10th, 100th, ... time, and never more than once
/// a second): a failing ALSA device calls a stream's error callback without pause, and every call printed a line: #239's
/// app.log reached 40 GB of one line in two days.
pub fn said_once<E: std::fmt::Display>(what: &'static str) -> impl FnMut(E) + Send + 'static {
    let mut seen = Repeats::default();
    move |e| {
        let m = e.to_string();
        match seen.hit(&m, std::time::Instant::now()) {
            Some(1) => eprintln!("{what}: {m}"),
            Some(n) => eprintln!("{what}: {m} ({n} times now)"),
            None => {}
        }
    }
}

/// Which of an error's repeats get said: `hit` answers with the count to print, or nothing.
#[derive(Default)]
struct Repeats {
    last: String,
    n: u64,
    next: u64,
    at: Option<std::time::Instant>,
}

impl Repeats {
    fn hit(&mut self, m: &str, now: std::time::Instant) -> Option<u64> {
        if m != self.last || self.n == 0 {
            (self.last, self.n, self.next) = (m.to_string(), 0, 1);
        }
        self.n += 1;
        let soon = self.at.is_some_and(|t| now.duration_since(t).as_secs() < 1);
        if self.n < self.next || (soon && self.n > 1) {
            return None;
        }
        self.next = self.n.saturating_mul(10);
        self.at = Some(now);
        Some(self.n)
    }
}

#[cfg(test)]
mod tests {
    use super::Repeats;
    use std::time::{Duration, Instant};

    /// #239: half a billion repeats of one ALSA error were half a billion lines. Two days of them are a handful.
    #[test]
    fn a_repeating_error_is_a_handful_of_lines() {
        let (mut r, t0) = (Repeats::default(), Instant::now());
        // 5,000 a second for a minute
        let said = (0..300_000u64).filter(|i| r.hit("alsa::poll() returned POLLERR", t0 + Duration::from_micros(i * 200)).is_some()).count();
        assert!(said <= 8, "{said} lines for 300,000 repeats");
        assert_eq!(Repeats::default().hit("x", t0), Some(1), "the first is always said");
        // another error is said at once, when a second has gone by
        assert_eq!(r.hit("the device went away", t0 + Duration::from_secs(120)), Some(1));
    }
}
