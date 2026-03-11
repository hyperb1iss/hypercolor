# 13. Performance Architecture & Monitoring

> Hypercolor runs 24/7 at 60fps alongside games. Performance is not optional — it is the product.

---

## Design Principles

1. **Never steal frames from the game.** Hypercolor exists to enhance the gaming experience, not degrade it. If the system is under load, Hypercolor yields.
2. **Measure everything, assume nothing.** Every stage of the pipeline is instrumented. Performance regressions are caught in CI before they reach users.
3. **Graceful degradation over graceful failure.** Dropping to 30fps is acceptable. Dropping frames silently is not. Crashing is never acceptable.
4. **Predictable latency beats peak throughput.** A consistent 8ms frame is better than alternating 2ms and 14ms frames. Jitter causes visible flicker.

---

## 1. Frame Budget

### The 16.6ms Contract

At 60fps, each frame has exactly 16,666 microseconds. Here is how that budget is allocated across the render pipeline stages:

```
┌─────────────────────────────────────────────────────────────────────┐
│                     16.6ms Frame Budget (60fps)                     │
├─────────────┬───────────┬───────────┬───────────┬──────┬───────────┤
│   Input     │  Effect   │  Spatial  │  Device   │ Bus  │  Slack    │
│  Sampling   │ Rendering │ Sampling  │  Output   │      │           │
│   1.0ms     │   8.0ms   │   0.5ms   │  2.0ms    │0.1ms │  5.0ms   │
└─────────────┴───────────┴───────────┴───────────┴──────┴───────────┘
```

### Stage Budgets

| Stage | Target | Hard Limit | Notes |
|---|---|---|---|
| **Input sampling** | 1.0ms | 2.0ms | Audio FFT read, screen capture read, keyboard poll |
| **Effect rendering** | 8.0ms | 12.0ms | wgpu: <1ms typical. Servo: 5-10ms. This is the variable stage |
| **Spatial sampling** | 0.5ms | 1.0ms | Bilinear interpolation over ~2000 LEDs on a 256KB buffer |
| **Device output** | 2.0ms | 4.0ms | Async dispatch to all backends (USB, UDP, TCP) |
| **Event bus publish** | 0.1ms | 0.5ms | `watch::Sender::send_replace` — single atomic swap |
| **Slack/headroom** | 5.0ms | — | Absorbs variance, GC pauses (Servo/SpiderMonkey), OS scheduling |

### Why 5ms of Slack?

That slack is not waste — it is survival margin. Real-world variance sources:

- **SpiderMonkey GC pauses**: 1-5ms minor GC, up to 10ms incremental slice (capped via `SetGCSliceTimeBudget`)
- **USB HID write latency**: Normally <1ms, but USB host controller scheduling can spike to 3-5ms under load
- **OS thread scheduling**: Linux CFS can delay wake-up by 1-4ms if cores are saturated by a game
- **Vulkan command submission**: wgpu queue submission is async, but fence waits can stall
- **PipeWire buffer delivery**: Screen capture frames arrive on PipeWire's schedule, not ours

If the slack is consumed and the frame exceeds 16.6ms, the frame is still dispatched — but the next frame's sleep is shortened or skipped entirely. Two consecutive missed frames trigger the adaptive performance system (Section 6).

### Frame Timing Strategy

```rust
pub struct FrameTimer {
    target_interval: Duration,        // 16.6ms at 60fps
    stage_timings: [Duration; 5],     // Per-stage measurement
    frame_start: Instant,
    consecutive_misses: u32,          // Frames over budget
    ewma_frame_time: f64,            // Exponentially weighted moving average
}

impl FrameTimer {
    /// Returns how long to sleep before next frame.
    /// Returns Duration::ZERO if we're already behind.
    pub fn frame_complete(&mut self) -> Duration {
        let elapsed = self.frame_start.elapsed();
        self.ewma_frame_time = 0.95 * self.ewma_frame_time + 0.05 * elapsed.as_secs_f64();

        if elapsed > self.target_interval {
            self.consecutive_misses += 1;
            Duration::ZERO
        } else {
            self.consecutive_misses = 0;
            self.target_interval - elapsed
        }
    }
}
```

The EWMA (exponentially weighted moving average) provides a smoothed frame time for the adaptive performance system to make decisions on. Raw frame times are too noisy — a single USB stall should not trigger FPS reduction.

---

## 2. Thread Architecture

### Thread Map

Hypercolor uses a hybrid threading model: a tokio async runtime for I/O-bound work (network, IPC, web server) and dedicated OS threads for latency-sensitive workloads (audio, render loop).

```
┌──────────────────────────────────────────────────────────────────┐
│                        Process: hypercolord                       │
│                                                                   │
│  Thread 0: Main / Render Loop ─────────────────────── pinned     │
│    - Frame timing                                                 │
│    - Effect dispatch (wgpu submit or Servo pump)                  │
│    - Spatial sampling                                             │
│    - Device output dispatch (async, non-blocking)                 │
│                                                                   │
│  Thread 1: Audio Capture ──────────────────── SCHED_FIFO (RT)    │
│    - cpal callback thread (system-managed)                        │
│    - Ring buffer write (lock-free)                                │
│    - FFT processing at audio callback rate                        │
│                                                                   │
│  Thread 2: Screen Capture ─────────────────────────── pinned     │
│    - PipeWire/X11 frame receiver                                  │
│    - Downsample to canvas resolution                              │
│    - Triple-buffered output                                       │
│                                                                   │
│  Thread 3: Servo Main ───────────────────────────── (if active)  │
│    - SpiderMonkey JS execution (single-threaded)                  │
│    - DOM layout + style                                           │
│    - Canvas 2D / WebGL rendering                                  │
│    - Compositing → pixel readback                                 │
│                                                                   │
│  Threads 4..N: Servo Workers ────────────────────── (if active)  │
│    - Servo's internal thread pool (style, layout, networking)     │
│    - Typically 2-4 threads, mostly idle for our workload          │
│                                                                   │
│  Tokio Runtime (multi-thread, 2-4 worker threads):                │
│    - Axum web server + WebSocket streaming                        │
│    - Device backend I/O (TCP for OpenRGB, UDP for WLED/DDP)       │
│    - D-Bus service (zbus)                                         │
│    - Unix socket IPC (TUI/CLI connections)                        │
│    - mDNS discovery                                               │
│    - Configuration file watching                                  │
│                                                                   │
│  Thread: wgpu Device Poll ───────────────────────── (if active)  │
│    - GPU fence polling and callback dispatch                      │
│    - Managed by wgpu internally                                   │
│                                                                   │
└──────────────────────────────────────────────────────────────────┘
```

### Core Requirements

| Configuration | Active Threads | Minimum Cores | Recommended Cores |
|---|---|---|---|
| **wgpu only, no audio** | 4-5 (render, tokio×2, wgpu poll, web) | 2 | 4 |
| **wgpu + audio** | 5-6 | 2 | 4 |
| **wgpu + audio + screen** | 6-7 | 4 | 4 |
| **Servo + audio + screen** | 8-12 | 4 | 6 |
| **Full stack during gaming** | 8-12 | 4 | 6+ |

On a modern 8-core/16-thread gaming CPU (like the i7-14700K in the reference system), Hypercolor needs at most 2-3 performance cores even in the heaviest configuration. The render loop and audio thread should be pinned to efficiency cores when available, leaving performance cores free for the game.

### Thread Priority Strategy

