//! Extension seams for downstream daemon builds.

use std::any::{Any, TypeId, type_name};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use anyhow::Result;
use async_trait::async_trait;
use axum::Router;

use crate::api::AppState;
use crate::startup::DaemonState;

#[derive(Clone, Default)]
pub struct ExtensionRegistry {
    states: Arc<RwLock<HashMap<TypeId, ExtensionState>>>,
}

struct ExtensionState {
    type_name: &'static str,
    value: Arc<dyn Any + Send + Sync>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExtensionRegistryError {
    #[error("extension state {type_name} is already registered")]
    DuplicateState { type_name: &'static str },
}

impl ExtensionRegistry {
    pub fn insert<T>(&self, value: Arc<T>) -> Result<(), ExtensionRegistryError>
    where
        T: Any + Send + Sync + 'static,
    {
        let type_id = TypeId::of::<T>();
        let type_name = type_name::<T>();
        let mut states = self
            .states
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if states.contains_key(&type_id) {
            return Err(ExtensionRegistryError::DuplicateState { type_name });
        }

        states.insert(type_id, ExtensionState { type_name, value });
        Ok(())
    }

    #[must_use]
    pub fn get<T>(&self) -> Option<Arc<T>>
    where
        T: Any + Send + Sync + 'static,
    {
        let states = self
            .states
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let value = Arc::clone(&states.get(&TypeId::of::<T>())?.value);
        drop(states);
        value.downcast::<T>().ok()
    }

    #[must_use]
    pub fn contains<T>(&self) -> bool
    where
        T: Any + Send + Sync + 'static,
    {
        self.states
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(&TypeId::of::<T>())
    }

    #[must_use]
    pub fn state_names(&self) -> Vec<&'static str> {
        let mut names = self
            .states
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .map(|state| state.type_name)
            .collect::<Vec<_>>();
        names.sort_unstable();
        names
    }
}

pub trait ApiExtension: Send + Sync {
    fn name(&self) -> &'static str;

    fn mount_api_routes(&self, router: Router<Arc<AppState>>) -> Router<Arc<AppState>>;
}

#[async_trait]
pub trait DaemonLifecycleExtension: Send + Sync {
    fn name(&self) -> &'static str;

    async fn start(&self, _daemon: &DaemonState) -> Result<()> {
        Ok(())
    }

    async fn shutdown(&self, _daemon: &DaemonState) -> Result<()> {
        Ok(())
    }
}
