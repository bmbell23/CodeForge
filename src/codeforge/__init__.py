"""CodeForge - Web-based IDE centered around Augment CLI."""

import tomllib
from pathlib import Path


def _get_version() -> str:
    """Read version from pyproject.toml."""
    try:
        pyproject_path = Path(__file__).parent.parent.parent / "pyproject.toml"
        with open(pyproject_path, "rb") as f:
            pyproject_data = tomllib.load(f)
        return pyproject_data["project"]["version"]
    except Exception:
        # Fallback version if pyproject.toml can't be read
        return "0.0.0"


__version__ = _get_version()

