#!/usr/bin/env python3
"""CLI boundary tests with fake engines; no physical GPU or COLMAP required."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

binary = Path(sys.argv[1]).resolve()
with tempfile.TemporaryDirectory(prefix="ooosplat-backend-") as temporary:
    root = Path(temporary)
    engine = root / "fake-engine"
    engine.write_text("#!" + sys.executable + "\n" + r'''
import os, sys, time
from pathlib import Path
args = sys.argv[1:]
family = os.environ.get("FAKE_FAMILY", "legacy")
extraction = "FeatureExtraction" if family == "modern" else "SiftExtraction"
matching = "FeatureMatching" if family == "modern" else "SiftMatching"
if "-h" in args or "--help" in args or "-version" in args:
    print("COLMAP test (" + os.environ.get("FAKE_CUDA", "with CUDA") + ")")
    print("--" + extraction + ".use_gpu --" + matching + ".use_gpu")
    sys.exit(0)
command = args[0]
with open(os.environ["FAKE_CALLS"], "a") as stream:
    stream.write(command + "\n")
if os.environ.get("FAKE_TIMEOUT"):
    time.sleep(60)
prefix = extraction if command == "feature_extractor" else matching
assert args[args.index("--" + prefix + ".use_gpu") + 1] == "1"
assert args[args.index("--" + prefix + ".gpu_index") + 1] == "0"
assert os.environ.get("CUDA_VISIBLE_DEVICES") == "2"
database = Path(args[args.index("--database_path") + 1])
if command == "feature_extractor":
    images = Path(args[args.index("--image_path") + 1])
    assert len(list(images.glob("*.png"))) == 2
    database.write_text("fake database")
else:
    assert command == "exhaustive_matcher" and database.is_file()
if os.environ.get("FAKE_FAIL") == command:
    print("CUDA device initialization failed", file=sys.stderr)
    sys.exit(1)
''')
    engine.chmod(0o755)
    calls = root / "calls"
    # An explicit engine directory must still honor individual engine overrides.
    base = dict(os.environ)
    for variable in list(base):
        if variable.startswith("OOOSPLAT_"):
            del base[variable]
    base.update({"OOOSPLAT_" + name: str(engine)
                 for name in ("FFMPEG", "FFPROBE", "COLMAP", "BRUSH")})
    base.update(FAKE_CALLS=str(calls), CUDA_VISIBLE_DEVICES="2", TMPDIR=str(root))
    cases = [
        ("default gpu", {}, 0, "gpu", 2),
        ("modern gpu", {"FAKE_FAMILY": "modern"}, 0, "gpu", 2),
        ("cpu build", {"FAKE_CUDA": "without CUDA"}, 0, "cpu", 0),
        ("unknown build", {"FAKE_CUDA": "unknown"}, 0, "cpu", 0),
        ("explicit cpu", {"OOOSPLAT_COLMAP_BACKEND": "cpu"}, 0, "cpu", 0),
        ("extract failure", {"FAKE_FAIL": "feature_extractor"}, 0, "cpu", 1),
        ("match failure", {"FAKE_FAIL": "exhaustive_matcher"}, 0, "cpu", 2),
        ("strict gpu", {"OOOSPLAT_COLMAP_BACKEND": "gpu"}, 0, "gpu", 2),
        ("strict missing cuda", {"OOOSPLAT_COLMAP_BACKEND": "gpu", "FAKE_CUDA": "without CUDA"}, 1, "cpu", 0),
        ("strict device failure", {"OOOSPLAT_COLMAP_BACKEND": "gpu", "FAKE_FAIL": "feature_extractor"}, 1, "cpu", 1),
        ("probe timeout", {"FAKE_TIMEOUT": "1"}, 0, "cpu", 1),
        ("invalid mode", {"OOOSPLAT_COLMAP_BACKEND": "gup"}, 1, "cpu", 0),
    ]
    for name, overrides, code, backend, count in cases:
        calls.write_text("")
        result = subprocess.run([str(binary), "--engine-dir", str(root), "health"],
                                env={**base, **overrides}, capture_output=True,
                                text=True, timeout=45)
        assert result.returncode == code, (name, result.stderr, result.stdout)
        statuses = json.loads(result.stdout)
        colmap = next(s for s in statuses if s["kind"] == "colmap")
        assert colmap["acceleration"]["backend"] == backend, (name, colmap)
        assert colmap["canStart"] == (code == 0), (name, colmap)
        assert len(calls.read_text().splitlines()) == count, name
        assert not list(root.glob("ooosplat-cuda-*")), "leaked probe directory"
        print("PASS:", name)
print("12 CLI backend scenarios passed (mock engines; no real GPU exercised)")
