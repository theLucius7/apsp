//! Typed interfaces that the application connects at startup (ADR 0128).
//!
//! An interface is declared by the owner that calls it and implemented by the owner
//! that answers; the application installs each implementation once. This module
//! holds only that mechanism and names no domain.

use std::{future::Future, pin::Pin, sync::OnceLock};

/// Future returned by an interface method.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The single implementation of one interface, installed by the application.
pub struct Port<T: ?Sized + 'static>(OnceLock<&'static T>);

impl<T: ?Sized + Sync + 'static> Port<T> {
    pub const fn new() -> Self {
        Self(OnceLock::new())
    }

    /// Installs the implementation; later calls keep the first one.
    pub fn install(&self, implementation: &'static T) {
        let _ = self.0.set(implementation);
    }

    pub fn get(&self) -> Option<&'static T> {
        self.0.get().copied()
    }
}

impl<T: ?Sized + Sync + 'static> Default for Port<T> {
    fn default() -> Self {
        Self::new()
    }
}
