//! In-memory cache for loaded skills.

use crate::agent::skills::types::Skill;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// Cache for loaded skill contents.
#[derive(Debug)]
pub struct SkillCache {
    loaded: HashMap<String, Arc<Skill>>,
    order: VecDeque<String>,
    max_loaded: usize,
}

impl SkillCache {
    /// Create a new cache with a maximum number of loaded skills.
    #[must_use]
    pub fn new(max_loaded: usize) -> Self {
        Self {
            loaded: HashMap::new(),
            order: VecDeque::new(),
            max_loaded,
        }
    }

    /// Fetch a cached skill by name, if present.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<Skill>> {
        self.loaded.get(name).cloned()
    }

    /// Insert a new skill into the cache, evicting old entries if needed.
    pub fn insert(&mut self, skill: Skill) -> Arc<Skill> {
        let name = skill.metadata.name.clone();

        if let Some(existing) = self.loaded.get(&name) {
            return Arc::clone(existing);
        }

        let arc = Arc::new(skill);
        self.loaded.insert(name.clone(), Arc::clone(&arc));
        self.order.push_back(name.clone());

        while self.loaded.len() > self.max_loaded {
            if let Some(oldest) = self.order.pop_front() {
                self.loaded.remove(&oldest);
            } else {
                break;
            }
        }

        arc
    }

    /// Clear the cache contents.
    pub fn clear(&mut self) {
        self.loaded.clear();
        self.order.clear();
    }
}