```rust
use thread_priority::{ThreadPriority, set_current_thread_priority};

// Audio capture: real-time priority (SCHED_FIFO on Linux)
// Prevents audio buffer underruns that cause audible clicks
// Requires CAP_SYS_NICE or rtkit-daemon
fn spawn_audio_thread() {
    std::thread::Builder::new()
        .name("hc-audio".into())
        .spawn(move || {
            // Request RT priority via rtkit (safe, no root required)
            if let Err(e) = set_realtime_priority_via_rtkit(5) {
                tracing::warn!("RT audio priority unavailable: {e}. Using SCHED_OTHER.");
            }
            audio_capture_loop();
        })
        .expect("failed to spawn audio thread");
}

// Render loop: normal priority, but pinned to a specific core
// Pinning prevents migration jitter (1-3ms on context switch to cold cache)
fn spawn_render_thread(core_id: usize) {
    std::thread::Builder::new()
        .name("hc-render".into())
        .spawn(move || {
            pin_to_core(core_id);
            render_loop();
        })
        .expect("failed to spawn render thread");
}
```

### Inter-Thread Communication

All hot-path communication uses lock-free primitives. No `Mutex` on the render path.

| Channel | Type | Direction | Semantics |
|---|---|---|---|
| Audio data | `triple_buffer::Output<AudioFrame>` | audio → render | Latest-value, lock-free, zero-copy |
| Screen data | `triple_buffer::Output<ScreenFrame>` | screen → render | Latest-value, lock-free |
| LED frame data | `tokio::sync::watch` | render → frontends | Latest-value, async |
| Device colors | `tokio::sync::mpsc` (bounded) | render → device tasks | Backpressure, per-backend |
| Events | `tokio::sync::broadcast` | any → subscribers | Fan-out, bounded |
| Metrics | `crossbeam::channel` (bounded) | any → metrics aggregator | Batched, non-blocking try_send |

**Why triple buffering for audio/screen?** The producer (audio callback, screen capture) writes to a back buffer while the consumer (render loop) reads from the front buffer. A third buffer ensures the producer is never blocked waiting for the consumer. This eliminates the primary source of priority inversion between the RT audio thread and the normal-priority render thread.

---

## 3. Memory Budget

### Target Memory Envelope

| State | Target | Hard Limit |
|---|---|---|
| **Idle** (daemon running, no effect active) | 30MB | 50MB |
| **wgpu effect active** | 50MB | 80MB |
| **Servo effect active** | 150MB | 300MB |
| **Servo + screen capture + audio** | 200MB | 350MB |

### Per-Component Memory Breakdown

#### Core Buffers

| Component | Size | Count | Total | Notes |
|---|---|---|---|---|
| Canvas buffer (320x200 RGBA) | 256 KB | 2 (double-buffered) | 512 KB | Render output |
| LED color buffer (2000 LEDs x RGB) | 6 KB | 2 | 12 KB | Output + staging |
| Audio FFT bins | 800 B | 3 (triple-buffered) | 2.4 KB | 200 bins x f32 |
| Audio sample ring buffer | 64 KB | 1 | 64 KB | 16384 samples x f32 |
| Screen capture frame | 8 MB | 3 (triple-buffered) | 24 MB | 1920x1080 RGBA downsample src |
| Screen capture downsampled | 256 KB | 2 | 512 KB | 320x200 for effect input |
| Event bus buffers | ~64 KB | — | 64 KB | broadcast(256) + watch channels |
| Spatial layout data | ~20 KB | 1 | 20 KB | 2000 LEDs with transforms |
| **Core subtotal** | | | **~25 MB** | |

#### wgpu Resources

| Component | Size | Notes |
|---|---|---|
| Device + queue state | ~5 MB | Vulkan/OpenGL driver overhead |
| Render pipeline (320x200) | ~1 MB | Compiled shaders, descriptor sets |
| Output texture (320x200 RGBA) | 256 KB | GPU-side render target |
| Staging buffer (MAP_READ) | 256 KB | CPU-readable pixel readback |
| Uniform buffers | ~4 KB | Time, resolution, audio uniforms |
| Shader cache | ~2 MB | Compiled SPIR-V / driver cache |
| **wgpu subtotal** | **~9 MB** | |

#### Servo Resources

| Component | Size | Notes |
|---|---|---|
| SpiderMonkey heap (default) | 32-64 MB | Configurable via `SetGCParameter(JSGC_MAX_BYTES)` |
| DOM + style system | 5-20 MB | Depends on effect complexity |
| Canvas 2D backing store | 256 KB | 320x200 — trivial |
| WebGL context (if used) | 5-15 MB | GPU state, texture cache |
| Network/resource loading | 2-5 MB | Servo's resource cache |
| Layout engine | 5-10 MB | Style trees, box trees |
| Font cache | 5-10 MB | Rasterized glyph atlas |
| **Servo subtotal** | **54-124 MB** | |

#### Optimization: SpiderMonkey GC Tuning

Most effects are simple `requestAnimationFrame` loops with minimal allocation. We can aggressively constrain the JS heap:

```rust
// Constrain SpiderMonkey for our tiny effects
fn configure_spidermonkey_gc() {
    // Max heap: 64MB (default is much higher for browser use)
    JS_SetGCParameter(cx, JSGC_MAX_BYTES, 64 * 1024 * 1024);

    // Slice time budget: 3ms (fits within our frame slack)
    JS_SetGCParameter(cx, JSGC_SLICE_TIME_BUDGET_MS, 3);

    // Incremental GC: enabled (avoid stop-the-world)
    JS_SetGCParameter(cx, JSGC_INCREMENTAL_GC_ENABLED, 1);

    // Compacting GC: disabled (not needed for small heap, costs time)
    JS_SetGCParameter(cx, JSGC_COMPACTING_ENABLED, 0);

    // Nursery size: 1MB (small, fast minor GCs)
    JS_SetGCParameter(cx, JSGC_MAX_NURSERY_BYTES, 1 * 1024 * 1024);
}
```

### Memory Monitoring

```rust
pub struct MemoryMonitor {
    /// Sampled every 5 seconds
    pub rss_bytes: u64,               // Resident set size (actual RAM)
    pub vss_bytes: u64,               // Virtual size (may be large, that's OK)
    pub canvas_pool_bytes: u64,       // Managed buffer pool
    pub wgpu_allocated_bytes: u64,    // GPU memory (via wgpu MemoryHints)
    pub servo_heap_bytes: Option<u64>,// SpiderMonkey heap (via GC stats)
}

impl MemoryMonitor {
    /// Alert if RSS exceeds soft limit. Kill Servo if RSS exceeds hard limit.
    pub fn check_limits(&self, state: &EngineState) -> MemoryAction {
        let hard_limit = if state.servo_active { 350_000_000 } else { 80_000_000 };
        let soft_limit = hard_limit * 3 / 4;

        if self.rss_bytes > hard_limit {
            MemoryAction::ForceServoCycle // Destroy and recreate Servo instance
        } else if self.rss_bytes > soft_limit {
            MemoryAction::RequestGC       // Trigger SpiderMonkey GC + drop caches
        } else {
            MemoryAction::None
        }
    }
}
```

---

## 4. GPU Resource Management

### The Sharing Problem

Hypercolor renders on the same GPU that games use. The key insight: **our workload is negligible, but we must not cause contention.**

A 320x200 render is approximately 64,000 pixels. A 4K game renders 8,294,400 pixels. Hypercolor's GPU work is 0.77% of a single 4K frame — effectively invisible to the GPU scheduler, *as long as we don't block on synchronization or starve the game of submission bandwidth.*

### wgpu Path: Zero-Contention Strategy

```rust
pub struct GpuResourcePolicy {
    /// Use low-priority Vulkan queue if available (VK_QUEUE_GLOBAL_PRIORITY_LOW)
    pub queue_priority: QueuePriority,

    /// Render at most once per vsync interval (never double-submit)
    pub max_submissions_per_frame: u32,

    /// Budget for GPU-side execution (monitored via timestamp queries)
    pub gpu_time_budget_us: u64,

    /// If GPU is under heavy load, fall back to CPU software render
    pub software_fallback: bool,
}
```

