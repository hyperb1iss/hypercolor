use std::collections::HashMap;
use std::sync::Arc;

use hypercolor_cloud_client::DeviceAuthorizationSession;
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

use crate::cloud_connection::CloudConnectionRuntime;
use crate::cloud_socket::CloudSocketRuntime;

pub struct CloudState {
    pub login_sessions: Arc<Mutex<HashMap<Uuid, DeviceAuthorizationSession>>>,
    pub connection: Arc<RwLock<CloudConnectionRuntime>>,
    pub connection_prepare_lock: Arc<Mutex<()>>,
    pub socket: Arc<Mutex<CloudSocketRuntime>>,
}

impl Default for CloudState {
    fn default() -> Self {
        Self {
            login_sessions: Arc::new(Mutex::new(HashMap::new())),
            connection: Arc::new(RwLock::new(CloudConnectionRuntime::default())),
            connection_prepare_lock: Arc::new(Mutex::new(())),
            socket: Arc::new(Mutex::new(CloudSocketRuntime::default())),
        }
    }
}
