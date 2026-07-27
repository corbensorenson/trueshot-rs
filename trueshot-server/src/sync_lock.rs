use std::sync::{Mutex, MutexGuard};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockUnavailable;

pub fn lock<'a, T>(
    mutex: &'a Mutex<T>,
    state: &'static str,
) -> Result<MutexGuard<'a, T>, LockUnavailable> {
    mutex.lock().map_err(|_| {
        tracing::error!(state, "shared state lock is poisoned");
        LockUnavailable
    })
}

pub fn replace<T>(mutex: &Mutex<T>, state: &'static str, value: T) -> Result<(), LockUnavailable> {
    *lock(mutex, state)? = value;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn poisoned_lock_fails_without_panicking_the_caller() {
        let mutex = Arc::new(Mutex::new(7_u8));
        let poisoned = mutex.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoned.lock().expect("initial lock");
            panic!("poison lock");
        })
        .join();

        lock(&mutex, "test.authority").expect_err("poison must fail closed");
    }
}