**Vulkan queue management:**
- Request a separate compute queue with `VK_QUEUE_GLOBAL_PRIORITY_LOW` for Hypercolor's work
- If the driver doesn't support priority queues (common on NVIDIA), use the same queue but submit only one command buffer per frame
- Never use `vkQueueWaitIdle` — always use timeline semaphores or polling fences
- Command buffer recording happens on the render thread; submission and fence polling on the wgpu device thread

**GPU memory allocation:**
- Total GPU memory for Hypercolor: <5MB (one 320x200 texture + staging buffer + uniform buffers)
- Use `wgpu::MemoryHints::MemoryBudget(budget)` to advertise our tiny footprint
- Never allocate GPU memory in the render loop — all resources created at pipeline setup

### Servo's GPU Path vs. Software Rendering

Servo can render via two paths:

| Path | Implementation | GPU Usage | CPU Usage | When to Use |
|---|---|---|---|---|
| **Software** | `SoftwareRenderingContext` (OSMesa) | None | Higher | Gaming active, GPU-constrained |
| **Hardware** | surfman + EGL/GLX | Shared OpenGL context | Lower | Idle/desktop, GPU available |

**Decision logic:**

```rust
pub fn select_servo_render_path(system: &SystemState) -> ServoRenderPath {
    if system.gpu_usage_percent > 70 {
        // Game is running — stay off the GPU entirely
        ServoRenderPath::Software
    } else if system.on_battery {
        // Laptop on battery — minimize GPU wake-ups
        ServoRenderPath::Software
    } else {
        // Desktop idle — GPU accelerated is faster and more efficient
        ServoRenderPath::Hardware
    }
}
```

The software path for Servo at 320x200 is cheap: Canvas 2D operations at this resolution are CPU-trivial (we measured similar workloads at <2ms on modern CPUs). WebGL effects may be slower in software, but most community HTML effects use Canvas 2D.

### GPU Contention Mitigation

| Technique | Implementation | Impact |
|---|---|---|
| **Low-priority queue** | Vulkan `VK_QUEUE_GLOBAL_PRIORITY_LOW` | GPU scheduler preempts us for game work |
| **Micro-submissions** | Single 320x200 dispatch per frame | Completes in <100us GPU time |
| **Async readback** | Map staging buffer from *previous* frame | Eliminates GPU pipeline stalls |
| **Shared-nothing** | Separate `VkDevice` from game (via wgpu) | No implicit synchronization |
| **Software fallback** | Servo software path, CPU compute shaders | Zero GPU usage when gaming |

### Async Readback Pipeline

The classic GPU performance trap is synchronous readback — calling `map()` on a buffer and blocking until the GPU finishes. We avoid this entirely:

```
Frame N:   Submit render → (no wait)
Frame N+1: Map buffer from Frame N → read pixels → submit render
Frame N+2: Map buffer from Frame N+1 → read pixels → submit render
```

This introduces exactly 1 frame of latency (16.6ms) between rendering and LED output. At 60fps this is imperceptible — the LEDs are responding to the effect state from 16ms ago. Human perception of LED color change latency is approximately 50-100ms, so we have generous margin.

```rust
pub struct AsyncReadback {
    buffers: [wgpu::Buffer; 2],  // Double-buffered staging
    current: usize,               // Which buffer was just submitted
    pending_map: Option<wgpu::BufferSlice<'_>>,
}

impl AsyncReadback {
    /// Called after render submission. Reads the PREVIOUS frame's result.
    pub fn read_and_submit(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        output_texture: &wgpu::Texture,
    ) -> Option<&[u8]> {
        // 1. Read previous frame (already mapped)
        let pixels = self.buffers[1 - self.current]
            .slice(..)
            .get_mapped_range();

        // 2. Copy current frame to staging buffer
        encoder.copy_texture_to_buffer(
            output_texture.as_image_copy(),
            wgpu::ImageCopyBuffer {
                buffer: &self.buffers[self.current],
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(320 * 4),
                    rows_per_image: Some(200),
                },
            },
            wgpu::Extent3d { width: 320, height: 200, depth_or_array_layers: 1 },
        );

        // 3. Swap and initiate async map for next frame
        self.current = 1 - self.current;
        self.buffers[1 - self.current].slice(..).map_async(
            wgpu::MapMode::Read,
            |_| {},
        );

        Some(&pixels)
    }
}
```

---

## 5. Device Output Optimization

### Per-Protocol Analysis

#### USB HID (PrismRGB / Nollie)

**Constraint:** 65-byte packets over USB Interrupt transfers. USB 2.0 Full Speed allows one interrupt transfer per 1ms polling interval.

| Device | LEDs | Bytes/Frame | Packets/Frame | Min Transfer Time |
|---|---|---|---|---|
| Prism 8 (8ch x 126) | 1008 | 3024 (GRB) | 48 + 1 latch = 49 | 49ms at 1ms polls |
| Prism S (ATX+GPU) | 282 | 846 (RGB) | 14 | 14ms at 1ms polls |
| Prism Mini | 128 | 384 (RGB) | 7 | 7ms at 1ms polls |

**Problem:** A fully-loaded Prism 8 needs 49ms of USB transfer time per frame — that exceeds the 16.6ms frame budget at 60fps. This is a hardware limitation, not a software one.

**Solutions:**
1. **Reduce to 33fps for USB HID** — other engines default Prism 8 to 33fps (30ms budget, still tight for full 8-channel)
2. **Async fire-and-forget** — The render loop dispatches USB writes to a dedicated I/O thread and moves on. The USB thread sends as fast as the bus allows. If a new frame arrives before the previous one finished transmitting, the old frame is dropped.
3. **Partial updates** — If only channels 0-3 changed significantly, skip channels 4-7 this frame
4. **USB 2.0 High Speed** — If the device supports it (HID over High Speed allows 125us micro-frames, 8x faster)

```rust
pub struct UsbOutputQueue {
    /// Latest frame to send. Overwrites previous if USB is still busy.
    latest: Arc<AtomicCell<Option<DeviceFrame>>>,
    /// USB writer thread pulls from `latest` and sends packets
    writer_handle: JoinHandle<()>,
}

impl UsbOutputQueue {
    pub fn push_frame(&self, frame: DeviceFrame) {
        // Atomic swap — if USB thread hasn't consumed the last frame, it's dropped
        self.latest.store(Some(frame));
    }
}
```

#### WLED / DDP (UDP)

**Constraint:** None, effectively. UDP is fire-and-forget. DDP supports 480 pixels per packet (1442 bytes).

| Strip Length | Packets/Frame | Bytes/Frame | Network Impact |
|---|---|---|---|
| 300 LEDs | 1 | ~902 bytes | Negligible |
| 600 LEDs | 2 | ~1804 bytes | Negligible |
| 1200 LEDs | 3 | ~3606 bytes | Negligible |
| 5000 LEDs (large install) | 11 | ~15010 bytes | Still negligible |

At 60fps with 5000 LEDs: 15KB x 60 = 900KB/s = 7.2 Mbps. Well within gigabit Ethernet or even WiFi capacity.

**Optimization: Batched sendmsg**

```rust
use std::os::unix::io::AsRawFd;

/// Send multiple DDP packets in a single syscall via sendmmsg(2)
pub fn send_ddp_batch(socket: &UdpSocket, packets: &[DdpPacket]) -> io::Result<()> {
    // Linux sendmmsg sends multiple datagrams in one syscall
    // Reduces syscall overhead from N to 1 for N packets
    let msgs: Vec<mmsghdr> = packets.iter().map(|p| p.to_mmsghdr()).collect();
    unsafe {
        libc::sendmmsg(socket.as_raw_fd(), msgs.as_mut_ptr(), msgs.len() as u32, 0);
    }
    Ok(())
}
```

#### E1.31 / sACN (UDP)

**Constraint:** 170 RGB pixels per universe (512 DMX channels). Multiple universes for longer strips.

