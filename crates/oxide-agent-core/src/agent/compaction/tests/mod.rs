// Allow clone_on_ref_ptr in compaction tests due to trait object coercion requirements
// when using Arc<dyn Trait> with concrete Arc<ConcreteType>
#![allow(clippy::clone_on_ref_ptr)]

mod budget_boundaries;
mod cleanup_stages;
mod fixtures;
mod rebuild_archive;
mod recent_window;
mod summary_paths;
