use gpui::{Pixels, Rems, px, rems};
use serde::{Deserialize, Serialize};

/// A design dimension in pixels at 100% zoom, resolved through the window rem size.
/// Keep measured geometry and OS insets in Pixels instead.
pub const fn design(pixels: f32) -> Rems {
    rems(pixels / 16.)
}

/// Space kept between a fixed-size surface and the window edge.
const VIEWPORT_INSET: f32 = 32.;
/// Below this a clamped surface is unusable anyway, so stop shrinking and let
/// it overflow rather than collapse to nothing.
const VIEWPORT_FLOOR: f32 = 120.;

/// Cap a fixed design dimension at what the window can actually show.
///
/// Design sizes (a 680px dialog, a 390px recents viewport) assume a desktop
/// window. The same views now run in a phone-sized or browser viewport, where an
/// unclamped size overflows off-screen instead of scrolling.
pub fn fit_viewport(desired: Pixels, available: Pixels) -> Pixels {
    desired.min(px(
        (f32::from(available) - VIEWPORT_INSET).max(VIEWPORT_FLOOR)
    ))
}

/// A size for tcode UI elements.
#[derive(Clone, Default, Copy, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub enum Size {
    /// Explicit design pixels at 100%; resolved to rems by the widget.
    Size(Pixels),
    XSmall,
    Small,
    #[default]
    Medium,
    Large,
}

impl From<Pixels> for Size {
    fn from(size: Pixels) -> Self {
        Self::Size(size)
    }
}

/// A trait for setting the size of an element.
pub trait Sizable: Sized {
    fn with_size(self, size: impl Into<Size>) -> Self;

    #[inline(always)]
    fn xsmall(self) -> Self {
        self.with_size(Size::XSmall)
    }

    #[inline(always)]
    fn small(self) -> Self {
        self.with_size(Size::Small)
    }

    #[inline(always)]
    fn large(self) -> Self {
        self.with_size(Size::Large)
    }
}
