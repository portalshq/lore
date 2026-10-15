#!/usr/bin/env python

import os
import subprocess

if os.name == "nt":
    # On Windows and if present, remove git for Windows' bin directory from the PATH, as this contains
    # link.exe, which overrides MSVC's link.exe
    split = os.environ["PATH"].split(os.pathsep)
    removed = [path for path in split if not path.lower().endswith(r"git\usr\bin")]
    os.environ["PATH"] = os.pathsep.join(removed)

result = subprocess.run(
    [
        "cargo",
        "clippy",
        "--all-targets",
        "--",
        "-D",
        "warnings",
        # async_trait expands Future-returning methods with duplicate must_use.
        "-A",
        "clippy::double_must_use",
        "--no-deps",
    ]
)

exit(result.returncode)
