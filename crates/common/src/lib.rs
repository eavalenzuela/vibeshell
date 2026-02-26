use std::sync::Once;

use tracing_subscriber::{fmt, EnvFilter};

static LOGGING_INIT: Once = Once::new();

pub fn init_logging(component: &str) {
    LOGGING_INIT.call_once(|| {
        let env_filter = EnvFilter::try_from_env("VIBESHELL_LOG")
            .or_else(|_| EnvFilter::try_from_default_env())
            .unwrap_or_else(|_| EnvFilter::new("info"));

        fmt()
            .with_env_filter(env_filter)
            .with_target(true)
            .compact()
            .init();

        tracing::debug!(component, "logging initialized");
    });
}
