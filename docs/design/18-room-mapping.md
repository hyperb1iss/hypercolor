# Room Mapping & Physical Space Design

> From a single LED strip to an entire house -- Hypercolor's spatial intelligence system.

---

## 1. Scale Levels

Hypercolor must feel natural at every scale. A user with one WLED strip on their desk and a user with 40 devices across a house are both first-class citizens. The spatial system uses a **fractal containment model**: every scale level is a container that holds the level below it, and every container uses the same mapping primitives.

### Scale Hierarchy

```
Installation                           (event, stage, multi-building)
  └── House                            (all rooms, outdoor zones)
       └── Room                        (one physical room)
            └── Area                   (desk, shelf, entertainment center)
                 └── Device            (one controller or bridge)
                      └── Segment      (WLED segment, channel, zone)
                           └── LED     (individual addressable pixel)
```

### What Changes at Each Scale

| Scale | Typical LED Count | Typical Devices | Primary Challenge | Latency Budget |
|-------|-------------------|-----------------|-------------------|----------------|
| **Device** | 1 -- 1,008 | 1 | LED topology (strip, ring, matrix) | < 1ms |
| **PC Case** | 50 -- 2,000 | 3 -- 10 | Inter-device sync, zone packing | < 5ms |
| **Desk** | 100 -- 3,000 | 5 -- 15 | Mixed protocols (HID + WLED + OpenRGB) | < 10ms |
| **Room** | 200 -- 10,000 | 5 -- 30 | Physical distance, wall paths, accent/ambient split | < 20ms |
| **House** | 500 -- 50,000 | 10 -- 100+ | Multi-room orchestration, network hops, WiFi reliability | < 50ms |
| **Installation** | 1,000 -- 500,000+ | 20 -- 1,000+ | Precise timing, failover, show control | < 10ms (wired) |

### Automatic Scale Detection

The system infers the user's scale from their device inventory and suggests appropriate defaults:

```
1 -- 3 devices, no rooms defined       → Device/Desk mode (flat canvas)
4 -- 15 devices, 1 room defined        → Room mode (spatial editor)
16+ devices OR 2+ rooms                → House mode (floor plan + rooms)
Manually enabled via project settings  → Installation mode (show control)
```

Users can override this at any time. Scale detection sets the **default UI complexity**, not a capability gate.

---

## 2. Coordinate Systems

Four coordinate spaces, connected by a transformation chain. Every LED position exists simultaneously in all four.

### The Four Spaces

```
                Physical Space (meters/cm)
                       │
                       │  room_transform (translate, rotate, scale)
                       ▼
                Room Space (normalized 0.0 -- 1.0)
                       │
                       │  canvas_projection (viewport mapping)
                       ▼
                Canvas Space (0 -- 319, 0 -- 199)
                       │
                       │  spatial_sampler (bilinear interpolation)
                       ▼
                Effect Space (RGBA pixel values)
```

### 2.1 Physical Space

Real-world measurements. The user defines room dimensions in their preferred unit (metric or imperial, stored internally as centimeters).

```rust
/// Physical coordinates in centimeters, origin at room's floor-level corner
pub struct PhysicalCoord {
    pub x: f64,    // left-to-right when facing the room's "front" wall
    pub y: f64,    // floor-to-ceiling (vertical axis)
    pub z: f64,    // front-to-back (depth into room)
}

pub struct RoomDimensions {
    pub width: f64,     // cm (x-axis)
    pub height: f64,    // cm (y-axis, floor to ceiling)
    pub depth: f64,     // cm (z-axis)
}
```

Physical space is used for:
- Room layout and furniture placement
- Distance-based calculations (latency estimation, effect propagation speed)
- 3D visualization
- "How far is this LED from that one?" queries

### 2.2 Room Space

Normalized coordinates within a single room. All physical coordinates are projected into 0.0 -- 1.0 range. This is where most spatial reasoning happens.

```rust
/// Normalized room coordinates, (0,0) = front-left-floor, (1,1) = back-right-ceiling
pub struct RoomCoord {
    pub x: f64,    // 0.0 = left wall, 1.0 = right wall
    pub y: f64,    // 0.0 = floor, 1.0 = ceiling
    pub z: f64,    // 0.0 = front wall, 1.0 = back wall
}

impl RoomCoord {
    /// Project 3D room coord to 2D for canvas mapping.
    /// Default: top-down (x, z). Configurable per-room.
    pub fn project_2d(&self, projection: Projection) -> (f64, f64) {
        match projection {
            Projection::TopDown => (self.x, self.z),
            Projection::FrontWall => (self.x, 1.0 - self.y),
            Projection::LeftWall => (self.z, 1.0 - self.y),
            Projection::RightWall => (1.0 - self.z, 1.0 - self.y),
            Projection::BackWall => (1.0 - self.x, 1.0 - self.y),
            Projection::Custom(matrix) => matrix.transform(self),
        }
    }
}
```

Why normalized? Two reasons:
1. Effects don't care if your room is 3m or 6m wide. A wave that crosses the room should cross the room, regardless of dimensions.
2. Multi-room orchestration needs a common coordinate system. "Effect starts at x=0.0 in room A and continues at x=0.0 in room B" is meaningful regardless of room sizes.

### 2.3 Canvas Space

The 320x200 effect canvas. This is where the effect engine lives. All room-space coordinates are mapped to canvas pixels.

```rust
/// Canvas pixel coordinates
pub struct CanvasCoord {
    pub x: u32,    // 0 -- 319
    pub y: u32,    // 0 -- 199
}

/// The projection from room space to canvas space
pub struct CanvasViewport {
    /// Which region of room space maps to the canvas
    pub room_rect: Rect2D,    // (x_min, y_min, x_max, y_max) in room coords

    /// Canvas dimensions (always 320x200 currently)
    pub width: u32,
    pub height: u32,

    /// Aspect ratio handling
    pub fit: ViewportFit,
}

pub enum ViewportFit {
    /// Stretch room rect to fill canvas (may distort)
    Stretch,
    /// Fit room rect inside canvas (may letterbox)
    Contain,
    /// Fill canvas, cropping room rect if needed
    Cover,
}
```

**The 320x200 question: how does it map to a 5m x 3m room?**

For a single room, the entire canvas maps to the room's 2D projection. The default projection is top-down (bird's eye), so a 5m x 3m room maps to 320x200 pixels. That's 64 pixels per meter horizontally, 66.7 pixels per meter vertically. For a WLED strip at 60 LEDs/m, one meter of strip maps to ~64 canvas pixels -- more than enough resolution for smooth sampling.

For a house with multiple rooms, two strategies:

**Strategy A: Shared Canvas.** The entire house floor plan maps to 320x200. A 15m x 10m house gets ~21 pixels/meter. Still sufficient for most effects (gradients, waves, ambient washes), but fine-grained effects lose detail. Best for house-wide sweeping effects.

**Strategy B: Per-Room Canvas (default).** Each room gets its own 320x200 canvas. Cross-room effects use a compositor that tiles or blends room canvases. This preserves full detail everywhere and is the recommended approach.

```
Strategy A: Shared Canvas               Strategy B: Per-Room Canvas

┌──────────────────────────┐             ┌──────────┐ ┌──────────┐
│   320 x 200 canvas       │             │ 320x200  │ │ 320x200  │
│                          │             │ Room A   │ │ Room B   │
│  ┌──────┐  ┌──────┐     │             └──────────┘ └──────────┘
│  │Room A│  │Room B│     │             ┌──────────┐ ┌──────────┐
│  └──────┘  └──────┘     │             │ 320x200  │ │ 320x200  │
│  ┌──────────────────┐   │             │ Room C   │ │ Room D   │
│  │     Room C       │   │             └──────────┘ └──────────┘
│  └──────────────────┘   │
└──────────────────────────┘             Cross-room compositor
                                         blends at room boundaries
Low detail per room                      Full detail per room
Simple effects work great                Complex effects preserved
```

### 2.4 Effect Space

The RGBA pixel buffer produced by the effect engine. This is purely output -- the spatial sampler reads from it, effects write to it. Effects don't know about rooms, devices, or physical coordinates. They paint pixels on a canvas. The spatial system handles everything else.

### Transformation Chain

```rust
pub struct TransformChain {
    /// Physical → Room: normalize physical coords to 0-1
    physical_to_room: AffineTransform3D,

    /// Room → 2D: project 3D room to 2D plane
    projection: Projection,

    /// 2D Room → Canvas: map room plane to canvas pixels
    viewport: CanvasViewport,
}

impl TransformChain {
    /// Full pipeline: where does this physical LED sample the canvas?
    pub fn physical_to_canvas(&self, physical: PhysicalCoord) -> CanvasCoord {
        let room = self.physical_to_room.transform(physical);
        let room_2d = room.project_2d(self.projection);
        self.viewport.map_to_canvas(room_2d)
    }

    /// Inverse: given a canvas position, where is it in the room?
    pub fn canvas_to_physical(&self, canvas: CanvasCoord) -> PhysicalCoord {
        // Used by the editor: click on canvas → show physical position
        self.viewport.unmap(canvas)
            .then(|p| self.projection.unproject(p))
            .then(|r| self.physical_to_room.inverse().transform(r))
    }
}
```