| Strip Length | Universes | Packets/Frame | Notes |
|---|---|---|---|
| 170 LEDs | 1 | 1 | Single universe |
| 300 LEDs | 2 | 2 | Typical WLED strip |
| 600 LEDs | 4 | 4 | Multi-segment |

E1.31 is less efficient than DDP (170 vs 480 pixels per packet) but more widely supported. Use DDP when the device supports it; fall back to E1.31 for legacy devices.

#### OpenRGB SDK (TCP)

**Constraint:** TCP adds connection overhead and head-of-line blocking. The OpenRGB protocol sends per-controller color updates.

**Optimization:**
- Persistent TCP connection (no reconnect per frame)
- Batch all controller updates into a single TCP write using Nagle-disabled socket (`TCP_NODELAY`)
- If OpenRGB server is on localhost: Unix socket if supported, otherwise TCP loopback is <0.1ms RTT

#### Philips Hue Entertainment (DTLS/UDP)

**Constraint:** Hue Entertainment API supports max 25fps for groups of up to 20 lights. DTLS handshake is expensive (500ms+) but only happens once per session.

**Optimization:**
- Maintain DTLS session across effect changes (only reconnect on error)
- Rate-limit Hue output to 25fps independently of the main render loop
- Pre-encode XY color space conversion (Hue uses CIE 1931, not sRGB)

### Output Pipeline Architecture

```
                    Render Loop
                        │
                        │ DeviceColors per zone
                        ▼
                ┌───────────────┐
                │ Output Router │
                └──┬──┬──┬──┬──┘
                   │  │  │  │
         ┌─────┐  │  │  │  └──────────────┐
         │     │  │  │  └────────┐         │
         ▼     ▼  ▼  ▼          ▼         ▼
      ┌─────┐┌────┐┌──────┐┌───────┐┌─────────┐
      │ HID ││ DDP││OpenRGB││ E1.31 ││Hue DTLS │
      │Queue││Send││ Batch ││ Batch ││  Queue  │
      └──┬──┘└──┬─┘└──┬───┘└──┬────┘└────┬────┘
         │      │     │       │          │
         ▼      ▼     ▼       ▼          ▼
       USB   UDP/IP  TCP    UDP/IP    DTLS/UDP
      async  sendmmsg batch  sendmmsg  rate-limited
```

Each backend gets its own output queue:
- **USB HID**: Dedicated thread with atomic latest-frame swap (see above)
- **UDP protocols** (DDP, E1.31): Tokio tasks, `sendmmsg` batching
- **TCP protocols** (OpenRGB): Tokio task, buffered writes with `TCP_NODELAY`
- **Hue**: Tokio task, rate-limited to 25fps, DTLS session management

The render loop never blocks on output. It dispatches color data to queues and moves to the next frame immediately.

---

## 6. Adaptive Performance

### FPS Tiers

| Tier | FPS | Frame Budget | When |
|---|---|---|---|
| **Full** | 60 | 16.6ms | Desktop idle, light applications |
| **Gaming** | 30 | 33.3ms | Game detected, moderate GPU/CPU usage |
| **Economy** | 15 | 66.6ms | Heavy system load, laptop on battery |
| **Standby** | 5 | 200ms | Screen off, system idle, slow breathing effect |
| **Suspended** | 0 | — | System sleep, daemon backgrounded, no active devices |

### Detection Signals

```rust
pub struct SystemLoadDetector {
    gpu_usage: GpuMonitor,         // Via /sys/class/drm/card0/device/gpu_busy_percent (AMD)
                                    // or nvidia-smi / NVML for NVIDIA
    cpu_usage: CpuMonitor,         // /proc/stat sampling
    fullscreen_app: bool,          // X11 _NET_WM_STATE_FULLSCREEN / Wayland protocol
    gamemode_active: bool,         // Feral GameMode D-Bus signal
    on_battery: bool,              // UPower D-Bus
    screen_blanked: bool,          // D-Bus org.freedesktop.ScreenSaver
    active_devices: usize,         // How many devices are actually connected
}

impl SystemLoadDetector {
    pub fn recommended_tier(&self) -> FpsTier {
        if self.screen_blanked || self.active_devices == 0 {
            return FpsTier::Standby;
        }
        if self.on_battery {
            return FpsTier::Economy;
        }
        if self.gamemode_active || self.fullscreen_app {
            return FpsTier::Gaming;
        }
        if self.gpu_usage.percent > 80 || self.cpu_usage.percent > 85 {
            return FpsTier::Gaming;
        }
        if self.gpu_usage.percent > 50 || self.cpu_usage.percent > 60 {
            // Check if we're sustaining 60fps — if so, stay at Full
            // This avoids unnecessary downshifts during brief load spikes
            return FpsTier::Full;
        }
        FpsTier::Full
    }
}
```

### Feral GameMode Integration

