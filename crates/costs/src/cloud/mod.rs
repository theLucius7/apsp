//! Cloud costs: what costs keeps about each cloud account and resource (`accounts`), CDT
//! billed traffic and monthly bills (`billing`), account balances and instance bills
//! (`cost_cache`), the hourly CDT traffic history (`traffic_history`), and each resource's
//! budget and, without a server, its purchase (`resource_costs`). They are stored in costs'
//! own tables beside resources' account and resource rows (ADR 0137 steps 6 and 2).

pub mod accounts;
pub mod billing;
pub mod cost_cache;
pub mod resource_costs;
pub mod traffic_history;