### Coordinate Diagram

```
Physical Space (cm)                    Room Space (0-1)
  ┌─────────────────┐                  ┌──────────────────┐
  │ (0,0,0)         │                  │ (0,0)            │
  │   ●────────●    │   normalize      │   ●────────●     │
  │   │ Shelf  │    │  ──────────→     │   │        │     │
  │   │ strip  │    │                  │   │  0.3   │     │
  │   ●────────●    │                  │   ●────────●     │
  │       (150,120) │                  │       (0.5, 0.4) │
  │                 │                  │                   │
  │   (0, 300cm)    │                  │         (0, 1.0) │
  └─────────────────┘                  └──────────────────┘

                    ↓ project 2D + map to canvas

Canvas Space (pixels)
  ┌─────────────────────────────────────────────────┐
  │ (0,0)                                    (319,0)│
  │                                                  │
  │        ●════════════════●                        │
  │        │ sampled region │                        │
  │        ●════════════════●                        │
  │                (160, 80)                         │
  │                                                  │
  │ (0,199)                                (319,199)│
  └─────────────────────────────────────────────────┘
```

---

## 3. LED Topology Challenges

Real-world LED installations are messy. Strips wrap around corners. Hexagonal panels tile irregularly. A single WLED controller drives three separate physical strips daisy-chained on one data pin. The topology system must handle all of this without forcing the user into painful manual configuration.

### 3.1 Topology Primitives

```rust
pub enum LedTopology {
    /// Linear strip: LEDs in a line, optionally with density info
    Strip {
        count: u32,
        density: LedDensity,
    },

    /// 2D matrix: grid of LEDs (WLED matrix, Strimer, panel)
    Matrix {
        width: u32,
        height: u32,
        serpentine: bool,     // alternating row direction
        start_corner: Corner, // which corner is LED 0
    },

    /// Ring: LEDs arranged in a circle (fan ring, halo)
    Ring {
        count: u32,
        start_angle: f32,     // degrees, 0 = top
        direction: Winding,   // CW or CCW
    },

    /// Path: strip that follows an arbitrary 2D/3D polyline
    /// This is the key primitive for real-world installations
    Path {
        count: u32,
        density: LedDensity,
        waypoints: Vec<PhysicalCoord>,  // the path the strip follows
    },

    /// Scatter: individually-placed LEDs with no geometric relationship
    /// Used for: Hue bulbs, standalone accent lights, mixed zones
    Scatter {
        positions: Vec<PhysicalCoord>,
    },

    /// Composite: multiple topologies grouped as one logical device
    Composite {
        segments: Vec<(String, LedTopology)>,  // (name, topology)
    },
}

pub enum LedDensity {
    PerMeter(f32),     // 30, 60, 144 LEDs/m
    Total(u32),        // just a count, evenly spaced along path
    Custom(Vec<f32>),  // per-LED spacing in cm
}

pub enum Corner {
    TopLeft, TopRight, BottomLeft, BottomRight,
}

pub enum Winding {
    Clockwise, CounterClockwise,
}
```

### 3.2 Path Topology: The Workhorse

Most real installations are strips that follow paths. A strip goes along a shelf edge, wraps around a corner, continues up a wall, across the ceiling. The `Path` topology handles all of this.

```
Physical path of a shelf-edge strip:

   Waypoint 0        Waypoint 1
       ●═══════════════●
       (shelf left)     │  (corner, strip bends 90 degrees)
                        │
                        ●═══════════════●
                   Waypoint 2      Waypoint 3
                   (shelf front)    (shelf right)

LED positions are computed by:
1. Calculate total path length from waypoints
2. Space LEDs evenly along the path (respecting density)
3. Each LED gets a physical coordinate interpolated along the polyline
```

```rust
impl PathTopology {
    /// Compute LED positions along the waypoint path
    pub fn compute_led_positions(&self) -> Vec<PhysicalCoord> {
        let segments: Vec<f64> = self.waypoints.windows(2)
            .map(|w| w[0].distance_to(&w[1]))
            .collect();

        let total_length: f64 = segments.iter().sum();

        let spacing = match self.density {
            LedDensity::PerMeter(d) => 100.0 / d as f64,  // cm per LED
            LedDensity::Total(n) => total_length / n as f64,
            LedDensity::Custom(ref s) => { /* use per-LED spacing */ return self.custom_spacing(s); }
        };

        let mut positions = Vec::new();
        let mut distance_along = 0.0;
        let mut segment_idx = 0;
        let mut segment_offset = 0.0;

        for led_idx in 0..self.count {
            let target_distance = led_idx as f64 * spacing;

            // Walk along segments to find the position
            while segment_idx < segments.len()
                && segment_offset + segments[segment_idx] < target_distance
            {
                segment_offset += segments[segment_idx];
                segment_idx += 1;
            }

            if segment_idx < segments.len() {
                let t = (target_distance - segment_offset) / segments[segment_idx];
                let a = &self.waypoints[segment_idx];
                let b = &self.waypoints[segment_idx + 1];
                positions.push(a.lerp(b, t));
            }
        }

        positions
    }
}
```

### 3.3 Real-World Topology Scenarios

**Strip follows a shelf edge, wraps around corners:**
```
Topology: Path
Waypoints: [(0, 120, 50), (150, 120, 50), (150, 120, 0), (0, 120, 0)]
Density: 60 LEDs/m
Result: LEDs follow an L-shaped path along two shelf edges
```

**Strip goes up a wall, across the ceiling, down another wall:**
```
Topology: Path
Waypoints: [(0, 0, 0), (0, 250, 0), (300, 250, 0), (300, 0, 0)]
Density: 30 LEDs/m
Result: 250cm up + 300cm across + 250cm down = 800cm = ~240 LEDs
```

**Hexagonal panel cluster (Nanoleaf-style):**
```
Topology: Composite {
    segments: [
        ("Hex 1", Scatter { positions: [center1] }),
        ("Hex 2", Scatter { positions: [center2] }),
        ("Hex 3", Scatter { positions: [center3] }),
        ...
    ]
}
Note: Each hex panel is a single color point (like a Hue bulb).
Position defined by panel center in physical coords.
```

**WLED 2D Matrix (e.g., 16x16 panel):**
```
Topology: Matrix {
    width: 16,
    height: 16,
    serpentine: true,    // rows alternate direction (standard wiring)
    start_corner: BottomLeft,
}
Position and size defined on the room/canvas layout.
```

**Mixed zone -- strip + Hue bulb covering the same area:**
```
Zone: "Bookshelf"
Devices:
  - WLED Strip (60 LEDs, density 30/m, path along shelf edges)
  - Hue Bulb (1 LED, scatter at center of shelf)
Both sample from the same canvas region.
The strip gets per-LED spatial resolution.
The Hue bulb gets the average color of its scatter position.
```

### 3.4 Density Mismatch Handling

When LEDs of different densities coexist in the same spatial region, the sampler must handle them gracefully:

```
144 LEDs/m strip:  ●●●●●●●●●●●●●●●●●●  (high resolution, fine gradients)
 30 LEDs/m strip:  ●   ●   ●   ●   ●    (low resolution, broader sampling)
 Hue bulb:         ●                      (single point, area average)
```

The spatial sampler already handles this naturally -- each LED has its own canvas position and samples independently. But the **editor** should communicate density visually:

- High-density strips: shown as thick solid lines
- Low-density strips: shown as dashed lines with visible LED dots
- Single-point devices (Hue): shown as large circles with a glow radius indicating their area of influence

---

## 4. Room Editor

The room editor is the primary tool for mapping physical space. It lives in the Layout section of the web UI (accessible via sidebar icon or `Ctrl+3`) and operates in several modes depending on user needs and scale.

### 4.1 Editor Architecture

```
┌────────────────────────────────────────────────────────────────┐
│  🗺️ Layout                                       [?] [⚙️]     │
│  ┌──────────┬──────────┬──────────┬──────────┐                │
│  │ Room View│ Wall View│ 3D View  │ Photo    │                │
│  └──────────┴──────────┴──────────┴──────────┘                │
│ ┌──────────────────────────────────────┬──────────────────────┐│
│ │                                      │ Inspector Panel      ││
│ │                                      │                      ││
│ │          Canvas / Editor Area        │ ┌──────────────────┐ ││
│ │                                      │ │ Selected: WLED   │ ││
│ │    (interactive spatial editor       │ │ Desk Strip       │ ││
│ │     with real-time effect overlay)   │ │                  │ ││
│ │                                      │ │ LEDs: 120        │ ││
│ │                                      │ │ Density: 60/m    │ ││
│ │                                      │ │ Protocol: DDP    │ ││
│ │                                      │ │ IP: 192.168.1.42 │ ││
│ │                                      │ │                  │ ││
│ │                                      │ │ Position         │ ││
│ │                                      │ │ X: 0.35  Y: 0.72│ ││
│ │                                      │ │ Width: 0.40      │ ││
│ │                                      │ │ Rotation: 0 deg  │ ││
│ │                                      │ │                  │ ││
│ │                                      │ │ Canvas Region    │ ││
│ │                                      │ │ [auto] [manual]  │ ││
│ │                                      │ └──────────────────┘ ││
│ │                                      │                      ││
│ ├──────────────────────────────────────┤ ┌──────────────────┐ ││
│ │ Device Tray (unplaced devices)       │ │ Layers           │ ││
│ │ ┌──────┐ ┌──────┐ ┌──────┐          │ │ ☑ Furniture      │ ││
│ │ │WLED  │ │Prism │ │ Hue  │          │ │ ☑ Devices        │ ││
│ │ │Strip2│ │S #2  │ │Lamp  │          │ │ ☑ Effect overlay │ ││
│ │ └──────┘ └──────┘ └──────┘          │ │ ☐ Grid           │ ││
│ │                                      │ │ ☐ Measurements   │ ││
│ └──────────────────────────────────────┘ └──────────────────┘ ││
└────────────────────────────────────────────────────────────────┘
```

