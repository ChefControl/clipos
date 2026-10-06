//! Killfeed rows, and the corner of the frame they are read in.

/// The killfeed corner of a `width` x `height` frame as `(x, y, w, h)`: the top half,
/// 0.75 x height wide from the right edge. Frames wider than 2:1 are already crops of
/// the killfeed and are used whole. Rows are read, followed and judged (whose they are)
/// in this cut, at the frame's own resolution.
pub fn crop_box(width: u32, height: u32) -> (u32, u32, u32, u32) {
    if width > 2 * height {
        return (0, 0, width, height);
    }
    let crop_w = width.min((0.75 * f64::from(height)).round() as u32);
    (
        width - crop_w,
        0,
        crop_w,
        (0.5 * f64::from(height)).round() as u32,
    )
}

/// One killfeed row, in pixels of the corner image it was found in.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Row {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    /// Model confidence, 0..1.
    pub score: f32,
}

impl Row {
    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }

    pub(crate) fn iou(&self, other: &Row) -> f32 {
        let w = (self.x1.min(other.x1) - self.x0.max(other.x0)).max(0.0);
        let h = (self.y1.min(other.y1) - self.y0.max(other.y0)).max(0.0);
        let inter = w * h;
        inter / (self.width() * self.height() + other.width() * other.height() - inter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crops_the_killfeed_corner() {
        assert_eq!(crop_box(1920, 1080), (1110, 0, 810, 540));
        assert_eq!(crop_box(1280, 960), (560, 0, 720, 480));
        // Already a killfeed crop: used whole.
        assert_eq!(crop_box(1347, 500), (0, 0, 1347, 500));
        // Narrower than the usual corner: the whole width.
        assert_eq!(crop_box(300, 600), (0, 0, 300, 300));
    }

    #[test]
    fn measures_overlap() {
        let row = |x0, y0, x1, y1| Row {
            x0,
            y0,
            x1,
            y1,
            score: 1.0,
        };
        let a = row(0.0, 0.0, 10.0, 10.0);
        assert_eq!((a.width(), a.height()), (10.0, 10.0));
        assert_eq!(a.iou(&a), 1.0);
        // Half of each: 50 / (100 + 100 - 50).
        assert_eq!(a.iou(&row(5.0, 0.0, 15.0, 10.0)), 50.0 / 150.0);
        assert_eq!(a.iou(&row(20.0, 20.0, 30.0, 30.0)), 0.0);
    }
}
