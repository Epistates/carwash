//! Engine behind [carwash](https://github.com/epistates/carwash): finds projects of every
//! major ecosystem under a directory, recognises their build outputs, dependency installs,
//! caches and environments, measures what deleting them would really free, and deletes them
//! safely.
//!
//! The pipeline, run by [`Engine::scan`]:
//!
//! 1. [`discover`]: a parallel walk that classifies directories by entry names only and
//!    never descends into artifacts.
//! 2. [`git`] and [`measure`] in parallel: tracked/ignored status per artifact, and
//!    allocated, hard-link-aware sizes.
//!
//! Ecosystems are data ([`ecosystem::builtin_specs`]); users extend or override them with a
//! TOML file in the same format.

pub mod cache;
pub mod clean;
pub mod deps;
pub mod discover;
pub mod ecosystem;
pub mod fmt;
pub mod git;
pub mod history;
pub mod measure;
pub mod model;
pub mod paths;
pub mod safety;
pub mod scan;
pub mod select;
pub mod tasks;

pub use discover::{Counters, DiscoverOptions};
pub use ecosystem::{EcoId, Registry, RuleId};
pub use model::*;
pub use scan::{Engine, ScanEvent, ScanOptions, Snapshot, Target};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Cooperative cancellation shared between a frontend and engine work.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}
