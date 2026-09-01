use std::time::SystemTime;

pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> SystemTime;
}

#[derive(Default, Clone)]
pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> SystemTime { SystemTime::now() }
}

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, UNIX_EPOCH};

    #[derive(Clone)]
    pub struct MockClock { inner: Arc<Mutex<SystemTime>> }

    impl MockClock {
        pub fn new(start_epoch_secs: u64) -> Self {
            Self { inner: Arc::new(Mutex::new(UNIX_EPOCH + Duration::from_secs(start_epoch_secs))) }
        }
        pub fn advance(&self, d: Duration) {
            let mut g = self.inner.lock().unwrap();
            *g += d;
        }
    }

    impl Clock for MockClock {
        fn now(&self) -> SystemTime { *self.inner.lock().unwrap() }
    }
}
