use hypercolor_core::types::canvas::{Canvas, PublishedSurface, Rgba};
use super::pipeline_runtime::{CachedStaticSurface, StaticSurfaceKey};

fn static_hold_canvas(width: u32, height: u32, color: [u8; 3]) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    if color != [0, 0, 0] {
        canvas.fill(Rgba::new(color[0], color[1], color[2], 255));
    }
    canvas
}

pub(crate) fn static_surface(
    cache: &mut Option<CachedStaticSurface>,
    width: u32,
    height: u32,
    color: [u8; 3],
) -> PublishedSurface {
    let key = StaticSurfaceKey {
        width,
        height,
        color,
    };

    if let Some(cached) = cache.as_ref()
        && cached.key == key
    {
        return cached.surface.clone();
    }

    let surface =
        PublishedSurface::from_owned_canvas(static_hold_canvas(width, height, color), 0, 0);
    *cache = Some(CachedStaticSurface {
        key,
        surface: surface.clone(),
    });
    surface
}
