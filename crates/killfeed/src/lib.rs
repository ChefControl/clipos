//! CS2 killfeed analysis: finds the killfeed rows in a clip's frames and reads them.
//!
//! - [`crop_box`]: the part of a frame the killfeed lives in (top-right corner).
//! - [`RowFinder`]: a small RF-DETR model that boxes every killfeed row in that corner,
//!   on any resolution, HUD scale or background.
//! - [`owner`]: whose row it is (red outline: the player's kill; red fill: their death).
//! - [`IconReader`]: a second RF-DETR model that reads each row's weapon and modifiers.
//! - [`Tracker`]: follows rows across frames so each kill is counted once.
//! - [`Analyzer`]: all of the above for one clip; [`summarise`] turns its kills into the
//!   recording player's stats.
//!
//! The models are trained, evaluated and published from
//! ChefControl/clipos-killfeed-training; the worker downloads them from the `models`
//! container. Plain library, no ffmpeg or async: the worker decodes frames and feeds
//! them in.

mod analyse;
mod detector;
mod icon_reader;
mod reading;
mod rows;
mod summary;
mod track;

pub use analyse::{Analyzer, SAMPLE_FPS};
pub use detector::{Detection, Detector};
pub use icon_reader::{IconReader, sheet};
pub use reading::{KILL_MODIFIERS, MARKS, Owner, Reading, owner};
pub use rows::{Row, RowFinder, crop_box};
pub use summary::{Summary, summarise};
pub use track::{Kill, Tracker};
