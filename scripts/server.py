#!/usr/bin/env python3
"""Development server for CodeForge."""

import uvicorn
from codeforge.config import settings


def main():
    """Run the development server."""
    uvicorn.run(
        "codeforge.main:app",
        host=settings.host,
        port=settings.port,
        reload=True,
    )


if __name__ == "__main__":
    main()

