"""Tiny stand-ins for the killfeed models, for tests.

The real models are private and never downloaded in CI. These have the same input and
outputs as an RF-DETR export (`input` 1x3xSxS float32; `dets` 1xNx4 normalised cx, cy, w,
h; `labels` 1xNx(C+1) logits, background last) but constant answers, so tests can assert
exactly what the crate makes of them. The committed .onnx files are what the tests use;
rerun this only to change them:

    python3 -m venv venv && venv/bin/pip install onnx
    venv/bin/python crates/killfeed/tests/models/make_models.py

Writes, next to this script:

- `hud/`: a HUD locator (64x64 input, three of its classes) whose boxes, on a 640x360
  frame, are: two killfeed rows in the killfeed corner (x 370.., y ..180), a near-copy
  of the first, a row-like box in the chat (bottom left), a radar, and a row too unsure.
  Its scores drop to nothing on dark frames (average pixel below about 65), so a black
  frame has no rows.
- `icons/`: an icon reader (32x32 input) answering for the first three slots of a sheet
  and the eleventh. Row 1: an AK-47 (seen twice) that beats an AWP, a headshot and a
  flash assist, and a wallbang too weak to count. Row 2: an AWP. Row 3: an AK-47 too
  unsure to count. Row 11: an AWP.
- `dynamic.onnx`: a model without a fixed input size, which the crate refuses.
- `misfits/`: models that run but don't fit the crate: an input not called `input`
  (`input-name.onnx`), and integer boxes or labels (`int-dets.onnx`, `int-labels.onnx`).

Each folder is laid out like a published model version: `model.onnx`, `classes.json`
and a `model-card.json` with the files' SHA-256.
"""

import hashlib
import json
from pathlib import Path

import numpy as np
import onnx
from onnx import TensorProto, helper, numpy_helper

HERE = Path(__file__).parent
OPSET = 17

# Sheet layout of icon_reader.rs: rows are 56 px slots, 2 px apart, from y = 2 on a
# 640 px sheet. The middle of slot i, normalised.
def slot_cy(i):
    return (2 + 58 * i + 29) / 640


def detector(path, size, boxes, logits, gate):
    """A detector answering `boxes` (Nx4) and `logits` (NxC+1) whatever the input; with
    `gate`, the logits fall by 10 for every unit the input's mean (after ImageNet
    normalisation) is below -1, so dark frames find nothing."""
    boxes = np.asarray(boxes, np.float32)[None]
    logits = np.asarray(logits, np.float32)[None]
    nodes = [helper.make_node("Identity", ["boxes"], ["dets"])]
    inits = [numpy_helper.from_array(boxes, "boxes"), numpy_helper.from_array(logits, "logits")]
    if gate:
        nodes += [
            helper.make_node("ReduceMean", ["input"], ["mean"], keepdims=0),
            helper.make_node("Add", ["mean", "one"], ["shifted"]),
            helper.make_node("Min", ["shifted", "zero"], ["dark"]),
            helper.make_node("Mul", ["dark", "ten"], ["penalty"]),
            helper.make_node("Add", ["logits", "penalty"], ["labels"]),
        ]
        inits += [
            numpy_helper.from_array(np.float32(v), name)
            for name, v in [("one", 1.0), ("zero", 0.0), ("ten", 10.0)]
        ]
    else:
        nodes.append(helper.make_node("Identity", ["logits"], ["labels"]))
    graph = helper.make_graph(
        nodes,
        "detector",
        [helper.make_tensor_value_info("input", TensorProto.FLOAT, [1, 3, size, size])],
        [
            helper.make_tensor_value_info("dets", TensorProto.FLOAT, list(boxes.shape)),
            helper.make_tensor_value_info("labels", TensorProto.FLOAT, list(logits.shape)),
        ],
        inits,
    )
    save(graph, path)


def save(graph, path):
    model = helper.make_model(
        graph, opset_imports=[helper.make_opsetid("", OPSET)], producer_name="clipos-tests"
    )
    model.ir_version = 8
    onnx.checker.check_model(model, full_check=True)
    path.parent.mkdir(parents=True, exist_ok=True)
    onnx.save(model, path)


def logit_row(classes, cls, logit):
    """One query's logits: `logit` for `cls`, very unsure of every other class, and the
    background (ignored by the crate) highest of all."""
    row = [-10.0] * len(classes) + [8.0]
    row[classes.index(cls)] = logit
    return row


