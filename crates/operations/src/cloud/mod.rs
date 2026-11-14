//! Cloud work: bandwidth changes (`bandwidth`), power (`power`), address rotation
//! (`eip_rotation`) and the background worker that refreshes accounts and runs queued work
//! (`worker`), with what operations keeps about each cloud account and resource in its own
//! tables (`automation`, ADR 0137 step 7). Operations answers the resources' questions
//! about work in progress (`apsp_resources::cloud::work`) and automation
//! (`apsp_resources::cloud::automation`) from here.

pub mod automation;
pub mod bandwidth;
pub mod eip_rotation;
pub mod power;
#[cfg(test)]
mod tests;
pub mod worker;
