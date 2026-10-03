//! Running an RF-DETR model exported to ONNX (trained in ChefControl/clipos-killfeed-training).

use std::path::Path;

use anyhow::{Context, anyhow};
use image::{RgbImage, imageops};
use ort::{session::Session, value::Tensor};

/// ImageNet normalisation, as in RF-DETR's training transforms.
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// One detection, with its box normalised to the input image (0..1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Detection {
    pub class: usize,
    pub score: f32,
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
}

pub struct Detector {
    session: Session,
    /// The model's square input side.
    size: u32,
}

impl Detector {
    pub fn load(model: &Path, threads: usize) -> anyhow::Result<Self> {
        let session = Session::builder()
            .map_err(|e| anyhow!("{e}"))?
            .with_intra_threads(threads)
            .map_err(|e| anyhow!("{e}"))?
            .commit_from_file(model)
            .map_err(|e| anyhow!("{e}"))
            .with_context(|| format!("loading {}", model.display()))?;
        let size = session
            .inputs()
            .first()
            .and_then(|input| input.dtype().tensor_shape())
            .and_then(|shape| shape.last().copied())
            .and_then(|side| u32::try_from(side).ok())
            .ok_or_else(|| anyhow!("model input has no fixed size"))?;
        Ok(Self { session, size })
    }

    /// Every detection in `image` scoring at least `min_score`, best class per query.
    pub fn detect(&mut self, image: &RgbImage, min_score: f32) -> anyhow::Result<Vec<Detection>> {
        let size = self.size;
        let resized = imageops::resize(image, size, size, imageops::FilterType::Triangle);
        let plane = (size * size) as usize;
        let mut input = vec![0f32; 3 * plane];
        for (i, pixel) in resized.pixels().enumerate() {
            for c in 0..3 {
                input[c * plane + i] = (f32::from(pixel[c]) / 255.0 - MEAN[c]) / STD[c];
            }
        }
        let tensor = Tensor::from_array(([1usize, 3, size as usize, size as usize], input))
            .map_err(|e| anyhow!("{e}"))?;
        let outputs = self
            .session
            .run(ort::inputs!["input" => tensor])
            .map_err(|e| anyhow!("{e}"))?;
        // Match outputs by name: `dets` is normalised cx,cy,w,h; `labels` are logits with
        // the background in the last slot.
        let (_, dets) = outputs["dets"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("{e}"))?;
        let (label_shape, logits) = outputs["labels"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("{e}"))?;
        decode(dets, label_shape, logits, min_score)
    }
}

/// The detections in a model's outputs: `dets` has 4 numbers per query, `logits` one per
/// class with the background in the last slot (`label_shape` says how many). Each query
/// keeps its best class, if that scores at least `min_score`.
fn decode(
    dets: &[f32],
    label_shape: &[i64],
    logits: &[f32],
    min_score: f32,
) -> anyhow::Result<Vec<Detection>> {
    let slots = *label_shape
        .last()
        .ok_or_else(|| anyhow!("labels has no shape"))? as usize;
    let classes = slots.saturating_sub(1).max(1);
    Ok(dets
        .as_chunks::<4>()
        .0
        .iter()
        .zip(logits.chunks_exact(slots))
        .filter_map(|(b, l)| {
            let (class, logit) = l[..classes]
                .iter()
                .copied()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(&b.1))?;
            let score = 1.0 / (1.0 + (-logit).exp());
            (score >= min_score).then_some(Detection {
                class,
                score,
                cx: b[0],
                cy: b[1],
                w: b[2],
                h: b[3],
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_best_class_of_each_query() {
        // Three queries, two classes and the background. The background is never picked,
        // however sure the model is of it.
        let dets = [
            0.5, 0.25, 0.2, 0.1, //
            0.1, 0.2, 0.3, 0.4, //
            0.9, 0.9, 0.1, 0.1,
        ];
        let logits = [
            -2.0, 3.0, 9.0, // class 1
            0.0, -1.0, 9.0, // class 0 at 0.5: kept, the threshold is inclusive
            -3.0, -4.0, 9.0, // too unsure
        ];
        let found = decode(&dets, &[1, 3, 3], &logits, 0.5).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].class, 1);
        assert!((found[0].score - 0.952_574).abs() < 1e-5);
        assert_eq!(
            (found[0].cx, found[0].cy, found[0].w, found[0].h),
            (0.5, 0.25, 0.2, 0.1)
        );
        assert_eq!((found[1].class, found[1].score), (0, 0.5));
        assert_eq!(found[1].cx, 0.1);
    }

    #[test]
    fn a_model_with_only_the_background_slot_still_has_a_class() {
        let found = decode(&[0.5; 4], &[1, 1, 1], &[2.0], 0.5).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].class, 0);
    }

    #[test]
    fn labels_need_a_shape() {
        let error = decode(&[], &[], &[], 0.5).unwrap_err();
        assert_eq!(error.to_string(), "labels has no shape");
    }
}
