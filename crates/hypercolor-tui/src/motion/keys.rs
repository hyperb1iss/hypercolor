//! Unique identifiers for keyed motion effects.
//!
//! Each `MotionKey` represents an effect slot. When a new effect is added
//! with the same key via `MotionSystem::trigger`, tachyonfx's `EffectManager`
//! cancels the previous instance and starts the new one fresh. This prevents
//! stacking and gives clean handoff between events.

/// Identifies a unique effect slot in the motion system.
///
/// See `docs/specs/38-tui-motion-layer.md` §5.2 for the catalog mapping.
///
/// `Default` is implemented as `TitleShimmer` purely so that
/// `EffectManager<MotionKey>` (which derives `Default` requiring `K: Default`)
/// can be constructed empty. The default variant is never actually used as
/// a key — it's only required by the trait bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MotionKey {
    // ── Chrome ──────────────────────────────────────────────
    /// Title bar shimmer (always-on background animation).
    #[default]
    TitleShimmer,
    /// Idle breathing on borders after no input for N seconds.
    IdleBreathing,

    // ── Device events ───────────────────────────────────────
    /// Sweep-in animation when a device connects.
    DeviceArrival,
    /// Dissolve-out animation when a device disconnects.
    DeviceDeparture,

    // ── Effect transitions ──────────────────────────────────
    /// Crossfade when the active effect changes.
    EffectTransition,
    /// Brightness pulse when a control slider is moved.
    ControlPatch,

    // ── Scene events ────────────────────────────────────────
    /// Radial HSL ripple when a scene activates.
    SceneActivation,

    // ── System state ────────────────────────────────────────
    /// Persistent glitch when daemon connection is lost.
    ConnectionLost,
    /// Green flash when daemon connection restores.
    ConnectionRestored,
    /// Brief red flash for errors.
    ErrorFlash,

    // ── Navigation ──────────────────────────────────────────
    /// Dissolve transition between screens.
    ScreenTransition,
    /// Border glow on focus change.
    PanelFocus,

    // ── Reactive layers (continuous, never_complete) ────────
    /// Border brightness modulation driven by audio bass energy.
    SpectrumPulse,
    /// Background tint driven by canvas dominant color.
    CanvasBleed,
}