### 4.2 Room View (Top-Down Floor Plan)

The default and most important view. A bird's-eye 2D representation of the room.

```
┌────────────────────────────────────────────────────────────┐
│                        Room: Office                         │
│   5.0m x 3.5m                                   [+] [-] 🔄│
│ ┌──────────────────────────────────────────────────────────┐│
│ │                          BACK WALL                        ││
│ │    ┌──────────────────┐    ┌─────────┐                   ││
│ │    │    Bookshelf     │    │ Cabinet │                   ││
│ │    │ ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓ │    │         │                   ││
│ │    │ (shelf strip)    │    │         │                   ││
│ │    └──────────────────┘    └─────────┘                   ││
│ │                                                           ││
│ │  L                                                    R   ││
│ │  E    ┌────────────────────────────────┐              I   ││
│ │  F    │                                │              G   ││
│ │  T    │         Desk                   │              H   ││
│ │       │   ╔══════════╗  ╔══════════╗   │              T   ││
│ │  W    │   ║ Monitor 1║  ║ Monitor 2║   │                  ││
│ │  A    │   ║          ║  ║          ║   │              W   ││
│ │  L    │   ╚══════════╝  ╚══════════╝   │              A   ││
│ │  L    │ ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓ │              L   ││
│ │       │ (desk underglow strip)         │              L   ││
│ │       │                    ┌────────┐  │                  ││
│ │       │                    │ PC Case│  │                  ││
│ │       │                    │ ▓▓▓▓▓▓ │  │                  ││
│ │       │                    └────────┘  │                  ││
│ │       └────────────────────────────────┘                  ││
│ │                                                           ││
│ │    ◉ Hue Floor Lamp          ◉ Hue Desk Lamp             ││
│ │                                                           ││
│ │                         FRONT WALL (door)                 ││
│ └──────────────────────────────────────────────────────────┘│
│                                                              │
│  ▓ = LED strip (live color)   ◉ = point light (live color)  │
└──────────────────────────────────────────────────────────────┘
```

**Interactions:**
- **Pan**: click + drag background, or scroll wheel + Shift
- **Zoom**: scroll wheel, or pinch on trackpad
- **Select**: click device / furniture
- **Move**: drag selected item
- **Rotate**: `R` key or grab rotation handle
- **Resize**: drag corner handles (furniture only; strips resize by changing path)
- **Multi-select**: `Shift+click` or drag selection box
- **Align**: smart guides appear when dragging near other items
- **Snap to grid**: `G` to toggle, configurable grid size (10cm, 25cm, 50cm)
- **Undo/Redo**: `Ctrl+Z` / `Ctrl+Shift+Z` -- full operation history

### 4.3 Wall Views

For devices mounted on walls (vertical strips, wall panels, shelves), the room view alone is insufficient. Wall views show the room "unfolded" -- each wall rendered as a flat elevation.

```
                    BACK WALL (facing)
  ┌──────────────────────────────────────────────────┐
  │ ceiling                                          │
  │   ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓   │
  │   (ceiling edge strip)                           │
  │                                                  │
  │                                                  │
  │   ┌─────────────────┐       ┌─────────┐         │
  │   │    Bookshelf    │       │ Cabinet │         │
  │   │ ▓▓▓▓▓ shelf 3  │       │         │         │
  │   │ ▓▓▓▓▓ shelf 2  │       │         │         │
  │   │ ▓▓▓▓▓ shelf 1  │       │         │         │
  │   └─────────────────┘       └─────────┘         │
  │                                                  │
  │ floor                                            │
  └──────────────────────────────────────────────────┘

  Wall selector:  [Front]  [Back ●]  [Left]  [Right]
```

Wall views use the `FrontWall`, `BackWall`, `LeftWall`, `RightWall` projections from the coordinate system. Devices placed in wall view automatically get their correct 3D physical coordinates. Editing in one view updates all others.

### 4.4 Trace Mode

For defining the path of a strip that follows a complex route. Activated by selecting a strip device and clicking "Trace Path" in the inspector.

```
Trace Mode active -- click to add waypoints, double-click to finish

  ┌──────────────────────────────────────────────────────┐
  │                                                      │
  │   1●═══════════════════════●2                        │
  │                             ║                        │
  │                             ║                        │
  │                             ●3══════════●4           │
  │                                          ║           │
  │                                          ║           │
  │                                          ●5          │
  │                                                      │
  │  Waypoints: 5       Path length: 4.2m               │
  │  LEDs on path: 252 (60/m)                            │
  │  [Undo last] [Clear all] [Done]                      │
  └──────────────────────────────────────────────────────┘
```

Each click places a waypoint. The strip follows the polyline between waypoints. LEDs are distributed along the path according to density. The preview shows actual LED positions as colored dots (sampling the current effect) so the user can verify the mapping looks right.

**Trace mode works in all views** -- room view for horizontal paths, wall views for vertical paths. Tracing across views (start on floor, go up wall) is supported by switching views mid-trace.

### 4.5 Photo Overlay Mode

The fastest path from "I have LEDs" to "they're mapped." The user uploads a photo of their setup and places LEDs directly on it.

```
┌──────────────────────────────────────────────────────────┐
│  📷 Photo Mode                    [Adjust] [Opacity: 70%]│
│ ┌────────────────────────────────────────────────────────┐│
│ │                                                        ││
│ │    ╔════════════╗    ╔════════════╗                     ││
│ │    ║            ║    ║            ║    (actual photo    ││
│ │    ║  monitor   ║    ║  monitor   ║     of user's desk ││
│ │    ║            ║    ║            ║     as background)  ││
│ │    ╚════════════╝    ╚════════════╝                     ││
│ │                                                        ││
│ │  ●──●──●──●──●──●──●──●──●──●──●──●──●──●──●         ││
│ │  (user clicked along desk strip -- LEDs shown as dots) ││
│ │                                                        ││
│ │           ┌──────────────┐                              ││
│ │           │   PC case    │                              ││
│ │           │  ●●  ●● ●●  │                              ││
│ │           │  (fan rings) │                              ││
│ │           └──────────────┘                              ││
│ │                                                        ││
│ └────────────────────────────────────────────────────────┘│
│                                                            │
│  Tools: [Place Strip] [Place Point] [Place Ring] [Erase]  │
│  Active: WLED Desk Strip (42 of 120 LEDs placed)          │
└──────────────────────────────────────────────────────────┘
```

**Photo overlay workflow:**
1. Upload a photo (drag-and-drop or file picker)
2. Optionally set scale reference (click two points and enter real-world distance)
3. Select a device from the tray
4. Use placement tools to position LEDs on the photo
5. The photo + LED positions are saved together as a layout preset

**Placement tools:**
- **Place Strip**: click start, click end -- LEDs distribute evenly along the line. For curves, click intermediate waypoints (same as Trace mode but on the photo).
- **Place Point**: single click for point lights (Hue bulbs, single LEDs)
- **Place Ring**: click center, drag to set radius -- LEDs distribute around the circle
- **Place Matrix**: click two corners to define the rectangle, set dimensions
- **Erase**: click LEDs to remove misplaced ones

Photo mode is purely for positioning. It doesn't replace the room editor's coordinate system -- behind the scenes, photo pixel positions are mapped to room coordinates using the scale reference and photo orientation.

### 4.6 3D View (Optional)

A Three.js-powered 3D visualization of the room. Not required for setup, but invaluable for complex installations with vertical elements.

