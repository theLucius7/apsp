#![forbid(unsafe_code)]
//! Panel platform: shared technical services and the HTTP shell (ADR 0128). It names no
//! domain and owns no domain table: every domain lives in its own crate, the `apsp-panel`
//! application assembles their routes and connects the typed interfaces of [`ports`].

pub mod config;
pub mod device_work;
pub mod digest;
pub mod error;
pub mod evidence;
pub mod frontend;
pub mod maintenance;
pub mod net;
pub mod notifications;
pub mod ports;
pub mod schema;
pub mod settings;
mod shell;
pub mod text;
pub mod tokens;
pub mod x509;

mod state;

pub use shell::shell;
pub use state::{AppState, DomainStates};
