//! Cloud costs: what costs keeps about each cloud account and resource (`accounts`), CDT
//! billed traffic and monthly bills (`billing`), account balances and instance bills
//! (`cost_cache`), and the hourly CDT traffic history (`traffic_history`). They are stored in
//! costs' own tables beside resources' account and resource rows (ADR 0137 step 6).

pub mod accounts;
pub mod billing;
pub mod cost_cache;
pub mod traffic_history;
