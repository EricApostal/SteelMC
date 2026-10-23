//! Vanilla `NearestVisibleLivingEntities` memory value.

use std::sync::Arc;

use crate::entity::SharedEntity;

/// Vanilla `NearestVisibleLivingEntities`: nearby living entities plus the cached
/// entity-targetable test the sensor installed.
#[derive(Clone)]
pub struct NearestVisibleLivingEntities {
    nearby_entities: Vec<SharedEntity>,
    line_of_sight_test: Arc<dyn Fn(&SharedEntity) -> bool + Send + Sync>,
}

impl NearestVisibleLivingEntities {
    #[must_use]
    pub fn new(
        nearby_entities: Vec<SharedEntity>,
        line_of_sight_test: impl Fn(&SharedEntity) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            nearby_entities,
            line_of_sight_test: Arc::new(line_of_sight_test),
        }
    }

    #[must_use]
    pub fn empty() -> Self {
        Self {
            nearby_entities: Vec::new(),
            line_of_sight_test: Arc::new(|_| false),
        }
    }

    #[must_use]
    pub fn nearby_entities(&self) -> &[SharedEntity] {
        &self.nearby_entities
    }

    #[must_use]
    pub fn find_closest(&self, filter: impl Fn(&SharedEntity) -> bool) -> Option<SharedEntity> {
        self.nearby_entities
            .iter()
            .find(|entity| filter(entity) && (self.line_of_sight_test)(entity))
            .cloned()
    }

    pub fn find<'a>(
        &'a self,
        filter: impl Fn(&SharedEntity) -> bool + 'a,
    ) -> impl Iterator<Item = &'a SharedEntity> {
        self.nearby_entities
            .iter()
            .filter(move |entity| filter(entity) && (self.line_of_sight_test)(entity))
    }

    #[must_use]
    pub fn contains(&self, target: &SharedEntity) -> bool {
        self.nearby_entities
            .iter()
            .any(|entity| entity.id() == target.id())
            && (self.line_of_sight_test)(target)
    }

    #[must_use]
    pub fn contains_where(&self, filter: impl Fn(&SharedEntity) -> bool) -> bool {
        self.nearby_entities
            .iter()
            .any(|entity| filter(entity) && (self.line_of_sight_test)(entity))
    }
}
