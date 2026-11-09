//! Cloud work: bandwidth changes (`bandwidth`), power (`power`), address rotation
//! (`eip_rotation`) and the background worker that refreshes accounts and runs queued work
//! (`worker`). Operations answers the resources' question about work in progress
//! (`apsp_resources::cloud::work`) from here.

pub mod bandwidth;
pub mod eip_rotation;
pub mod power;
#[cfg(test)]
mod tests;
pub mod worker;
