#!/usr/bin/env python3
"""Backward-compatible wrapper for the renamed pikaci run summary tool."""

from pathlib import Path
import runpy


runpy.run_path(
    str(Path(__file__).with_name("pikaci-run-summary.py")),
    run_name="__main__",
)
