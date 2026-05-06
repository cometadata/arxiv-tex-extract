use std::time::Instant;

/// Collects wall-clock timing for each pipeline stage.
#[derive(Debug, Clone, Default)]
pub struct StageTimings {
    entries: Vec<(&'static str, u64)>,
}

impl StageTimings {
    pub fn new() -> Self {
        Self {
            entries: Vec::with_capacity(12),
        }
    }

    /// Time a stage, recording its name and duration in microseconds.
    pub fn time<F, R>(&mut self, name: &'static str, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        let start = Instant::now();
        let result = f();
        self.entries.push((name, start.elapsed().as_micros() as u64));
        result
    }

    pub fn total_us(&self) -> u64 {
        self.entries.iter().map(|(_, us)| us).sum()
    }

    /// Serialize as JSON object: `{"stage": microseconds, ...}`
    pub fn to_json(&self) -> String {
        let pairs: Vec<String> = self
            .entries
            .iter()
            .map(|(name, us)| format!("\"{}\":{}", name, us))
            .collect();
        format!("{{{}}}", pairs.join(","))
    }

    pub fn entries(&self) -> &[(&'static str, u64)] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_time_returns_value() {
        let mut t = StageTimings::new();
        let val = t.time("add", || 2 + 2);
        assert_eq!(val, 4);
        assert_eq!(t.entries().len(), 1);
        assert_eq!(t.entries()[0].0, "add");
    }

    #[test]
    fn test_total_us() {
        let mut t = StageTimings::new();
        t.time("a", || std::thread::sleep(std::time::Duration::from_millis(2)));
        t.time("b", || std::thread::sleep(std::time::Duration::from_millis(2)));
        assert!(t.total_us() >= 4000, "total should be >= 4ms: {}", t.total_us());
    }

    #[test]
    fn test_to_json() {
        let mut t = StageTimings::new();
        t.time("x", || {});
        t.time("y", || {});
        let json = t.to_json();
        assert!(json.starts_with('{'));
        assert!(json.ends_with('}'));
        assert!(json.contains("\"x\":"));
        assert!(json.contains("\"y\":"));
    }

    #[test]
    fn test_empty() {
        let t = StageTimings::new();
        assert_eq!(t.total_us(), 0);
        assert_eq!(t.to_json(), "{}");
    }
}