```
┌──────────────────────────────────────────────────────────┐
│  🧊 3D View                    [Orbit] [Pan] [Reset cam] │
│ ┌────────────────────────────────────────────────────────┐│
│ │                                                        ││
│ │              ╱─────────────────────╲                    ││
│ │             ╱  ceiling strip ▓▓▓▓   ╲                  ││
│ │            ╱───────────────────────── ╲                 ││
│ │           │wall                  wall │                 ││
│ │           │strip                strip │                 ││
│ │           │ ▓                      ▓ │                 ││
│ │           │ ▓    ╔════════╗        ▓ │                 ││
│ │           │ ▓    ║ monitor║        ▓ │                 ││
│ │           │ ▓    ╚════════╝        ▓ │                 ││
│ │           │ ▓  ▓▓▓▓▓ desk ▓▓▓▓▓▓  ▓ │                 ││
│ │           │       ┌─────┐            │                 ││
│ │           │       │ PC  │            │                 ││
│ │           └───────┴─────┴────────────┘                 ││
│ │            floor                                       ││
│ └────────────────────────────────────────────────────────┘│
│                                                            │
│  Camera: Orbit  │  Grid: On  │  Furniture: Wireframe      │
└──────────────────────────────────────────────────────────┘
```

The 3D view is built with Three.js (already in the Hypercolor web UI stack for the spatial editor). Furniture is rendered as translucent wireframe boxes. LED strips are rendered as glowing lines with real-time color from the effect engine. Point lights (Hue) are rendered as glowing spheres.

**3D view is read-mostly.** Users can orbit, pan, and zoom to inspect their layout, but primary editing happens in room view and wall views. Items can be repositioned by dragging in 3D, but this is secondary to the 2D workflow.

### 4.7 Furniture System

Furniture items provide spatial context for device placement. They're not decorative -- they define mounting surfaces, occlusion, and logical groupings.

```rust
pub struct FurnitureItem {
    pub id: String,
    pub name: String,
    pub item_type: FurnitureType,
    pub position: PhysicalCoord,      // center point
    pub dimensions: (f64, f64, f64),  // width, height, depth in cm
    pub rotation: f32,                // degrees around Y axis
    pub color: Option<String>,        // editor display color
}

pub enum FurnitureType {
    Desk,
    Shelf,
    Monitor,
    TVPanel,
    Bed,
    Couch,
    Table,
    Cabinet,
    PCCase,
    Custom { shape: FurnitureShape },
}

pub enum FurnitureShape {
    Rectangle,
    LShape { cutout: (f64, f64) },
    Circle { radius: f64 },
    Custom { points: Vec<(f64, f64)> },
}
```