[GameMode](https://github.com/FeralInteractive/gamemode) is the standard mechanism for games to signal "I'm running, give me resources." Hypercolor subscribes to GameMode's D-Bus signals:

```rust
// org.freedesktop.DBus signal: NameOwnerChanged for com.feralinteractive.GameMode
// Or direct: GameMode's client library signals RegisterGameActive / UnregisterGameActive

pub async fn watch_gamemode(bus: &HypercolorBus) {
    let connection = zbus::Connection::session().await.unwrap();
    let proxy = GameModeProxy::new(&connection).await.unwrap();

    // When GameMode activates, drop to 30fps
    let mut signal = proxy.receive_game_registered().await.unwrap();
    while let Some(_) = signal.next().await {
        bus.events.send(HypercolorEvent::PerformanceTierChanged(FpsTier::Gaming)).ok();
    }
}
```

### Tier Transition Logic

Transitions are **asymmetric**: fast to downshift (protect the game), slow to upshift (prevent oscillation).

```
Downshift:  2 consecutive frames over budget → immediate tier change
Upshift:    5 seconds sustained below threshold → gradual tier change

Full → Gaming:    200ms delay (GameMode signal is instant, GPU spike needs confirmation)
Gaming → Economy: 2 seconds sustained high load
Economy → Gaming: 5 seconds sustained lower load
Gaming → Full:    10 seconds after GameMode deactivates or GPU usage drops below 40%
```

### Effect Quality Scaling

When dropping to a lower FPS tier, the system can also reduce effect complexity:

| Quality Level | Changes | Impact |
|---|---|---|
| **High** (60fps) | Full resolution, all post-processing | Baseline |
| **Medium** (30fps) | Skip every other audio FFT frame | Minimal visual difference |
| **Low** (15fps) | Reduce canvas to 160x100, simpler spatial interpolation | Visible but acceptable |
| **Minimal** (5fps) | Solid color or last-frame hold | Breathing/static only |

For Servo effects, quality scaling is more limited — we can't tell the JavaScript to "render simpler." The primary lever is reducing the canvas resolution by adjusting the WebView viewport size. Most effects scale gracefully since they use normalized coordinates.

For wgpu effects, we can pass a quality uniform and let the shader author implement LOD:

```wgsl
@group(0) @binding(0) var<uniform> quality: f32; // 0.0 = minimal, 1.0 = full

fn main(@builtin(global_invocation_id) id: vec3<u32>) -> @location(0) vec4<f32> {
    // Effect can branch on quality level
    let iterations = u32(mix(4.0, 64.0, quality));
    // ...
}
```

### Skip Frame Strategy

When a frame exceeds the budget, the question is: which stage to skip?

**Priority order (never skip):**
1. Device output — must always send *something*, even if it's the previous frame's data
2. Event bus — trivially cheap, never worth skipping

**Skippable stages:**
1. **Input sampling** — Reuse previous audio/screen data. 1 stale frame is imperceptible.
2. **Effect rendering** — Reuse previous canvas buffer. This is the biggest time saver.
3. **Spatial sampling** — Skip only if LED positions haven't changed (they rarely do).

```rust
pub fn skip_strategy(frame_timer: &FrameTimer) -> SkipDecision {
    if frame_timer.consecutive_misses >= 3 {
        // We're consistently over budget — skip rendering entirely, reuse last canvas
        SkipDecision::ReuseCanvas
    } else if frame_timer.ewma_frame_time > frame_timer.target_interval.as_secs_f64() * 0.9 {
        // Close to budget — skip input re-sampling
        SkipDecision::ReuseInputs
    } else {
        SkipDecision::None
    }
}
```

---

## 7. Monitoring & Diagnostics

### Built-In Metrics

Every frame produces a `FrameMetrics` struct that feeds both the internal dashboard and external exporters:

```rust
#[derive(Clone, Debug)]
pub struct FrameMetrics {
    pub frame_number: u64,
    pub timestamp: Instant,
    pub total_time: Duration,

    // Per-stage timings
    pub input_sample_time: Duration,
    pub render_time: Duration,
    pub spatial_sample_time: Duration,
    pub device_output_time: Duration,
    pub bus_publish_time: Duration,

    // Render details
    pub active_renderer: RendererType,  // Wgpu | Servo
    pub gpu_time_us: Option<u64>,       // GPU timestamp query (wgpu only)
    pub canvas_pixels: u32,             // 320*200 = 64000 normally

    // System state
    pub fps_tier: FpsTier,
    pub rss_bytes: u64,
    pub cpu_usage_percent: f32,
    pub gpu_usage_percent: Option<f32>,

    // Device output details
    pub devices_active: u32,
    pub devices_errored: u32,
    pub packets_sent: u32,
    pub bytes_sent: u64,
}
```

### Performance Dashboard

The web UI includes a dedicated performance panel (accessible at `/dashboard/performance` or via the TUI's `perf` tab):

```
╔══════════════════════════════════════════════════════════════════╗
║  Hypercolor Performance Dashboard                                ║
╠══════════════════════════════════════════════════════════════════╣
║                                                                   ║
║  FPS: 60.0  │  Frame: 8.2ms avg  │  Tier: Full  │  RSS: 52MB   ║
║                                                                   ║
║  Stage Breakdown (last 60 frames)                                ║
║  ├─ Input      ▓░░░░░░░░░░░░░░░░░░░░  0.3ms                    ║
║  ├─ Render     ▓▓▓▓▓▓▓▓▓░░░░░░░░░░░░  4.1ms  (wgpu)           ║
║  ├─ Spatial    ▓░░░░░░░░░░░░░░░░░░░░░  0.2ms                    ║
║  ├─ Output     ▓▓░░░░░░░░░░░░░░░░░░░░  1.2ms                    ║
║  ├─ Bus        ░░░░░░░░░░░░░░░░░░░░░░  0.04ms                   ║
║  └─ Slack      ▓▓▓▓▓▓▓▓▓▓▓░░░░░░░░░░  5.1ms                    ║
║                                                                   ║
║  Frame Time Histogram (ms)                                       ║
║  2-4  ▓▓▓▓▓▓▓▓▓▓                        15%                     ║
║  4-6  ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓  45%                     ║
║  6-8  ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓          30%                     ║
║  8-10 ▓▓▓▓▓▓                              8%                     ║
║  10+  ▓▓                                   2%                     ║
║                                                                   ║
║  Device Output                                                    ║
║  ├─ Prism 8 (USB)    33fps  48pkt/f  OK                         ║
║  ├─ WLED Strip (DDP) 60fps   1pkt/f  OK                         ║
║  ├─ OpenRGB (TCP)    60fps   3ctrl   OK                          ║
║  └─ Hue Living (DTLS) 25fps  1pkt/f  OK                         ║
║                                                                   ║
║  Audio Pipeline                                                   ║
║  ├─ Buffer: 512 samples │ Latency: 10.7ms │ Underruns: 0        ║
║  └─ FFT: 0.15ms │ Beat detect: 0.03ms                           ║
║                                                                   ║
╚══════════════════════════════════════════════════════════════════╝
```

### Prometheus Metrics Export

An optional `/metrics` endpoint serves Prometheus-compatible metrics for long-term trending and alerting:

```rust
use metrics::{counter, gauge, histogram};

fn record_frame_metrics(m: &FrameMetrics) {
    // Gauges (current value)
    gauge!("hypercolor_fps").set(1.0 / m.total_time.as_secs_f64());
    gauge!("hypercolor_rss_bytes").set(m.rss_bytes as f64);
    gauge!("hypercolor_devices_active").set(m.devices_active as f64);
    gauge!("hypercolor_fps_tier").set(m.fps_tier as f64);

    // Histograms (distribution)
    histogram!("hypercolor_frame_time_seconds").record(m.total_time.as_secs_f64());
    histogram!("hypercolor_render_time_seconds").record(m.render_time.as_secs_f64());
    histogram!("hypercolor_input_time_seconds").record(m.input_sample_time.as_secs_f64());
    histogram!("hypercolor_output_time_seconds").record(m.device_output_time.as_secs_f64());

    // Counters (cumulative)
    counter!("hypercolor_frames_total").increment(1);
    counter!("hypercolor_packets_sent_total").increment(m.packets_sent as u64);
    counter!("hypercolor_bytes_sent_total").increment(m.bytes_sent);
    if m.total_time > Duration::from_micros(16_666) {
        counter!("hypercolor_frames_missed_total").increment(1);
    }
}
```

**Exported metrics (selection):**

| Metric | Type | Labels | Description |
|---|---|---|---|
| `hypercolor_frame_time_seconds` | Histogram | `renderer` | Total frame processing time |
| `hypercolor_render_time_seconds` | Histogram | `renderer` | Effect render stage time |
| `hypercolor_fps` | Gauge | — | Current frames per second |
| `hypercolor_fps_tier` | Gauge | — | Active performance tier (0-4) |
| `hypercolor_rss_bytes` | Gauge | — | Resident set size |
| `hypercolor_devices_active` | Gauge | — | Connected device count |
| `hypercolor_device_errors_total` | Counter | `backend`, `device` | Per-device error count |
| `hypercolor_packets_sent_total` | Counter | `backend` | Packets sent per backend |
| `hypercolor_frames_missed_total` | Counter | — | Frames exceeding budget |
| `hypercolor_audio_underruns_total` | Counter | — | Audio buffer underruns |
| `hypercolor_servo_gc_pause_seconds` | Histogram | `gc_type` | SpiderMonkey GC pause duration |

### tracing Integration

All performance-relevant code paths are instrumented with `tracing` spans:

```rust
use tracing::{instrument, info_span, Span};

#[instrument(skip_all, fields(frame = %frame_num))]
async fn render_frame(&mut self, frame_num: u64) {
    let _input_span = info_span!("input_sample").entered();
    let inputs = self.sample_inputs().await;
    drop(_input_span);

    let _render_span = info_span!("effect_render", renderer = %self.active_renderer).entered();
    let canvas = self.effect_engine.render(inputs).await;
    drop(_render_span);

    let _spatial_span = info_span!("spatial_sample", leds = %self.layout.total_leds()).entered();
    let colors = self.spatial_engine.sample(&canvas);
    drop(_spatial_span);

    let _output_span = info_span!("device_output", backends = %self.backends.len()).entered();
    self.dispatch_to_backends(&colors).await;
    drop(_output_span);
}
```

**Debugging tools:**
- `RUST_LOG=hypercolor=trace` for full span output
- `tokio-console` for async task introspection (runtime state, waker counts, poll times)
- `tracy` integration via `tracing-tracy` for frame-by-frame visual profiling during development
- `perf` / `flamegraph` compatibility via `tracing-flame` subscriber

### Per-Backend Latency Tracking

Each device backend reports its own latency metrics:

```rust
pub struct BackendMetrics {
    pub backend_name: String,
    pub device_name: String,

    /// Time from color data received to hardware acknowledgment (if applicable)
    pub output_latency: Duration,

    /// Packets sent this frame
    pub packets_this_frame: u32,

    /// Consecutive failures (triggers reconnect after threshold)
    pub consecutive_errors: u32,

    /// Last error message
    pub last_error: Option<String>,

    /// Device-specific stats
    pub extra: HashMap<String, f64>,  // e.g., "voltage" for Prism 8
}
```

---

## 8. Startup Performance

### Target: Daemon Ready in <2 Seconds

"Ready" means: first frame rendered, first device output sent. Not necessarily all devices discovered or Servo initialized.

### Startup Sequence (Parallel Where Possible)

```
T+0ms      │ Process start
           │
T+50ms     │ Config loaded (TOML parse)
           │ Logging initialized
           │
T+100ms    ├──┬──┬──┬── Parallel init ──────────────────────
           │  │  │  │
           │  │  │  └─ wgpu device creation + pipeline compile
           │  │  │     Target: 200-500ms (driver-dependent)
           │  │  │
           │  │  └──── Audio device open (cpal)
           │  │        Target: 50-100ms
           │  │
           │  └─────── Device discovery start (mDNS, USB enum, TCP probe)
           │           Target: 100-300ms for first device, 2-5s for full scan
           │
           └────────── Axum web server bind + start
                       Target: 20ms
           │
T+500ms    │ wgpu ready → load default effect shader
           │ First device likely discovered
           │
T+800ms    │ First frame rendered (wgpu path)
           │ First device output sent
           │ ✅ DAEMON READY (signal systemd, open IPC socket)
           │
T+1-5s     │ Background: remaining devices discovered
           │ Background: mDNS resolution completes
           │
T+?        │ Servo: loaded ONLY when an HTML effect is first selected
           │ (see Lazy Servo Loading below)
```

### Lazy Servo Loading

Servo initialization is expensive: 2-5 seconds for the first load (SpiderMonkey JIT warmup, font cache, resource loading). We never pay this cost at startup.

```rust
pub enum EffectRenderer {
    Wgpu(WgpuRenderer),                    // Always available
    Servo(Option<ServoRenderer>),           // Lazily initialized
}

impl EffectRenderer {
    pub async fn ensure_servo(&mut self) -> &mut ServoRenderer {
        if let EffectRenderer::Servo(inner) = self {
            if inner.is_none() {
                tracing::info!("Initializing Servo engine (first HTML effect load)...");
                let start = Instant::now();
                *inner = Some(ServoRenderer::new().await.expect("Servo init failed"));
                tracing::info!("Servo ready in {:?}", start.elapsed());
            }
            inner.as_mut().unwrap()
        } else {
            panic!("ensure_servo called on non-Servo renderer");
        }
    }
}
```

**UX during Servo warmup:** The daemon sends a "loading" event to the event bus. The web UI shows a subtle loading indicator. The TUI shows "Warming up..." with a spinner. Previous effect continues running until Servo is ready. The transition is seamless from the user's perspective.

### Device Discovery Parallelization

```rust
pub async fn discover_all_devices(registry: &DeviceRegistry) -> Vec<DeviceInfo> {
    // All discovery methods run concurrently
    let (usb_devices, mdns_devices, openrgb_devices) = tokio::join!(
        // USB enumeration is synchronous (hidapi), run on blocking thread
        tokio::task::spawn_blocking(|| discover_usb_hid_devices()),

        // mDNS is async, with 2-second timeout for initial results
        tokio::time::timeout(
            Duration::from_secs(2),
            discover_mdns_devices()  // WLED, Hue bridges
        ),

        // OpenRGB SDK probe is async TCP
        tokio::time::timeout(
            Duration::from_secs(1),
            discover_openrgb_controllers()
        ),
    );

    // Combine results, logging any timeouts (not errors — discovery continues in background)
    let mut devices = usb_devices.unwrap_or_default();
    devices.extend(mdns_devices.unwrap_or(Ok(vec![])).unwrap_or_default());
    devices.extend(openrgb_devices.unwrap_or(Ok(vec![])).unwrap_or_default());
    devices
}
```

Discovery that times out during startup continues in the background. Devices that come online later are detected and added dynamically (mDNS listener, USB hotplug via udev).

### Config Loading Optimization

Configuration files are small (typically <50KB of TOML) and loaded synchronously at startup. No optimization needed — `toml::from_str` parses 50KB in <1ms.

The spatial layout (LED positions, zone definitions) is part of the config and does not require any heavy computation at load time. Layout transforms (zone rotation, canvas coordinate mapping) are precomputed once and cached as lookup tables.

---

## 9. Long-Running Stability

### Memory Leak Prevention

A daemon running 24/7 at 60fps produces 5.2 million frames per day. Even a 1-byte leak per frame means 5MB/day — noticeable within a week. Strategies:

**Compile-time prevention:**
- No `Box::leak` outside of initialization code
- All buffers are pre-allocated and reused (double/triple buffering)
- `Arc<T>` cycles detected via `#[cfg(debug_assertions)]` weak-reference audits
- Canvas and LED buffers are stack-allocated or pool-allocated — never fresh `Vec` per frame

**Runtime detection:**
- RSS monitoring every 5 seconds (see Memory Monitor in Section 3)
- Trend detection: if RSS grows >1MB/hour with constant load, log a warning
- SpiderMonkey heap size monitoring via GC stats
- `jemalloc` as the global allocator (via `tikv-jemallocator`) for accurate `malloc_stats` and heap profiling

```rust
// Memory trend detector
pub struct MemoryTrendDetector {
    samples: VecDeque<(Instant, u64)>,  // (timestamp, rss_bytes)
    window: Duration,                    // 1 hour
}

impl MemoryTrendDetector {
    pub fn add_sample(&mut self, rss: u64) {
        let now = Instant::now();
        self.samples.push_back((now, rss));

        // Trim old samples
        while self.samples.front().map_or(false, |(t, _)| now - *t > self.window) {
            self.samples.pop_front();
        }
    }

    /// Returns bytes/second growth rate. Positive = leak suspected.
    pub fn growth_rate(&self) -> f64 {
        if self.samples.len() < 10 { return 0.0; }
        let (t0, rss0) = self.samples.front().unwrap();
        let (t1, rss1) = self.samples.back().unwrap();
        let dt = (*t1 - *t0).as_secs_f64();
        if dt < 60.0 { return 0.0; } // Need at least 1 minute of data
        (*rss1 as f64 - *rss0 as f64) / dt
    }
}
```

### File Descriptor Management

At 60fps with multiple device backends, file descriptor hygiene is critical:

| Resource | FDs Used | Lifecycle |
|---|---|---|
| USB HID devices | 1 per device | Held while connected |
| UDP sockets (DDP, E1.31) | 1 per network target | Held while connected |
| TCP connections (OpenRGB) | 1 per OpenRGB server | Reconnect on error |
| DTLS sessions (Hue) | 1 per Hue bridge | Reconnect on timeout |
| Unix socket (IPC) | 1 listener + 1 per client | Closed on disconnect |
| TCP listener (Axum) | 1 + 1 per HTTP connection | HTTP keep-alive timeout |
| PipeWire | 2-3 (screen capture) | Held while capturing |
| cpal audio | 1-2 (ALSA/PipeWire) | Held while running |
| inotify (config watch) | 1 | Held for daemon lifetime |

**Typical total:** 15-30 FDs. Well within the default Linux limit of 1024.

**Protection:** Set `RLIMIT_NOFILE` soft limit to 256 at startup. If we approach it, something is leaking connections. Log and investigate rather than silently bumping the limit.

### System Sleep/Resume Handling

```rust
// Subscribe to systemd-logind sleep/resume signals via D-Bus
pub async fn watch_sleep_signals(engine: &mut Engine) {
    let connection = zbus::Connection::system().await.unwrap();
    let logind = LogindManagerProxy::new(&connection).await.unwrap();

    let mut sleep_signal = logind.receive_prepare_for_sleep().await.unwrap();

    while let Some(signal) = sleep_signal.next().await {
        let is_suspending: bool = signal.args().unwrap().start;

        if is_suspending {
            // System going to sleep
            tracing::info!("System suspending — pausing all output");
            engine.suspend();
            // - Stop render loop (set fps_tier to Suspended)
            // - Close USB HID handles (they will be invalid after resume)
            // - Close DTLS sessions
            // - Keep TCP/UDP sockets (kernel handles reconnect)
        } else {
            // System waking up
            tracing::info!("System resuming — reinitializing devices");
            engine.resume().await;
            // - Re-enumerate USB devices (hotplug events may have been lost)
            // - Reconnect DTLS (Hue sessions expired during sleep)
            // - Restore previous FPS tier
            // - Resume render loop
        }
    }
}
```

### Resource Cleanup on Effect Switch

When switching effects, all resources from the previous effect must be released before the new effect allocates:

```rust
pub async fn switch_effect(&mut self, new_effect: EffectId) {
    // 1. Stop current render (hold last frame on devices)
    self.render_loop.pause();

    // 2. Clean up previous effect
    match &mut self.current_renderer {
        EffectRenderer::Wgpu(r) => {
            r.destroy_pipeline();  // Release GPU resources
            // Device and queue persist — pipeline is cheap to recreate
        }
        EffectRenderer::Servo(Some(r)) => {
            // Navigate to about:blank to release JS heap, DOM, canvas state
            r.navigate("about:blank");
            r.servo.spin_event_loop();
            // Optionally trigger aggressive GC
            r.force_gc();
        }
        _ => {}
    }

    // 3. Load new effect
    let renderer = self.load_effect(new_effect).await?;
    self.current_renderer = renderer;

    // 4. Resume rendering
    self.render_loop.resume();
}
```

### Watchdog

A watchdog thread monitors the render loop and restarts it if it hangs:

```rust
pub struct Watchdog {
    last_heartbeat: Arc<AtomicU64>,  // Epoch millis, updated every frame
    timeout: Duration,                // 5 seconds (300 missed frames at 60fps)
}

impl Watchdog {
    pub fn spawn(self, restart_tx: mpsc::Sender<()>) -> JoinHandle<()> {
        std::thread::Builder::new()
            .name("hc-watchdog".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                    let last = self.last_heartbeat.load(Ordering::Relaxed);
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64;

                    if now - last > self.timeout.as_millis() as u64 {
                        tracing::error!(
                            "Render loop hung for {:?} — requesting restart",
                            Duration::from_millis(now - last)
                        );
                        restart_tx.blocking_send(()).ok();
                    }
                }
            })
            .expect("failed to spawn watchdog thread")
    }
}
```

The watchdog does **not** kill the process. It sends a restart signal to the main loop, which attempts a graceful recovery:
1. Drop the current effect renderer
2. Re-initialize wgpu device (GPU may have been lost)
3. Re-enumerate USB devices
4. Resume with the default effect

If the restart signal itself is not acknowledged within 10 seconds, `sd_notify(WATCHDOG=trigger)` tells systemd to hard-restart the service.

### systemd Integration

```ini
# hypercolor.service
[Service]
Type=notify
WatchdogSec=30
Restart=on-failure
RestartSec=2
MemoryMax=512M
CPUQuota=25%
IOWeight=50
Nice=5

# Capability for RT audio thread (via rtkit)
AmbientCapabilities=CAP_SYS_NICE

# Hardening
ProtectSystem=strict
ProtectHome=read-only
PrivateTmp=yes
NoNewPrivileges=yes
```

`CPUQuota=25%` is a hard backstop: even if Hypercolor goes haywire, it cannot consume more than 25% of system CPU. `MemoryMax=512M` kills the process if memory exceeds half a gig — something is very wrong at that point.

---

## 10. Benchmarking Strategy

### Benchmark Categories

#### Category 1: Effect Rendering Throughput

Measure time per frame for each renderer at the canonical 320x200 resolution.

```rust
#[bench]
fn bench_wgpu_render_solid_color(b: &mut Bencher) {
    let mut renderer = WgpuRenderer::new_for_bench(320, 200);
    let shader = load_shader("solid_color.wgsl");
    renderer.load_pipeline(&shader);

    b.iter(|| {
        renderer.render_sync(&Uniforms::default())
    });
}

#[bench]
fn bench_wgpu_render_complex_shader(b: &mut Bencher) {
    // Noise-based shader with multiple octaves — worst case for compute
    let mut renderer = WgpuRenderer::new_for_bench(320, 200);
    let shader = load_shader("fractal_noise.wgsl");
    renderer.load_pipeline(&shader);

    b.iter(|| {
        renderer.render_sync(&Uniforms::default())
    });
}

#[bench]
fn bench_wgpu_pixel_readback(b: &mut Bencher) {
    // Measure readback latency independently of render
    let renderer = WgpuRenderer::new_for_bench(320, 200);
    b.iter(|| {
        renderer.readback_sync()
    });
}

#[bench]
fn bench_servo_render_canvas2d(b: &mut Bencher) {
    let mut renderer = ServoRenderer::new_for_bench(320, 200);
    renderer.navigate("file:///effects/builtin/Rainbow.html");
    renderer.warmup(60); // 60 frames to stabilize JIT

    b.iter(|| {
        renderer.render_sync()
    });
}
```

**Targets:**

| Benchmark | Target | Hard Limit |
|---|---|---|
| wgpu solid color render | <0.5ms | <1ms |
| wgpu complex shader render | <2ms | <5ms |
| wgpu pixel readback | <0.3ms | <1ms |
| Servo Canvas 2D render | <5ms | <10ms |
| Servo WebGL render | <8ms | <12ms |

#### Category 2: Spatial Sampling

```rust
#[bench]
fn bench_spatial_sample_500_leds(b: &mut Bencher) {
    let canvas = Canvas::random(320, 200);
    let layout = SpatialLayout::linear_strip(500);

    b.iter(|| {
        SpatialSampler::sample(&canvas, &layout)
    });
}

#[bench]
fn bench_spatial_sample_2000_leds(b: &mut Bencher) {
    let canvas = Canvas::random(320, 200);
    let layout = SpatialLayout::grid(50, 40); // 2000 LEDs in a grid

    b.iter(|| {
        SpatialSampler::sample(&canvas, &layout)
    });
}
```

**Targets:**

| LED Count | Target | Notes |
|---|---|---|
| 500 | <0.1ms | Typical setup |
| 2000 | <0.3ms | Large setup |
| 5000 | <0.8ms | Extreme setup |

#### Category 3: Device Output

```rust
#[bench]
fn bench_ddp_packet_encode_300_leds(b: &mut Bencher) {
    let colors: Vec<Rgb> = (0..300).map(|_| Rgb::new(128, 64, 255)).collect();

    b.iter(|| {
        DdpPacket::encode_all(&colors)
    });
}

#[bench]
fn bench_usb_hid_packet_encode_prism8(b: &mut Bencher) {
    let colors: Vec<Rgb> = (0..1008).map(|_| Rgb::new(128, 64, 255)).collect();

    b.iter(|| {
        Prism8Protocol::encode_frame(&colors)
    });
}
```

#### Category 4: Audio Pipeline

```rust
#[bench]
fn bench_fft_1024_samples(b: &mut Bencher) {
    let samples: Vec<f32> = (0..1024).map(|i| (i as f32 * 0.01).sin()).collect();

    b.iter(|| {
        FftProcessor::process(&samples)
    });
}

#[bench]
fn bench_beat_detection(b: &mut Bencher) {
    let spectrum = SpectrumData::synthetic_beat();

    b.iter(|| {
        BeatDetector::analyze(&spectrum)
    });
}
```

**Targets:**

| Benchmark | Target |
|---|---|
| FFT 1024 samples | <0.2ms |
| FFT 4096 samples | <0.5ms |
| Beat detection | <0.05ms |
| Full audio pipeline | <0.5ms |

#### Category 5: Memory Regression

```rust
#[test]
fn test_memory_stable_after_1000_frames() {
    let mut engine = Engine::new_for_test();
    engine.load_effect("Rainbow.html");

    // Warmup
    for _ in 0..100 {
        engine.render_frame_sync();
    }

    let rss_before = get_rss_bytes();

    // Run 1000 frames
    for _ in 0..1000 {
        engine.render_frame_sync();
    }

    let rss_after = get_rss_bytes();
    let growth = rss_after as i64 - rss_before as i64;

    // Allow 1MB tolerance for allocator fragmentation
    assert!(
        growth < 1_000_000,
        "Memory grew by {} bytes over 1000 frames — possible leak",
        growth
    );
}

#[test]
fn test_memory_stable_across_effect_switches() {
    let mut engine = Engine::new_for_test();
    let effects = ["Rainbow.html", "Solid Color.html", "Neon Shift.html"];

    // Warmup
    for effect in &effects {
        engine.load_effect(effect);
        for _ in 0..60 { engine.render_frame_sync(); }
    }

    let rss_before = get_rss_bytes();

    // Switch effects 100 times
    for i in 0..100 {
        engine.load_effect(effects[i % effects.len()]);
        for _ in 0..30 { engine.render_frame_sync(); }
    }

    let rss_after = get_rss_bytes();
    let growth = rss_after as i64 - rss_before as i64;

    assert!(
        growth < 5_000_000,
        "Memory grew by {} bytes over 100 effect switches — cleanup issue",
        growth
    );
}
```

### Performance CI Pipeline

Performance benchmarks run on every PR that touches the render pipeline, device backends, or spatial engine.

```yaml
# .github/workflows/perf.yml
name: Performance Gates
on:
  pull_request:
    paths:
      - 'crates/hypercolor-core/src/effect/**'
      - 'crates/hypercolor-core/src/spatial/**'
      - 'crates/hypercolor-core/src/device/**'
      - 'crates/hypercolor-core/src/input/**'

jobs:
  benchmarks:
    runs-on: ubuntu-latest  # Ideally self-hosted with GPU for wgpu benches
    steps:
      - uses: actions/checkout@v4

      - name: Run benchmarks
        run: cargo bench --bench render_benchmarks -- --output-format bencher | tee bench-output.txt

      - name: Compare against main
        uses: benchmark-action/github-action-benchmark@v1
        with:
          tool: 'cargo'
          output-file-path: bench-output.txt
          alert-threshold: '120%'        # Fail if 20% regression
          comment-on-alert: true
          fail-on-alert: true
          github-token: ${{ secrets.GITHUB_TOKEN }}

  memory-tests:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Memory regression tests
        run: cargo test --test memory_regression -- --nocapture
```

### Performance Reporting

The benchmark suite generates a report on every release:

```
╔══════════════════════════════════════════════════════════════╗
║  Hypercolor v0.3.0 Performance Report                        ║
╠══════════════════════════════════════════════════════════════╣
║                                                               ║
║  Render Pipeline (320x200)                                   ║
║  ├─ wgpu solid color:       0.31ms  (target: <0.5ms)   ✅   ║
║  ├─ wgpu fractal noise:     1.82ms  (target: <2.0ms)   ✅   ║
║  ├─ wgpu pixel readback:    0.18ms  (target: <0.3ms)   ✅   ║
║  ├─ Servo Canvas 2D:        4.7ms   (target: <5.0ms)   ✅   ║
║  └─ Servo WebGL:            7.1ms   (target: <8.0ms)   ✅   ║
║                                                               ║
║  Spatial Sampling                                            ║
║  ├─ 500 LEDs:               0.06ms                      ✅   ║
║  ├─ 2000 LEDs:              0.21ms                      ✅   ║
║  └─ 5000 LEDs:              0.54ms                      ✅   ║
║                                                               ║
║  Device Output                                               ║
║  ├─ DDP encode 300 LEDs:    0.008ms                     ✅   ║
║  ├─ DDP encode 5000 LEDs:   0.11ms                      ✅   ║
║  ├─ Prism 8 encode 1008:    0.04ms                      ✅   ║
║  └─ E1.31 encode 600 LEDs:  0.02ms                      ✅   ║
║                                                               ║
║  Audio Pipeline                                              ║
║  ├─ FFT 1024 samples:       0.14ms                      ✅   ║
║  ├─ FFT 4096 samples:       0.41ms                      ✅   ║
║  └─ Beat detection:         0.03ms                      ✅   ║
║                                                               ║
║  Memory                                                      ║
║  ├─ Idle RSS:               28MB    (target: <50MB)     ✅   ║
║  ├─ wgpu active RSS:        47MB    (target: <80MB)     ✅   ║
║  ├─ Servo active RSS:       138MB   (target: <300MB)    ✅   ║
║  ├─ 1000-frame stability:   +120KB  (limit: <1MB)      ✅   ║
║  └─ 100 effect switches:    +2.1MB  (limit: <5MB)      ✅   ║
║                                                               ║
║  Startup                                                     ║
║  ├─ Config load:            0.8ms                        ✅   ║
║  ├─ wgpu init:              340ms                        ✅   ║
║  ├─ First frame:            720ms                        ✅   ║
║  ├─ Daemon ready:           810ms   (target: <2000ms)   ✅   ║
║  └─ Servo cold start:       3200ms  (lazy, not startup) ℹ️   ║
║                                                               ║
╚══════════════════════════════════════════════════════════════╝
```

---

## Appendix A: Key Crate Dependencies for Performance

| Crate | Purpose | Why This One |
|---|---|---|
| `tikv-jemallocator` | Global allocator | malloc_stats, heap profiling, reduced fragmentation |
| `triple_buffer` | Lock-free producer-consumer | Zero-copy, wait-free, perfect for audio/screen threads |
| `metrics` + `metrics-exporter-prometheus` | Metrics framework | Prometheus-compatible, zero-cost disabled metrics |
| `tracing` + `tracing-subscriber` | Structured logging + spans | Per-frame span timing, async-aware |
| `tracing-tracy` | Tracy profiler integration | Visual frame profiling during development |
| `tokio-console` | Async runtime debugger | Task introspection, poll timing |
| `thread-priority` | OS thread priority control | RT priority for audio via rtkit |
| `core_affinity` | CPU core pinning | Reduce render thread migration jitter |
| `criterion` | Benchmarking framework | Statistical analysis, regression detection |
| `sysinfo` | System metrics (CPU, RAM, GPU) | Cross-platform load detection |

## Appendix B: Reference Hardware Performance Expectations

Based on the reference system (i7-14700K, RTX 4070 SUPER, 64GB DDR5):

| Scenario | Expected FPS | CPU Usage | GPU Usage | RAM |
|---|---|---|---|---|
| wgpu solid color, 1 WLED strip | 60 | <1% | <0.1% | 45MB |
| wgpu complex shader, 5 devices | 60 | 2-3% | <0.5% | 50MB |
| Servo Canvas 2D, 5 devices | 60 | 3-5% | <0.5% | 150MB |
| Servo WebGL, 10 devices, audio | 60 | 5-8% | <1% | 180MB |
| Above + screen capture | 60 | 8-12% | <1% | 200MB |
| Above + game running (GPU@90%) | 30 (Gaming tier) | 5-8% | <0.5% (software) | 200MB |
| System idle, breathing effect | 5 (Standby) | <0.5% | 0% | 40MB |
