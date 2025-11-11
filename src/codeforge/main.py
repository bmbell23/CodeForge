"""Main FastAPI application."""

from fastapi import FastAPI
from fastapi.staticfiles import StaticFiles
from fastapi.templating import Jinja2Templates
from pathlib import Path

from .database import engine, Base
from .routes import auth, chat, projects, files, git, pages, terminal

# Create database tables
Base.metadata.create_all(bind=engine)

# Create FastAPI app
app = FastAPI(
    title="CodeForge",
    description="Web-based IDE centered around Augment CLI",
    version="0.1.0",
)

# Get the package directory
package_dir = Path(__file__).parent

# Mount static files
static_dir = package_dir / "static"
static_dir.mkdir(exist_ok=True)
app.mount("/static", StaticFiles(directory=str(static_dir)), name="static")

# Setup templates
templates_dir = package_dir / "templates"
templates_dir.mkdir(exist_ok=True)
templates = Jinja2Templates(directory=str(templates_dir))

# Include routers
app.include_router(auth.router, prefix="/api/auth", tags=["auth"])
app.include_router(chat.router, prefix="/api/chat", tags=["chat"])
app.include_router(projects.router, prefix="/api/projects", tags=["projects"])
app.include_router(files.router, prefix="/api/files", tags=["files"])
app.include_router(git.router, prefix="/api/git", tags=["git"])
app.include_router(terminal.router, prefix="/api/terminal", tags=["terminal"])
app.include_router(pages.router, tags=["pages"])


@app.get("/health")
async def health_check():
    """Health check endpoint."""
    return {"status": "healthy"}

