"""LinearLearning support on the main board: learns the ESP32 final-weight predictor's per-machine,
per-retraction calibration from finished shots and sends it to the ESP32, and hands LinearLearning the
final-weight stop only on a machine where it beat the stock prediction (earned control)."""

from .calibrator import LinearLearningCalibrator, replay
from .calibration import CalibrationStore, bucket_key, fit
from .control import EarnedControl
from .labeler import PostDecisionTrace
from .model import FleetModel
from .stream import FeatureStream

__all__ = [
    "LinearLearningCalibrator",
    "replay",
    "CalibrationStore",
    "bucket_key",
    "fit",
    "EarnedControl",
    "PostDecisionTrace",
    "FleetModel",
    "FeatureStream",
]