**Built-in furniture presets:**
- Desk (various sizes: 120cm, 150cm, 180cm, L-shaped)
- Monitor (24", 27", 32", 34" ultrawide, 49" super ultrawide)
- Bookshelf (2-shelf, 3-shelf, 4-shelf, Kallax grid)
- TV (43", 55", 65", 75", 85")
- PC Case (ATX mid-tower, ATX full-tower, ITX)
- Bed (single, double, queen, king)
- Couch (2-seat, 3-seat, L-shaped)

Users can create custom furniture with the polygon tool or by entering dimensions. Furniture templates are shareable as JSON files.

---

## 5. Ambient/Accent Separation

Not all lighting serves the same purpose. A ceiling strip washing the room in color is fundamentally different from a shelf accent strip highlighting a display. The spatial system must understand these roles to produce coherent lighting.

### 5.1 Lighting Roles

```rust
pub enum LightingRole {
    /// Broad, atmospheric fill lighting. Soft gradients, slow transitions.
    /// Canvas sampling uses area averaging over a large region.
    Ambient {
        influence_radius: f64,  // cm -- how large an area this light "fills"
    },

    /// Directional, highlighting specific objects or areas.
    /// Canvas sampling at precise position with tight radius.
    Accent,

    /// Functional lighting (desk lamp, monitor backlight).
    /// May use different effect processing (bias lighting, color temp).
    Task {
        task_type: TaskLightType,
    },

    /// Display/showcase lighting (PC case interior, display shelf).
    /// Full effect detail, high spatial resolution.
    Feature,
}

pub enum TaskLightType {
    MonitorBacklight,   // bias lighting, may derive from screen content
    DeskLamp,           // warm white bias, reduced saturation
    ReadingLight,       // color temperature priority
}
```

### 5.2 How Roles Affect Effect Rendering

```
Canvas (320 x 200)
┌────────────────────────────────────────────────────────────┐
│ ▓▓░░▓▓░░▓▓░░▓▓░░▓▓░░▓▓░░▓▓░░▓▓░░▓▓░░▓▓░░▓▓░░▓▓░░▓▓░░ │
│ ░░▓▓░░▓▓░░▓▓░░▓▓░░▓▓░░▓▓░░▓▓░░▓▓░░▓▓░░▓▓░░▓▓░░▓▓░░▓▓ │
│  (effect rendering -- full detail, complex animation)      │
└────────────────────────────────────────────────────────────┘
                          │
              ┌───────────┼───────────────┐
              ▼           ▼               ▼
        ┌──────────┐ ┌──────────┐  ┌──────────────┐
        │ Feature  │ │ Accent   │  │ Ambient      │
        │          │ │          │  │              │
        │ Full     │ │ Precise  │  │ Area-average │
        │ detail,  │ │ point    │  │ gaussian     │
        │ per-LED  │ │ sample   │  │ blur over    │
        │ sampling │ │ w/ small │  │ large region │
        │          │ │ radius   │  │              │
        └──────────┘ └──────────┘  └──────────────┘
        PC case LEDs  Shelf strip   Hue floor lamp
        Strimer       Wall accent   Ceiling strip
```

**Feature devices** get the raw canvas sample at each LED position. Maximum spatial resolution. A 144 LED/m strip in the PC case shows every detail of the effect.

**Accent devices** get a precise canvas sample but with a small Gaussian blur radius (configurable, default 2-4 canvas pixels). This smooths out single-pixel noise in effects while preserving the spatial pattern.

**Ambient devices** get an area-averaged sample. A Hue floor lamp in the corner of the room samples the canvas over a large region (its `influence_radius` mapped to canvas coordinates) and returns the weighted average color. This prevents ambient lights from "flickering" with high-frequency effect patterns.

**Task devices** get special processing:
- **Monitor backlight**: optionally uses screen capture input instead of the effect canvas, creating bias lighting
- **Desk lamp**: applies a warmth bias (shift toward 2700K-4000K) to maintain task-appropriate color temperature
- **Reading light**: respects a minimum brightness floor and limits saturation

### 5.3 Role Assignment

Roles are set per-device or per-segment in the inspector panel. The system suggests roles based on device type and position:

| Device Type | Position | Suggested Role |
|-------------|----------|----------------|
| Hue bulb | Floor/standing | Ambient |
| Hue bulb | Desk | Task (desk lamp) |
| WLED strip | Ceiling edge | Ambient |
| WLED strip | Shelf front | Accent |
| WLED strip | Behind monitor | Task (backlight) |
| WLED strip | Inside PC case | Feature |
| OpenRGB device | Inside PC case | Feature |
| PrismRGB Strimer | Inside PC case | Feature |

Users can override any suggestion. The role can also be set per-scene (a strip might be Ambient during "Movie Night" but Feature during "Gaming").

---

## 6. WLED Segment Support

WLED controllers are the backbone of room-scale installations. A single ESP32 running WLED can drive hundreds of LEDs, and WLED's segment system allows one controller to behave as multiple independent devices. Hypercolor must deeply integrate with this.

### 6.1 WLED Segment Discovery

```rust
pub struct WledDevice {
    pub ip: IpAddr,
    pub name: String,
    pub total_leds: u32,
    pub segments: Vec<WledSegment>,
    pub matrix: Option<WledMatrix>,
}

pub struct WledSegment {
    pub id: u32,
    pub start: u32,     // first LED index in the physical strip
    pub stop: u32,      // last LED index (exclusive)
    pub name: String,
    pub grouping: u32,  // LEDs per virtual pixel
    pub spacing: u32,   // skip N LEDs between groups
}

pub struct WledMatrix {
    pub width: u32,
    pub height: u32,
    pub serpentine: bool,
}
```

When Hypercolor discovers a WLED device, it queries the [WLED JSON API](http://[ip]/json) to enumerate segments. Each segment becomes a **virtual device** in Hypercolor's device tree.

### 6.2 Virtual Device Mapping

One physical WLED controller with 300 LEDs might be wired to three separate strips daisy-chained:

```
Physical wiring:
  ┌─────────────────────────────────────────────────┐
  │ ESP32 ──data──> Strip A (0-99) ──> Strip B      │
  │                 (desk back)     (100-219)        │
  │                                 (shelf left)     │
  │                                 ──> Strip C      │
  │                                     (220-299)    │
  │                                     (shelf right)│
  └─────────────────────────────────────────────────┘

WLED segments:
  Segment 0: start=0,   stop=100,  name="Desk Back"
  Segment 1: start=100, stop=220,  name="Shelf Left"
  Segment 2: start=220, stop=300,  name="Shelf Right"

Hypercolor virtual devices:
  ┌──────────────────────┐
  │ WLED 192.168.1.42    │
  │ ├── Desk Back (100)  │  ← maps to position behind desk
  │ ├── Shelf Left (120) │  ← maps to left shelf edge
  │ └── Shelf Right (80) │  ← maps to right shelf edge
  └──────────────────────┘
```

Each virtual device can be:
- Independently positioned in the room editor
- Assigned different topologies (one is a Path along the desk, two are Paths along shelf edges)
- Given different lighting roles (desk strip = Task/backlight, shelf strips = Accent)
- Part of different room zones and scenes

### 6.3 Hypercolor-Controlled vs. WLED-Native

Two operating modes for WLED segments:

**Mode A: Hypercolor-Controlled (default)**
- Hypercolor sends raw LED colors via DDP
- WLED's built-in effects are disabled
- Hypercolor's effect engine drives everything
- Maximum spatial integration -- the strip is part of the room's unified effect

**Mode B: WLED-Native**
- WLED runs its own effect on this segment
- Hypercolor doesn't send color data
- Used when WLED's built-in effects are preferred (e.g., for a segment in another room not yet mapped)
- Hypercolor can still set the WLED effect and parameters via the JSON API

```rust
pub enum WledSegmentMode {
    /// Hypercolor sends per-LED colors via DDP
    HypercolorControlled,

    /// WLED runs its own effect; Hypercolor sets effect parameters only
    WledNative {
        effect_id: u32,
        palette_id: u32,
        speed: u8,
        intensity: u8,
    },

    /// WLED segment is ignored entirely
    Disabled,
}
```

### 6.4 WLED 2D Matrix Support

WLED v0.14+ supports 2D matrix layouts. When Hypercolor detects a WLED device with a matrix configuration, it offers matrix-specific features:

```
WLED 2D Matrix (16x16):

Hypercolor can either:

A) Treat as a spatial zone -- map the 16x16 grid to a region
   of the 320x200 canvas. Each matrix pixel samples its canvas
   position. The matrix displays a zoomed-in portion of the effect.

B) Treat as a display -- send the entire 320x200 canvas downscaled
   to 16x16 and push it as a complete frame. The matrix shows a
   thumbnail of the full effect.

C) Direct matrix effect -- run a separate effect instance sized
   to the matrix resolution (16x16) and push it directly.
```

For most setups, option A is correct -- the matrix is a window into the room's effect, positioned where it physically sits. Option B is useful for "status display" use cases. Option C is for when the matrix should do its own thing independently.

---

## 7. Multi-Room Orchestration

When Hypercolor grows beyond a single room, the system needs orchestration primitives that coordinate effects, timing, and state across independent spaces.

### 7.1 Room & Group Hierarchy

```rust
pub struct House {
    pub name: String,
    pub rooms: Vec<Room>,
    pub room_groups: Vec<RoomGroup>,
    pub floor_plan: Option<FloorPlan>,
}

pub struct Room {
    pub id: String,
    pub name: String,
    pub dimensions: RoomDimensions,
    pub devices: Vec<DeviceMapping>,
    pub furniture: Vec<FurnitureItem>,
    pub canvas: CanvasInstance,           // per-room effect canvas
    pub adjacency: Vec<RoomAdjacency>,   // which rooms share walls
}

pub struct RoomGroup {
    pub name: String,
    pub room_ids: Vec<String>,
    pub sync_mode: GroupSyncMode,
}

pub enum GroupSyncMode {
    /// Same effect runs independently in each room
    SameEffect,

    /// Effect is spatially continuous across rooms
    /// (requires adjacency data)
    ContinuousEffect,

    /// Each room runs a different effect, synchronized in time
    TimeSynced,
}

pub struct RoomAdjacency {
    pub room_id: String,
    pub shared_wall: Wall,           // which wall is shared
    pub alignment: WallAlignment,    // how the walls align
}
```

### 7.2 House-Level Floor Plan

```
┌────────────────────────────────────────────────────────────────┐
│  🏠 House: Bliss's Place                                       │
│  ┌───────────┬──────────┬──────────┐                           │
│  │ Floor Plan│ Room List│ Groups   │                           │
│  └───────────┴──────────┴──────────┘                           │
│ ┌──────────────────────────────────────────────────────────────┐│
│ │                                                              ││
│ │   ┌───────────────┐  ┌─────────────────────┐                ││
│ │   │               │  │                     │                ││
│ │   │   Bedroom     │  │    Living Room      │                ││
│ │   │   🟢 4 devs   │  │    🟢 6 devs        │                ││
│ │   │   "Nightlight"│  │    "Movie Night"    │                ││
│ │   │               │  │                     │                ││
│ │   └───────┬───────┘  └──────────┬──────────┘                ││
│ │           │                     │                            ││
│ │   ┌───────┴───────┐  ┌─────────┴──────────┐                ││
│ │   │               │  │                     │                ││
│ │   │   Hallway     │  │    Kitchen          │                ││
│ │   │   🟡 1 dev    │  │    🟢 3 devs        │                ││
│ │   │   (ambient)   │  │    "Cooking"        │                ││
│ │   │               │  │                     │                ││
│ │   └───────────────┘  └────────────────────┘                ││
│ │                                                              ││
│ │   ┌──────────────────────────────────────┐                   ││
│ │   │               Office                 │                   ││
│ │   │               🟢 14 devs             │                   ││
│ │   │               "ADHD Hyperfocus"      │                   ││
│ │   └──────────────────────────────────────┘                   ││
│ │                                                              ││
│ └──────────────────────────────────────────────────────────────┘│
│                                                                  │
│  Active scene: "Evening"  │  Total: 28 devices, 4,200 LEDs     │
│  [Edit floor plan]  [Create scene]  [Sync all rooms]            │
└──────────────────────────────────────────────────────────────────┘
```

The floor plan is a simplified top-down layout of rooms. It's not an architectural drawing -- it's a spatial relationship map. Rooms are rectangles (or simple polygons) positioned relative to each other, showing:
- Room name and current effect/scene
- Device count and online status
- Adjacency (shared walls for continuous effects)
- Click to drill into room-level editor

### 7.3 Cross-Room Effects

When rooms share a wall and the user enables `ContinuousEffect` sync mode, effects flow seamlessly across the boundary.

```
Room A (Office)                    Room B (Hallway)
Canvas A (320x200)                 Canvas B (320x200)

┌──────────────────────────┐      ┌──────────────────────────┐
│                          │      │                          │
│  effect wave →→→→→→→→→→  │      │  →→→→→→→→→→→ continues  │
│                          │      │                          │
│                    ▓▓▓▓▓▓│══════│▓▓▓▓▓                    │
│                (strip A   shared  strip B)                 │
│                 ends at   wall    starts at                │
│                 right edge        left edge                │
│                          │      │                          │
└──────────────────────────┘      └──────────────────────────┘
```

**Implementation: Room Compositor**

```rust
pub struct RoomCompositor {
    pub rooms: Vec<RoomCanvas>,
    pub adjacencies: Vec<AdjacencyLink>,
}

pub struct AdjacencyLink {
    pub room_a: String,
    pub room_b: String,
    pub wall_a: Wall,
    pub wall_b: Wall,
    /// How far along each wall the overlap region sits (0.0 - 1.0)
    pub alignment: (f64, f64),
    /// Width of the blending region in canvas pixels
    pub blend_width: u32,
}

impl RoomCompositor {
    /// Run a single effect across multiple rooms by creating
    /// a virtual super-canvas, rendering the effect on it,
    /// then extracting per-room slices.
    pub fn render_continuous(
        &self,
        effect: &EffectInstance,
        inputs: &InputData,
    ) -> HashMap<String, Canvas> {
        // 1. Calculate the composite canvas size based on room arrangement
        let (total_width, total_height) = self.composite_dimensions();

        // 2. Render effect at composite resolution
        let composite = effect.render_at_size(total_width, total_height, inputs);

        // 3. Slice composite into per-room canvases
        self.rooms.iter().map(|room| {
            let slice = composite.extract_region(room.composite_rect);
            let canvas = slice.resize(320, 200); // back to standard resolution
            (room.id.clone(), canvas)
        }).collect()
    }
}
```

### 7.4 Per-Room Overrides

In a multi-room scene, each room can have independent adjustments:

```rust
pub struct RoomOverrides {
    /// Brightness multiplier (0.0 = off, 1.0 = normal, 2.0 = double)
    pub brightness: f32,

    /// Effect speed multiplier (0.5 = half speed, 2.0 = double speed)
    pub speed: f32,

    /// Color temperature shift (in Kelvin delta, -2000 to +2000)
    pub color_temp_shift: f32,

    /// Saturation multiplier (0.0 = grayscale, 1.0 = normal)
    pub saturation: f32,

    /// Effect-specific parameter overrides
    pub param_overrides: HashMap<String, ControlValue>,
}
```

**Scene example: "Movie Night"**

| Room | Effect | Brightness | Speed | Notes |
|------|--------|------------|-------|-------|
| Living Room | Aurora | 30% | 0.5x | Low, slow ambient behind TV |
| Kitchen | Solid Color | 10% | -- | Warm white 2700K, just enough to see |
| Hallway | Solid Color | 5% | -- | Very dim warm white, wayfinding only |
| Bedroom | Off | 0% | -- | Fully dark |
| Office | Independent | 100% | 1.0x | Not part of this scene |

### 7.5 Latency Compensation

Devices at different network distances have different latencies. A USB HID device responds in < 1ms. A WLED device over WiFi might take 5-20ms. Hue over the bridge adds 20-50ms. For cross-room synchronized effects, this matters.

```rust
pub struct LatencyProfile {
    pub device_id: String,
    /// Measured round-trip latency in milliseconds
    pub measured_rtt_ms: f64,
    /// Estimated one-way latency (RTT / 2)
    pub estimated_one_way_ms: f64,
    /// Jitter (standard deviation of latency measurements)
    pub jitter_ms: f64,
}

impl LatencyCompensator {
    /// Pre-advance the effect time for high-latency devices
    /// so they display "in sync" with low-latency ones.
    pub fn compensate(&self, base_time: f64, device: &str) -> f64 {
        let profile = self.profiles.get(device)?;
        base_time + (profile.estimated_one_way_ms / 1000.0)
    }
}
```

The daemon periodically measures latency to each device (DDP ping for WLED, frame ack timing for HID) and adjusts the frame time sent to high-latency devices. This won't achieve perfect sync, but it reduces visible lag from "noticeable" to "imperceptible" for typical home setups.

---

## 8. Photo-Based Setup

Photo overlay mode is described in Section 4.5, but the broader photo-based setup system deserves deeper treatment. This is the primary onboarding path for users who don't want to manually measure their room.

### 8.1 Photo Import Pipeline

```
                User takes photo
                       │
                       ▼
              ┌──────────────────┐
              │  Import & Crop   │  → Auto-rotate, crop to room area
              └────────┬─────────┘
                       │
                       ▼
              ┌──────────────────┐
              │  Scale Reference │  → Click two points, enter distance
              └────────┬─────────┘    (e.g., "this desk is 150cm wide")
                       │
                       ▼
              ┌──────────────────┐
              │  Place Devices   │  → Click/trace to position LEDs
              └────────┬─────────┘
                       │
                       ▼
              ┌──────────────────┐
              │  Verify & Save   │  → See live effect on photo overlay
              └──────────────────┘
```

### 8.2 Scale Reference System

Without a scale reference, photo positions are just pixel coordinates with no physical meaning. The scale reference bridges photo space to physical space.

```
Scale Reference Dialog:

  ┌──────────────────────────────────────────────────────┐
  │  Set a reference distance                            │
  │  Click two points on something you can measure.      │
  │                                                      │
  │  [photo with two marked points]                      │
  │                                                      │
  │       ●═══════════════════════●                      │
  │       A                       B                      │
  │                                                      │
  │  Distance A → B:  [ 150 ] cm                         │
  │                                                      │
  │  Calculated scale: 1 pixel = 0.42 cm                 │
  │                                                      │
  │               [Skip]  [Apply]                        │
  └──────────────────────────────────────────────────────┘
```

The user clicks two points on a known object (desk width, doorframe height, monitor diagonal) and enters the real-world distance. The system calculates pixels-per-centimeter and uses this to convert all subsequent LED placements to physical coordinates.

Multiple reference points can be set for better accuracy (least-squares fit for perspective distortion). But a single reference is good enough for most setups.

### 8.3 Multi-Photo Layouts

A single photo rarely captures an entire room. The system supports multiple photos stitched together:

- **Per-wall photos**: one photo per wall, each mapped to a wall view
- **Per-area photos**: close-up of desk, wide shot of room, detail of shelf
- **Panorama**: automatic alignment of overlapping photos (stretch goal)

Each photo is an independent overlay layer. The user switches between photos in the editor, and device placements persist across all photos (since they're stored in physical coordinates, not photo pixels).

### 8.4 Computer Vision Stretch Goal: Auto-Detect LED Positions

For users with addressable LED strips, auto-detection is possible:

```
Auto-Detect Workflow:

1. Hypercolor turns ALL LEDs off (black)
2. User takes a photo of the dark room
3. Hypercolor turns ALL LEDs to full white
4. User takes a photo of the lit room
5. Difference between photos = LED positions
6. Hypercolor sequentially lights LEDs (1 at a time, or binary coding)
   while user records a video
7. Computer vision extracts position and order of each LED

Result: fully automatic LED position mapping
```

This is how xLights' `xlCapture` tool works. The core algorithm:
1. Subtract dark frame from lit frame = LED mask
2. For each frame in the sequence, identify which LEDs are lit
3. Binary encoding: LED N is lit when frame number has bit N set
4. 10 frames can identify 1024 LEDs (2^10)

This requires a camera that can see the LEDs (webcam or phone camera) and is a **Phase 4+** feature. The infrastructure (WLED control, frame capture, image differencing) all exists; the integration is the challenge.

### 8.5 Export & Share

Layouts (with or without photos) can be exported and shared:

```json
{
  "name": "Bliss's Office",
  "version": "1.0",
  "room": {
    "width": 500,
    "height": 250,
    "depth": 350
  },
  "furniture": [
    {
      "type": "desk",
      "preset": "180cm-lshape",
      "position": [250, 0, 175],
      "rotation": 0
    }
  ],
  "devices": [
    {
      "name": "Desk Underglow",
      "type": "wled_strip",
      "topology": {
        "type": "path",
        "count": 120,
        "density_per_meter": 60,
        "waypoints": [
          [100, 0, 130], [400, 0, 130], [400, 0, 220], [100, 0, 220]
        ]
      },
      "role": "accent"
    }
  ],
  "photos": [
    {
      "name": "Desk front view",
      "file": "desk-front.jpg",
      "scale_px_per_cm": 2.4,
      "view": "front_wall"
    }
  ]
}
```

Photos are stored as separate files alongside the JSON layout. Shared layouts can include or exclude photos (privacy consideration -- your room photo may show personal items).

---

## 9. Persona Scenarios

### 9.1 Bliss's Room: The Power User Paradise

**Setup inventory:**
- PC case: 12 internal devices via OpenRGB + PrismRGB (fans, RAM, GPU, Strimers, header strips)
- Desk: underglow WLED strip (120 LEDs, 60/m), keyboard backlight, mouse lighting
- Monitors: 2x backlight WLED strips (90 LEDs each)
- Walls: 4x WLED strips (2x vertical flanking desk, 2x horizontal shelf accents)
- Shelves: 2x WLED strips (60 LEDs each, accent lighting)
- Hue desk lamp (1 bulb)
- Hue floor lamp (1 bulb)
- Total: ~2,400+ LEDs across ~20 logical devices

**Room layout (top-down):**
```
                      BACK WALL (5.0m)
  ┌──────────────────────────────────────────────────────────┐
  │                                                          │
  │  ┌───────────┐   ┌───────────┐                          │
  │  │ Bookshelf │   │ Bookshelf │                          │
  │  │ ═══ S6    │   │ ═══ S7    │                          │
  │  │ ═══ S5    │   │ ═══       │                          │
  │  └───────────┘   └───────────┘                          │
  │                                                          │
  │  ▐ W1 (vert)                            ▐ W2 (vert)     │
  │  ▐                                      ▐               │
  │  ▐        ╔═══════════╗ ╔═══════════╗   ▐               │
  │  ▐        ║ Monitor 1 ║ ║ Monitor 2 ║   ▐               │
  │  ▐   M1 ─ ║           ║ ║           ║ ─ M2              │
  │  ▐        ╚═══════════╝ ╚═══════════╝   ▐               │
  │  ▐                                      ▐               │
  │  ▐   ┌──────────────────────────────┐   ▐               │
  │       │          D E S K            │                    │
  │       │  ═══════ underglow DU ══════│                    │
  │       │                     ┌──────┐│                    │
  │       │                     │ PC   ││                    │
  │       │                     │ Case ││                    │
  │       │                     │ ▓▓▓▓ ││                    │
  │       └─────────────────────┴──────┘│                    │
  │                                                          │
  │  ═══ W3 (horiz, shelf height)     ═══ W4 (horiz)        │
  │                                                          │
  │  ◉ Hue Floor Lamp                  ◉ Hue Desk Lamp      │
  │                                                          │
  │                    FRONT WALL (door)                      │
  └──────────────────────────────────────────────────────────┘

  Legend:
    ═══  WLED strip    ▐  Vertical WLED strip
    ◉    Hue bulb      ▓  PC case (OpenRGB + PrismRGB)
    ─    Monitor backlight strip
```

**Effect flow: One unified effect across everything.**

All devices share a single 320x200 canvas. The effect engine renders "ADHD Hyperfocus" -- a custom audio-reactive shader. The spatial sampler maps each LED to its canvas position:

- **PC case** (Feature role): maps to bottom-right quadrant of canvas. High detail, per-LED sampling. Strimers use Matrix topology, fans use Ring topology.
- **Desk underglow** (Feature role): maps to bottom center. Path topology following desk perimeter.
- **Monitor backlights** (Task role): map to upper center. Optionally switch to screen capture input for bias lighting.
- **Wall strips** (Accent role): vertical strips map to left/right edges, horizontal strips to mid-height band.
- **Shelf strips** (Accent role): map to upper quadrants.
- **Hue bulbs** (Ambient role): area-average sampling. Floor lamp covers bottom-left quadrant. Desk lamp covers center.

When music is playing, the entire room responds as a unified organism. Bass hits pulse the floor lamp and desk underglow. Treble sparkles through the wall strips and PC case. The monitors maintain their bias lighting independently. The effect flows spatially -- a wave that starts on the left wall strip, crosses through the monitors, through the PC case, and reaches the right wall strip.

### 9.2 Alex's House: Multi-Room Harmony

**Living Room:**
- TV backlight (WLED, 90 LEDs)
- Ceiling strip (WLED, 200 LEDs, ambient)
- 4x Hue bulbs (2 floor lamps, 2 table lamps)

**Kitchen:**
- Under-cabinet WLED strip (180 LEDs, accent)
- Pendant Hue bulb (ambient)

**Bedroom:**
- Headboard WLED strip (90 LEDs, accent/feature)
- 2x Bedside Hue bulbs (ambient)

**Office:**
- Full setup similar to Bliss's (simplified: 8 devices, ~1,200 LEDs)

**House floor plan:**
```
  ┌──────────────────┬──────────────────────┐
  │                  │                      │
  │    Bedroom       │    Living Room       │
  │    🔵 3 devs     │    🟢 6 devs         │
  │    "Nightlight"  │    "Movie Night"     │
  │                  │                      │
  ├──────────────────┤                      │
  │                  │                      │
  │    Office        │                      │
  │    🟢 8 devs     │                      │
  │    "Focus"       │                      │
  │                  ├──────────────────────┤
  │                  │                      │
  │                  │    Kitchen           │
  │                  │    🟢 2 devs         │
  │                  │    "Cooking"         │
  │                  │                      │
  └──────────────────┴──────────────────────┘
```

**Multi-room scene: "Movie Night"**

Alex activates "Movie Night" from the Hypercolor app on her phone. The scene applies per-room settings:

| Room | Effect | Brightness | Role |
|------|--------|------------|------|
| Living Room | Screen Ambience (TV capture) | 40% | Ambient wash from TV content |
| Kitchen | Solid Warm White (2700K) | 10% | Just enough for snack runs |
| Hallway strip | Solid Warm White (2000K) | 5% | Wayfinding |
| Bedroom | Off | 0% | Dark |
| Office | Independent (not part of scene) | 100% | Keeps doing its thing |

The living room's ceiling strip and Hue bulbs all react to the TV content -- the WLED strip behind the TV uses screen capture, and the room's ambient lighting derives from an area-averaged version of the TV image. When there's an explosion on screen, the entire room flashes warm orange.

**Cross-room effect: "Party Mode"**

All rooms join a continuous `ContinuousEffect` group. A rainbow wave sweeps through the house: Living Room -> Kitchen -> Hallway -> Office -> Bedroom. The compositor creates a virtual super-canvas mapping the floor plan and renders one continuous effect across it. Each room extracts its slice and pushes to local devices.

### 9.3 Sam's Studio: Precision Audio-Reactive

**Setup:**
- 4x vertical WLED panels behind monitors (each 60x4 matrix, mounted vertically)
- 1x ceiling ring WLED (120 LEDs in a circle, 80cm diameter)
- 1x desk underglow WLED (100 LEDs)

**Spatial requirement: "Radiate from center"**

Sam needs effects that expand outward from the center of the desk. Audio bass hits should create rings of color that grow from the center.

```
Spatial layout (top-down):

       Panel 1     Panel 2     Panel 3     Panel 4
         ▐           ▐           ▐           ▐

                  ╔═══════════╗
                  ║  ceiling  ║
                  ║   ring    ║
                  ╚═══════════╝

              ═══════ desk underglow ═══════

Canvas mapping -- radial projection:

  ┌────────────────────────────────────────────┐
  │                                            │
  │        ○    ○     ○     ○     ○            │
  │      P1   P2    Ring   P3   P4             │
  │                                            │
  │               ●                            │
  │            (center)                        │
  │                                            │
  │        ═══════════════════                 │
  │              desk                          │
  │                                            │
  └────────────────────────────────────────────┘
```

For radial effects, Sam switches the room's projection from the default top-down to a **radial projection**:

```rust
Projection::Radial {
    center: (0.5, 0.5),  // center of canvas = center of desk
    // Distance from center → canvas Y position
    // Angle from center → canvas X position
}
```

With radial projection, an audio-reactive effect that renders concentric circles on the canvas maps to physical concentric rings: desk underglow (closest), ceiling ring (mid), vertical panels (farthest). A bass hit creates an expanding ring of color from the desk outward to the room edges.

### 9.4 Event Setup: Concert Stage

**Setup:**
- 20x vertical WLED strips (1m each, 60 LEDs/m) arranged across stage backdrop
- 4x horizontal WLED strips (3m each, 60 LEDs/m) along stage front
- 2x WLED matrix panels (16x16) flanking the stage

**Requirements:**
- Precise timing across all 26 devices
- All devices on wired Ethernet (no WiFi for reliability)
- Show-control integration (triggered by music timecode or manual cues)
- Failover: if one strip disconnects, others continue

```
Stage layout:

  ┌──────────────────────────────────────────────────────────────┐
  │  BACKDROP                                                    │
  │  ▐ ▐ ▐ ▐ ▐ ▐ ▐ ▐ ▐ ▐ ▐ ▐ ▐ ▐ ▐ ▐ ▐ ▐ ▐ ▐                │
  │  1 2 3 4 5 6 7 8 9 ...                    20                │
  │                                                              │
  │ ┌────┐                                           ┌────┐     │
  │ │ M1 │              STAGE                        │ M2 │     │
  │ │16x16             AREA                         │16x16     │
  │ └────┘                                           └────┘     │
  │                                                              │
  │  ════════════ ════════════ ════════════ ════════════         │
  │  Front Strip 1  Front Strip 2  Front Strip 3  Front Strip 4 │
  │                                                              │
  │                        AUDIENCE                              │
  └──────────────────────────────────────────────────────────────┘
```

This is Installation scale. Key differences from Room scale:

- **Latency budget drops to < 10ms.** All WLED devices are on a dedicated VLAN with gigabit switches. DDP over wired Ethernet achieves < 1ms latency per device.
- **Per-room canvas becomes per-section.** The backdrop strips share one canvas (1280x200 stretched across 20 strips). The front strips share another. The matrices get their own.
- **Show control** integrates via OSC (Open Sound Control) or MIDI timecode. Hypercolor listens for cue triggers and switches effects/scenes on command. This is Phase 4+ territory.
- **Failover**: each strip is independently addressable. If strip 7 disconnects, the spatial sampler simply skips it. The effect continues across the remaining 19 strips with a visible gap, rather than everything stopping. The dashboard shows a red alert for the disconnected strip.

---

## 10. Future Vision

### 10.1 AR Overlay (Phone Camera LED Preview)

The killer demo feature: hold your phone camera up to your room and see a preview of the lighting effect rendered over the real-world LED positions in augmented reality.

```
Phone Camera View:

  ┌───────────────────────────┐
  │                           │
  │   ╔══════════╗            │
  │   ║ Monitor  ║            │
  │   ║          ║            │
  │   ╚══════════╝            │
  │                           │
  │   ===== desk =====       │  ← AR overlay: virtual LEDs
  │   ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓       │     glowing with the preview
  │                           │     effect colors, blended
  │                           │     into the camera feed
  │                           │
  │                           │
  │   [Effect: Aurora]  [▶]   │
  └───────────────────────────┘
```

**Technical approach:**
1. WebXR API (browser-based AR, no native app required)
2. User places AR anchors at known physical positions (match the room layout)
3. AR renderer draws glowing sprites at each LED's physical coordinate
4. Effect engine runs on the phone (WebGL) or streams from the daemon (WebSocket)
5. User can browse effects in AR mode, seeing the result before activating

**Prerequisite:** room layout must be defined. AR mode uses the physical coordinates from the room editor to place virtual LEDs.

### 10.2 LiDAR-Based Room Scanning

Modern phones (iPhone Pro, iPad Pro) and standalone sensors (Intel RealSense) have LiDAR. This enables automatic room layout capture.

```
LiDAR Scan Workflow:

1. User walks around room with LiDAR-equipped phone
2. Point cloud → mesh → room geometry
3. Hypercolor imports the room mesh:
   - Extract walls, floor, ceiling
   - Detect furniture (desk, shelf, TV) via ML object detection
   - Generate room dimensions and furniture placement
4. User only needs to place LED devices on the auto-detected furniture

Result: room setup in 30 seconds instead of 5 minutes
```

This builds on Apple's RoomPlan API (iOS 16+) or open-source alternatives like OpenCV + depth camera processing. The room mesh is exported as USDZ or glTF and imported into Hypercolor's 3D view.

### 10.3 Digital Twin

A persistent, accurate 3D model of the user's room with real-time lighting simulation.

```
Digital Twin Features:

- Photorealistic room model (imported from LiDAR scan or manual modeling)
- Accurate light propagation simulation (ray tracing or radiosity)
- Real-time effect preview on the digital twin
- "What will this look like?" without touching real hardware
- Share your digital twin with others (show off your setup)
- Screenshot / video export of the simulated room
```

The digital twin is a WebGL scene (Three.js) with:
- Room geometry (walls, floor, ceiling as reflective surfaces)
- Furniture as textured meshes
- LED strips as emissive line geometries
- Point lights (Hue) as emissive spheres with bloom post-processing
- Basic global illumination to simulate light bouncing off walls

This is a Phase 4+ feature that transforms Hypercolor from a device control tool into a spatial lighting design tool. Useful for planning installations before buying hardware.

### 10.4 Multi-User Collaborative Editing

For installations with multiple people (events, shared living spaces):

- Real-time collaborative room editor (CRDT-based, like Figma)
- User presence indicators (cursors, selections)
- Per-user permission levels (viewer, editor, admin)
- Conflict resolution for simultaneous edits
- Chat / annotation layer on the room layout

**Technical approach:** Yjs (CRDT library) for real-time collaboration, WebSocket for transport. Each user's edits are merged automatically without conflicts. The daemon acts as the authoritative state holder.

### 10.5 Adaptive Lighting Intelligence

Beyond manual mapping, the system learns and adapts:

- **Time-of-day adaptation**: automatically dim and warm-shift in the evening
- **Occupancy awareness**: Home Assistant presence detection triggers room scenes
- **Content-aware**: detect what's on screen (game, movie, work) and adapt lighting
- **Weather-responsive**: outdoor light sensor influences indoor color temperature
- **Circadian rhythm**: follow natural light patterns for health-conscious defaults

These integrations build on Home Assistant's sensor ecosystem and the scene/trigger system from the UX architecture. The spatial system provides the "where" -- adaptive intelligence provides the "when" and "why."

---

## 11. Data Model Summary

The complete spatial data hierarchy, from house down to LED:

```rust
pub struct HypercolorSpatial {
    /// Top level: the entire installation
    pub house: House,

    /// Global settings
    pub unit_system: UnitSystem,           // Metric or Imperial
    pub default_projection: Projection,

    /// Cross-room effect compositor
    pub compositor: Option<RoomCompositor>,
}

pub struct House {
    pub name: String,
    pub rooms: Vec<Room>,
    pub room_groups: Vec<RoomGroup>,
    pub floor_plan: Option<FloorPlan>,
}

pub struct Room {
    pub id: String,
    pub name: String,
    pub dimensions: RoomDimensions,
    pub furniture: Vec<FurnitureItem>,
    pub devices: Vec<DeviceMapping>,
    pub canvas: CanvasConfig,
    pub overrides: RoomOverrides,
    pub adjacency: Vec<RoomAdjacency>,
    pub photos: Vec<PhotoOverlay>,
}

pub struct DeviceMapping {
    pub device_id: String,             // references the physical device
    pub name: String,                  // user-friendly name
    pub topology: LedTopology,         // how LEDs are arranged
    pub role: LightingRole,            // ambient, accent, task, feature
    pub position: PhysicalCoord,       // position in room
    pub transform: TransformChain,     // physical → canvas mapping
    pub segment_mode: Option<WledSegmentMode>,
}

pub struct CanvasConfig {
    pub width: u32,                    // default: 320
    pub height: u32,                   // default: 200
    pub projection: Projection,        // how 3D room maps to 2D canvas
    pub viewport: CanvasViewport,      // which room region maps to canvas
}

pub struct PhotoOverlay {
    pub name: String,
    pub file_path: PathBuf,
    pub scale_px_per_cm: f64,
    pub view: Projection,
    pub opacity: f32,
}
```

### Serialization

The entire spatial configuration serializes to TOML (following Hypercolor's convention) for the config file and JSON for import/export/sharing. The TOML config lives at `~/.config/hypercolor/spatial.toml`.

```toml
[house]
name = "Bliss's Place"
unit_system = "metric"

[[house.rooms]]
id = "office"
name = "Office"
width = 500
height = 250
depth = 350

[[house.rooms.furniture]]
type = "desk"
preset = "180cm-lshape"
position = [250, 0, 175]
rotation = 0

[[house.rooms.devices]]
device_id = "wled-192.168.1.42-seg0"
name = "Desk Underglow"
role = "accent"

[house.rooms.devices.topology]
type = "path"
count = 120
density_per_meter = 60
waypoints = [[100, 0, 130], [400, 0, 130], [400, 0, 220], [100, 0, 220]]

[[house.rooms.devices]]
device_id = "hue-bridge1-light7"
name = "Floor Lamp"
role = "ambient"

[house.rooms.devices.topology]
type = "scatter"
positions = [[80, 120, 300]]

[house.rooms.devices.topology.ambient]
influence_radius = 200
```

---

## 12. Implementation Phases

### Phase 0 (Foundation): Basic Spatial Mapping
- **Already designed**: `SpatialLayout`, `DeviceZone`, `LedTopology`, `SpatialSampler` (see ARCHITECTURE.md)
- **Deliverable**: Flat canvas with drag-and-drop zone placement. Strip, Ring, Matrix topologies.
- **Scale**: Device/PC Case level only
- **Editor**: 2D canvas editor in web UI (existing plan)

### Phase 1 (Room): Physical Space Awareness
- Add `PhysicalCoord`, `RoomDimensions`, `TransformChain`
- Add `Path` topology with waypoints and density
- Add `LightingRole` (ambient, accent, task, feature) with role-aware sampling
- Add furniture system with built-in presets
- Room view editor with walls, trace mode
- Photo overlay mode (import, scale reference, place LEDs)
- **Scale**: Room level

### Phase 2 (House): Multi-Room
- Add `House`, `Room`, `RoomGroup` hierarchy
- Floor plan editor
- `RoomCompositor` for cross-room continuous effects
- Per-room overrides in scenes
- Latency compensation
- WLED segment/virtual device support
- Wall views
- **Scale**: House level

### Phase 3 (Polish): Advanced Editing
- 3D view (Three.js room visualization)
- Multi-photo layouts
- Export/share/import layouts
- Layout presets and community sharing
- Radial and custom projections

### Phase 4 (Vision): Future Features
- AR overlay (WebXR)
- LiDAR room scanning
- Digital twin with light simulation
- Computer vision LED auto-detection
- Multi-user collaborative editing
- Adaptive lighting intelligence
- Installation/show-control scale

---

## Appendix A: Coordinate System Quick Reference

```
┌──────────────────────────────────────────────────────────┐
│                                                          │
│  PHYSICAL              ROOM                CANVAS        │
│  (centimeters)         (normalized 0-1)    (pixels)      │
│                                                          │
│  x: left → right       x: 0.0 → 1.0       x: 0 → 319   │
│  y: floor → ceiling    y: 0.0 → 1.0       y: 0 → 199   │
│  z: front → back       z: 0.0 → 1.0       (2D only)    │
│                                                          │
│  Origin: front-left    Origin: same        Origin: top-  │
│          floor corner                       left pixel   │
│                                                          │
│  Units: cm             Units: fraction     Units: px     │
│                                                          │
│  Example:              Example:            Example:      │
│  (250, 120, 175)       (0.5, 0.48, 0.5)   (160, 96)    │
│  = center of room,     = center of room,   = center of  │
│    120cm height          48% height          canvas      │
│                                                          │
└──────────────────────────────────────────────────────────┘

Transformations:
  Physical → Room:     divide by room dimensions
  Room → 2D:           project (default: top-down, drop Y)
  2D Room → Canvas:    multiply by canvas dimensions
  Canvas → Effect:     bilinear sample pixel buffer
```

## Appendix B: Comparison with Existing Systems

| Feature | Hypercolor | SignalRGB | Artemis | xLights | OpenRGB |
|---------|-----------|-----------|---------|---------|---------|
| Room-level mapping | Yes (Phase 1) | No | Partial (surface editor) | Yes (3D models) | No |
| Multi-room | Yes (Phase 2) | No | No | Yes (controllers) | No |
| Photo overlay | Yes | No | No | Yes (custom model backgrounds) | No |
| Physical coordinates | Yes (cm) | No (canvas-only) | Sort of (pixel coords) | Yes (meters) | No |
| 3D visualization | Yes (Phase 3) | No | No | Yes (core feature) | No |
| Mixed device types | Yes (WLED + Hue + HID + OpenRGB) | Yes (proprietary) | Yes (via OpenRGB) | Yes (controllers) | Yes (USB/SMBus) |
| Ambient/accent roles | Yes | No | Layer-based | No | No |
| WLED segments | Yes (virtual devices) | Limited | No | E1.31 only | Partial |
| Cross-room effects | Yes (compositor) | No | No | Yes (universes) | No |
| AR preview | Phase 4 | No | No | No | No |

xLights is the closest precedent for installation-scale mapping. Hypercolor differentiates by focusing on the home/room scale with a dramatically simpler UX, real-time effects (vs. pre-programmed sequences), and native integration with the PC RGB ecosystem (OpenRGB, HID controllers).
