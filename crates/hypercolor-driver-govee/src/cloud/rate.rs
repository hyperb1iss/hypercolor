use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant};

const ACCOUNT_DAY: Duration = Duration::from_secs(24 * 60 * 60);
const DEVICE_MINUTE: Duration = Duration::from_secs(60);
const DEFAULT_ACCOUNT_DAY_LIMIT: u32 = 10_000;
const DEFAULT_ENDPOINT_MINUTE_LIMIT: u32 = 10;

#[derive(Debug, Clone)]
pub struct RateBudget {
    account_day_limit: u32,
    endpoint_minute_limit: u32,
    account_window: Option<RateWindow>,
    endpoint_windows: HashMap<String, RateWindow>,
}

#[derive(Debug, Clone, Copy)]
struct RateWindow {
    started_at: Instant,
    count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V1RateOperation {
    DeviceList,
    DeviceState,
    DeviceControl,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitRejection {
    pub scope: RateLimitScope,
    pub retry_after: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitScope {
    AccountDay,
    EndpointMinute,
}

impl Default for RateBudget {
    fn default() -> Self {
        Self {
            account_day_limit: DEFAULT_ACCOUNT_DAY_LIMIT,
            endpoint_minute_limit: DEFAULT_ENDPOINT_MINUTE_LIMIT,
            account_window: None,
            endpoint_windows: HashMap::new(),
        }
    }
}

impl RateBudget {
    #[must_use]
    pub fn with_limits(account_day_limit: u32, endpoint_minute_limit: u32) -> Self {
        Self {
            account_day_limit,
            endpoint_minute_limit,
            ..Self::default()
        }
    }

    pub fn reserve_v1(
        &mut self,
        operation: V1RateOperation,
        model: Option<&str>,
        device: Option<&str>,
    ) -> Result<(), RateLimitRejection> {
        self.reserve_v1_at(operation, model, device, Instant::now())
    }

    pub fn reserve_v1_at(
        &mut self,
        operation: V1RateOperation,
        model: Option<&str>,
        device: Option<&str>,
        now: Instant,
    ) -> Result<(), RateLimitRejection> {
        let account_window = self.account_window.get_or_insert(RateWindow {
            started_at: now,
            count: 0,
        });
        reserve_window(
            account_window,
            ACCOUNT_DAY,
            self.account_day_limit,
            now,
            RateLimitScope::AccountDay,
        )?;

        let endpoint_key = operation.endpoint_key(model, device);
        let endpoint_window = self
            .endpoint_windows
            .entry(endpoint_key)
            .or_insert(RateWindow {
                started_at: now,
                count: 0,
            });
        reserve_window(
            endpoint_window,
            DEVICE_MINUTE,
            self.endpoint_minute_limit,
            now,
            RateLimitScope::EndpointMinute,
        )
    }
}

impl V1RateOperation {
    fn endpoint_key(self, model: Option<&str>, device: Option<&str>) -> String {
        match self {
            Self::DeviceList => "v1:devices".to_owned(),
            Self::DeviceState => format!(
                "v1:state:{}:{}",
                model.unwrap_or_default(),
                device.unwrap_or_default()
            ),
            Self::DeviceControl => format!(
                "v1:control:{}:{}",
                model.unwrap_or_default(),
                device.unwrap_or_default()
            ),
        }
    }
}

impl fmt::Display for RateLimitRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.scope {
            RateLimitScope::AccountDay => write!(
                f,
                "Govee cloud account daily rate limit exceeded; retry after {:?}",
                self.retry_after
            ),
            RateLimitScope::EndpointMinute => write!(
                f,
                "Govee cloud endpoint rate limit exceeded; retry after {:?}",
                self.retry_after
            ),
        }
    }
}

impl std::error::Error for RateLimitRejection {}

fn reserve_window(
    window: &mut RateWindow,
    span: Duration,
    limit: u32,
    now: Instant,
    scope: RateLimitScope,
) -> Result<(), RateLimitRejection> {
    if now.duration_since(window.started_at) >= span {
        *window = RateWindow {
            started_at: now,
            count: 0,
        };
    }

    if window.count >= limit {
        return Err(RateLimitRejection {
            scope,
            retry_after: span.saturating_sub(now.duration_since(window.started_at)),
        });
    }

    window.count = window.count.saturating_add(1);
    Ok(())
}