def publish(folder, classes):
    (folder / "classes.json").write_text(json.dumps(classes))
    files = {
        name: {"sha256": hashlib.sha256((folder / name).read_bytes()).hexdigest()}
        for name in ["model.onnx", "classes.json"]
    }
    card = {"name": folder.name, "version": "test", "classes": classes, "files": files}
    (folder / "model-card.json").write_text(json.dumps(card, indent=2) + "\n")


def hud():
    classes = ["radar", "killfeed_row", "money"]
    W, H = 640, 360

    def box(x0, y0, x1, y1):
        """Frame pixels to the normalised cx, cy, w, h a locator answers."""
        return ((x0 + x1) / 2 / W, (y0 + y1) / 2 / H, (x1 - x0) / W, (y1 - y0) / H)

    queries = [
        # box in frame pixels, class, logit
        (box(478, 30, 640, 42), "killfeed_row", 4.0),  # a row: the corner's 108..270 x 30..42
        (box(505, 48, 640, 60), "killfeed_row", 3.0),  # a second row, below
        (box(480, 31, 640, 43), "killfeed_row", 2.0),  # a near-copy of the first, weaker
        (box(20, 250, 220, 262), "killfeed_row", 4.0),  # in the chat, outside the corner
        (box(10, 10, 110, 110), "radar", 4.0),  # another element
        (box(478, 80, 640, 92), "killfeed_row", -3.0),  # too unsure
    ]
    folder = HERE / "hud"
    detector(
        folder / "model.onnx",
        64,
        [b for b, _, _ in queries],
        [logit_row(classes, cls, logit) for _, cls, logit in queries],
        gate=True,
    )
    publish(folder, classes)


def icons():
    classes = ["ak47", "awp", "headshot", "wallbang", "flash_assist"]
    queries = [
        # slot, class, logit
        (0, "ak47", 2.0),
        (0, "ak47", 1.0),
        (0, "awp", 0.0),
        (0, "headshot", 1.0),
        (0, "wallbang", -0.5),
        (0, "flash_assist", 3.0),
        (1, "awp", 0.5),
        (2, "ak47", -0.2),
        (10, "awp", 1.5),
        (1, "headshot", -6.0),  # below the detection threshold
    ]
    folder = HERE / "icons"
    detector(
        folder / "model.onnx",
        32,
        [(0.5, slot_cy(slot), 0.05, 0.05) for slot, _, _ in queries],
        [logit_row(classes, cls, logit) for _, cls, logit in queries],
        gate=False,
    )
    publish(folder, classes)


def dynamic():
    graph = helper.make_graph(
        [
            helper.make_node("Identity", ["input"], ["dets"]),
            helper.make_node("Identity", ["input"], ["labels"]),
        ],
        "dynamic",
        [helper.make_tensor_value_info("input", TensorProto.FLOAT, ["n", 3, "h", "w"])],
        [
            helper.make_tensor_value_info("dets", TensorProto.FLOAT, ["n", 3, "h", "w"]),
            helper.make_tensor_value_info("labels", TensorProto.FLOAT, ["n", 3, "h", "w"]),
        ],
    )
    save(graph, HERE / "dynamic.onnx")


def misfit(name, input_name="input", dets=TensorProto.FLOAT, labels=TensorProto.FLOAT):
    """An 8x8 detector with one constant query, its input and outputs as given."""
    def const(out, elem_type, shape):
        dtype = onnx.helper.tensor_dtype_to_np_dtype(elem_type)
        value = numpy_helper.from_array(np.ones(shape, dtype), out + "_value")
        return helper.make_node("Constant", [], [out], value=value)

    graph = helper.make_graph(
        [const("dets", dets, [1, 1, 4]), const("labels", labels, [1, 1, 2])],
        "misfit",
        [helper.make_tensor_value_info(input_name, TensorProto.FLOAT, [1, 3, 8, 8])],
        [
            helper.make_tensor_value_info("dets", dets, [1, 1, 4]),
            helper.make_tensor_value_info("labels", labels, [1, 1, 2]),
        ],
    )
    save(graph, HERE / "misfits" / name)


if __name__ == "__main__":
    hud()
    icons()
    dynamic()
    misfit("input-name.onnx", input_name="images")
    misfit("int-dets.onnx", dets=TensorProto.INT64)
    misfit("int-labels.onnx", labels=TensorProto.INT64)
