//! Extension seams for downstream daemon builds.

use std::any::{Any, TypeId, type_name};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use anyhow::Result;
use async_trait::async_trait;
use axum::http::Method;
use utoipa_axum::router::OpenApiRouter;

use crate::app_state::AppState;
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

    fn mount_api_routes(
        &self,
        router: OpenApiRouter<Arc<AppState>>,
    ) -> OpenApiRouter<Arc<AppState>>;

    /// Routes this extension mounts that answer without a credential.
    ///
    /// A declared route skips bearer authentication only. The network
    /// access policy still applies, the request is rate-limited in the
    /// declared class for every caller, and the handler sees an anonymous
    /// request context that is never loopback. A declaration that names a
    /// route the engine serves, falls within the API docs paths or the MCP
    /// mount, or is not an exact path is dropped at router assembly, and
    /// the route stays authenticated. The engine cannot tell one
    /// extension's routes from another's, so an extension must declare only
    /// routes it mounts, and must not mount routes beneath the API docs
    /// paths, which are exempt from authentication as a whole.
    fn public_routes(&self) -> Vec<PublicRoute> {
        Vec::new()
    }
}

/// Rate budget a public route spends per client address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicRateClass {
    /// The read budget.
    Read,
    /// The write budget.
    Write,
    /// The pairing budget, the tightest the daemon has.
    Pairing,
}

/// A route an [`ApiExtension`] serves without a credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicRoute {
    method: Method,
    path: String,
    class: PublicRateClass,
}

impl PublicRoute {
    /// Declare `method path` public.
    ///
    /// `path` is written the way the extension mounts it, relative to the
    /// versioned API prefix (`/example/route` for `/api/v1/example/route`),
    /// and matches exactly. A `GET` declaration also covers `HEAD`.
    #[must_use]
    pub fn new(method: Method, path: impl Into<String>, class: PublicRateClass) -> Self {
        Self {
            method,
            path: path.into(),
            class,
        }
    }

    /// The declared method.
    #[must_use]
    pub const fn method(&self) -> &Method {
        &self.method
    }

    /// The declared path, relative to the versioned API prefix.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The declared rate class.
    #[must_use]
    pub const fn class(&self) -> PublicRateClass {
        self.class
    }
}

#[async_trait]
pub trait DaemonLifecycleExtension: Send + Sync {
    fn name(&self) -> &'static str;

    async fn start(&self, _daemon: &DaemonState) -> Result<()> {
        Ok(())
    }

    async fn api_ready(&self, _daemon: &DaemonState, _state: Arc<AppState>) -> Result<()> {
        Ok(())
    }

    async fn shutdown(&self, _daemon: &DaemonState) -> Result<()> {
        Ok(())
    }
}
