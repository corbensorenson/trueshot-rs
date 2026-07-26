use human_panic::setup_panic;
use sentry::ClientInitGuard;

#[allow(deprecated)]
pub fn init_crash_handler(dsn: Option<String>) -> Option<ClientInitGuard> {
    // 1. Human readable panic messages for CLI users
    setup_panic!();

    // 2. Sentry reporting for production monitoring
    if let Some(dsn_url) = dsn {
        let guard = sentry::init((dsn_url, sentry::ClientOptions {
            release: sentry::release_name!(),
            ..Default::default()
        }));
        Some(guard)
    } else {
        None
    }
}
