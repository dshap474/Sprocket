pub trait Clock {
    fn now_unix(&self) -> i64;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix(&self) -> i64 {
        if let Some(now) = std::env::var("SPROCKET_TEST_NOW")
            .ok()
            .and_then(|raw| raw.parse::<i64>().ok())
        {
            return now;
        }

        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }
}
