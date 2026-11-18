//! `SystemClock`: the real wall clock. Platform-independent.

use zwisper_core::ports::Clock;

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advances() {
        let c = SystemClock;
        let a = c.now();
        assert!(c.now() >= a);
    }
}
