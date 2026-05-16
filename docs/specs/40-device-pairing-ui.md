# Spec 40 — Device Pairing UI and Generic Pairing Surface

> Implementation-ready specification for a user-facing pairing flow for
> authenticated network devices, starting with Philips Hue and Nanoleaf and
> extending cleanly to secured WLED deployments.

**Status:** Implemented — `DevicePairingModal` and `ForgetCredentialsModal` shipped in `hypercolor-ui`
**Author:** Nova
**Date:** 2026-03-15
**Crates:** `hypercolor-ui`, `hypercolor-daemon`, `hypercolor-core`, `hypercolor-types`
**Related:** `docs/archive/33-network-device-backends.md`

---

## Table of Contents

1. [Overview](#1-overview)
2. [Problem Statement](#2-problem-statement)
3. [Goals and Non-Goals](#3-goals-and-non-goals)
4. [Pairing Boundary](#4-pairing-boundary)
5. [API Contract](#5-api-contract)
6. [UI and UX](#6-ui-and-ux)
7. [Post-Pair Activation](#7-post-pair-activation)
8. [Security and Credential Handling](#8-security-and-credential-handling)
9. [Implementation Plan](#9-implementation-plan)
10. [Recommendation](#10-recommendation)

---

## 1. Overview

Hypercolor can now discover Hue bridges and Nanoleaf controllers, but discovered
devices are not yet actionable from the UI when they require credentials. Users
can see them in the devices list, but they cannot complete the required physical
pairing flow or understand why direct actions fail.

This spec adds:

- A generic pairing capability surface exposed by the daemon
- A pairing-aware device summary returned by the API
- A pairing panel + modal flow in the UI
- Immediate post-pair activation so paired devices become usable without a
  manual daemon restart or guesswork
- A data model that can later support secured WLED with manual credentials

This spec is intentionally designed to reduce frontend/backend hardcoding:
the UI should render pairing from a backend-provided descriptor, not from
`match backend { "hue" => ..., "nanoleaf" => ... }` branches scattered across
the app.

---

## 2. Problem Statement

Current state:

- `GET /api/v1/devices` returns discovered Hue/Nanoleaf devices
- `POST /api/v1/devices/pair/hue` and `POST /api/v1/devices/pair/nanoleaf`
  exist
- The UI has no pairing affordance
- The API does not expose a clear pairing/auth state per device
- The current pair routes are backend-specific and not a stable generic UI
  boundary
- Successful pairing only stores credentials; it does not guarantee the device
  becomes immediately controllable

User-visible failure mode:

1. Discovery finds the device
2. The device appears in the devices page
3. The user tries actions like identify/brightness/enable
4. Backend connect fails because credentials are missing
5. The UI gives no obvious next step

This is especially bad for:

- Hue, where the bridge requires the link-button flow
- Nanoleaf, where the controller requires the power-button hold flow
- Future secured WLED devices, where the UI will need a manual credentials form

---

## 3. Goals and Non-Goals

### Goals

- Make unpaired network devices obviously actionable in the devices UI
- Expose pairing/auth state directly in `DeviceSummary`
- Replace backend-specific UI behavior with a generic pairing descriptor
- Support two flow families:
  - physical-action pairing (`press button`, `hold power`)
  - manual credential entry (future WLED)
- Attempt immediate activation after successful pairing
- Reuse existing device lifecycle and `device_state_changed` refresh behavior

### Non-Goals

- Dynamic backend loading or a full plugin/wasm runtime
- A complete credential-management dashboard
- OAuth/cloud account linking
- Multi-device bulk pairing in v1

Note: this spec does define a cleaner pairing interface that is compatible with a
future plugin boundary, but it does not require dynamic loading now.

---

## 4. Pairing Boundary

### 4.1 Design Principle

The UI should not know how Hue or Nanoleaf pairing works. It should only know:

- whether a device needs credentials
- whether pairing is supported
- what instructions to show
- what input fields to render, if any
- where to submit the pairing action

That means the backend must provide a generic pairing descriptor.

### 4.2 Generic Pairing Model

Add a pairing/auth summary to device API responses:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceAuthState {
    /// Device does not require credentials.
    Open,
    /// Device requires credentials and none are stored.
    Required,
    /// Credentials are stored and can be used for connect attempts.
    Configured,
    /// Credentials are stored but known to be invalid or stale.
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingFlowKind {
    /// User must perform a physical action, then press the action button.
    PhysicalAction,
    /// UI must render input fields and submit entered credentials.
    CredentialsForm,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingFieldDescriptor {
    pub key: String,
    pub label: String,
    pub secret: bool,
    pub optional: bool,
    pub placeholder: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingDescriptor {
    pub kind: PairingFlowKind,
    pub title: String,
    pub instructions: Vec<String>,
    pub action_label: String,
    #[serde(default)]
    pub fields: Vec<PairingFieldDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceAuthSummary {
    pub state: DeviceAuthState,
    pub can_pair: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub descriptor: Option<PairingDescriptor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}
```

Add it to the daemon/UI `DeviceSummary` response model:

```rust
pub struct DeviceSummary {
    // existing fields...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<DeviceAuthSummary>,
}
```

### 4.3 Backend-Side Interface

To avoid growing more backend-specific daemon code paths, network backends should
participate through a generic pairing interface.

Recommended shape:

```rust
#[async_trait]
pub trait DevicePairingProvider: Send + Sync {
    fn auth_summary(&self, device: &TrackedDeviceContext) -> Option<DeviceAuthSummary>;

    async fn pair(
        &self,
        device: &TrackedDeviceContext,
        request: PairDeviceRequest,
    ) -> anyhow::Result<PairDeviceOutcome>;

    async fn clear_credentials(&self, device: &TrackedDeviceContext) -> anyhow::Result<()>;
}
```

This does not require runtime plugins yet. It simply creates a clean seam:

- daemon API talks to a generic pairing provider
- Hue/Nanoleaf implement the provider
- future WLED auth can implement the same provider

---

## 5. API Contract

### 5.1 Generic Pair Endpoint

Add a device-scoped generic route:

```http
POST /api/v1/devices/:id/pair
Content-Type: application/json
```

Request:

```rust
pub struct PairDeviceRequest {
    /// Form-submitted values for flows like WLED username/password/token.
    #[serde(default)]
    pub values: std::collections::HashMap<String, String>,
    /// Attempt to connect the device immediately after successful pairing.
    #[serde(default = "default_true")]
    pub activate_after_pair: bool,
}
```

Response:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairDeviceStatus {
    Paired,
    ActionRequired,
    AlreadyPaired,
    InvalidInput,
}

pub struct PairDeviceResponse {
    pub status: PairDeviceStatus,
    pub message: String,
    pub activated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceSummary>,
}
```

Behavior by backend:

- Hue:
  - if link button has not been pressed: `action_required`
  - if pairing succeeds: `paired`
- Nanoleaf:
  - if power-button pairing mode is not active: `action_required`
  - if pairing succeeds: `paired`
- Secured WLED future:
  - invalid username/password/token: `invalid_input`
  - valid credentials: `paired`

### 5.2 Unpair / Forget Credentials

Add:

```http
DELETE /api/v1/devices/:id/pair
```

Behavior:

- remove stored credentials for the device
- publish a `device_state_changed` event with auth-related changes
- if the device is currently connected via authenticated transport, disconnect it

This is required for repairing stale Hue keys, reset Nanoleaf tokens, and future
secured WLED credentials.

### 5.3 Backward Compatibility

Existing backend-specific routes may stay temporarily:

- `POST /api/v1/devices/pair/hue`
- `POST /api/v1/devices/pair/nanoleaf`

But they should be treated as transitional/internal. The UI should use only:

- `GET /api/v1/devices`
- `GET /api/v1/devices/:id`
- `POST /api/v1/devices/:id/pair`
- `DELETE /api/v1/devices/:id/pair`

### 5.4 Refresh Signaling

Do not add a new websocket event type for v1.

Instead, after pair/unpair operations, publish:

```rust
HypercolorEvent::DeviceStateChanged {
    device_id,
    changes: {
        "auth_state": ...,
        "pairing_required": ...,
        "activated": ...,
    }
}
```

The UI already listens for `device_state_changed` and can refetch devices.

### 5.5 Rate Limiting

Pairing endpoints should get their own limiter in API security middleware.

Recommended initial budget:

- `PAIRING_LIMIT_PER_MIN = 6`

This should be separate from discovery rate limiting. Pairing often requires
several user retries while they press/hold the physical button.

---

## 6. UI and UX

### 6.1 Devices Grid

Add a pairing badge to device cards when:

- `device.auth.state == required`
- `device.auth.state == error`

Card behavior:

- show a high-signal pill: `"Pair required"` or `"Repair auth"`
- add a compact action button: `"Pair"`
- clicking the badge or action opens the pairing flow for that device

The card should not show backend-specific copy. It should use the generic
descriptor label/title from the API.

### 6.2 Device Detail Sidebar

Add a dedicated pairing panel near the top of the detail sidebar when
`device.auth.can_pair` is true.

States:

- `open`
  - no pairing panel
- `required`
  - show warning panel with title, instructions, primary CTA
- `configured`
  - show subtle `"Credentials configured"` status and a secondary `"Forget credentials"` action
- `error`
  - show error panel with retry + forget actions

While pairing is required:

- `Identify` action should be disabled
- brightness slider should be disabled
- explanatory helper text should say that direct control requires pairing first

`Enable` / `Disable` may remain available because that is user-state management,
not transport auth.

### 6.3 Pairing Modal

Use a modal, not a full page. The device detail sidebar remains the context anchor.

Modal layout:

1. Title from `PairingDescriptor.title`
2. Instruction checklist from `PairingDescriptor.instructions`
3. Optional credential form fields from `descriptor.fields`
4. Primary action button from `descriptor.action_label`
5. Secondary cancel button
6. Inline result area for:
   - `action_required`
   - validation errors
   - success state

Behavior:

- open from card or detail panel
- submit to `POST /api/v1/devices/:id/pair`
- on success:
  - refetch devices resource
  - keep modal visible briefly with success state
  - close automatically after ~800ms
  - toast success
- on `action_required`:
  - keep modal open
  - show returned message
  - allow immediate retry
- on error:
  - show inline error and toast

### 6.4 Hue UX Copy

Recommended default instructions:

1. Press the link button on the Hue Bridge.
2. Return here within 30 seconds.
3. Click **Pair Bridge**.

### 6.5 Nanoleaf UX Copy

Recommended default instructions:

1. Hold the Nanoleaf power button for 5-7 seconds.
2. Wait for the controller to enter pairing mode.
3. Click **Pair Device**.

### 6.6 Future WLED UX

When WLED auth is added, the same modal should render a credentials form, for example:

- username
- password
- token

No new UI surface should be required.

---

## 7. Post-Pair Activation

This is the missing piece between "credentials were stored" and "the device is now usable."

After successful pairing, the daemon should:

1. Persist credentials in `CredentialStore`
2. Refresh the affected device's auth summary
3. Attempt targeted activation for that device
4. Return a refreshed `DeviceSummary` in the pair response
5. Publish `DeviceStateChanged`

### Activation rules

- If the device is known, enabled, and its layout mapping allows auto-connect:
  - attempt immediate backend connect
- If immediate connect is not possible:
  - trigger a targeted discovery refresh for that backend/device
- If activation still cannot complete:
  - pairing succeeds, but `activated = false` and the response message explains why

This avoids the current broken-feeling outcome where pairing technically succeeds
but nothing changes in the UI or transport state.

---

## 8. Security and Credential Handling

- UI must never prefill or echo stored secret values
- Credential form fields marked `secret` must render as password inputs
- `DELETE /api/v1/devices/:id/pair` must not return stored values
- Pairing errors should avoid leaking secrets into API messages or logs
- Pairing UI should not store secrets in localStorage

The existing encrypted `CredentialStore` remains the source of truth.

---

## 9. Implementation Plan

### Wave 1: Daemon pairing model

- Add `DeviceAuthState`, `PairingDescriptor`, and `DeviceAuthSummary`
- Extend `DeviceSummary` in daemon + UI API types
- Compute auth summary for Hue/Nanoleaf from discovery metadata + credential store

Verification:

- `cargo check -p hypercolor-daemon -p hypercolor-ui`
- daemon API tests for `GET /api/v1/devices` auth summary serialization

### Wave 2: Generic pair/unpair routes

- Add `POST /api/v1/devices/:id/pair`
- Add `DELETE /api/v1/devices/:id/pair`
- Implement backend delegation for Hue and Nanoleaf
- Keep old backend-specific routes only as compatibility shims or remove them

Verification:

- targeted API tests for:
  - `action_required`
  - `paired`
  - `unpair`
  - immediate activation response

### Wave 3: UI API client + pairing modal

- Extend `crates/hypercolor-ui/src/api/devices.rs`
- Add pairing modal component
- Add device-card badge + quick action
- Add device-detail pairing panel

Verification:

- `cd crates/hypercolor-ui && trunk build`
- component tests where applicable
- manual browser verification with mocked `required` and `configured` states

### Wave 4: Action gating and refresh behavior

- Disable identify/brightness for `auth.state == required`
- Refetch devices after pair/unpair
- Ensure `device_state_changed` refreshes the devices resource correctly

Verification:

- manual UI verification against live Hue/Nanoleaf devices
- daemon/UI integration check:
  - device starts as `required`
  - pair succeeds
  - device transitions to `configured`
  - device becomes controllable without restarting the daemon

### Candidate files

- `crates/hypercolor-daemon/src/api/devices.rs`
- `crates/hypercolor-daemon/src/api/mod.rs`
- `crates/hypercolor-daemon/src/api/security.rs`
- `crates/hypercolor-daemon/tests/api_tests.rs`
- `crates/hypercolor-ui/src/api/devices.rs`
- `crates/hypercolor-ui/src/components/device_card.rs`
- `crates/hypercolor-ui/src/components/device_detail.rs`
- `crates/hypercolor-ui/src/components/device_pairing_modal.rs`
- `crates/hypercolor-ui/src/pages/devices.rs`

---

## 10. Recommendation

Implement pairing as a generic device capability, not as two special-case UI
paths for Hue and Nanoleaf.

That choice solves the immediate problem cleanly:

- users can finally pair and use discovered Hue/Nanoleaf devices
- the UI stops hardcoding backend behavior
- secured WLED can plug into the same surface later
- the daemon gets a much cleaner seam for future backend/plugin work

The concrete recommendation is:

1. add `auth` / pairing descriptors to `DeviceSummary`
2. add generic `POST/DELETE /api/v1/devices/:id/pair`
3. build the pairing UI from the descriptor, not from backend string checks
4. activate devices immediately after successful pairing whenever possible
