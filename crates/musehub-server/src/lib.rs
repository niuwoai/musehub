//! MuseHub server library (HTTP router for tests and binary).

mod routes;

pub use routes::{app, build_product_state, AppState, GIT_NOT_CONFIGURED, PROCESS_OK};
