"""Page routes for serving HTML templates."""

from fastapi import APIRouter, Request, Depends, HTTPException
from fastapi.responses import HTMLResponse, RedirectResponse
from fastapi.templating import Jinja2Templates
from pathlib import Path
from sqlalchemy.orm import Session

from ..auth import get_current_user_from_cookie
from ..models.user import User
from ..database import get_db

router = APIRouter()

# Setup templates
templates_dir = Path(__file__).parent.parent / "templates"
templates = Jinja2Templates(directory=str(templates_dir))


@router.get("/", response_class=HTMLResponse)
async def index(request: Request, db: Session = Depends(get_db)):
    """Landing page - redirects to dashboard."""
    current_user = get_current_user_from_cookie(request, db)
    # Check if we're behind a proxy with /code/ prefix
    prefix = "/code" if request.headers.get("x-forwarded-prefix") == "/code" else ""
    return RedirectResponse(url=f"{prefix}/dashboard")


@router.get("/login", response_class=HTMLResponse)
async def login_page(request: Request):
    """Login page."""
    return templates.TemplateResponse("login.html", {"request": request})


@router.get("/register", response_class=HTMLResponse)
async def register_page(request: Request):
    """Registration page."""
    return templates.TemplateResponse("register.html", {"request": request})


@router.get("/dashboard", response_class=HTMLResponse)
async def dashboard(request: Request, db: Session = Depends(get_db)):
    """Main dashboard page."""
    current_user = get_current_user_from_cookie(request, db)
    return templates.TemplateResponse("dashboard.html", {
        "request": request,
        "current_user": current_user
    })


@router.get("/settings", response_class=HTMLResponse)
async def settings_page(request: Request, db: Session = Depends(get_db)):
    """Settings page."""
    current_user = get_current_user_from_cookie(request, db)
    return templates.TemplateResponse("settings.html", {
        "request": request,
        "current_user": current_user
    })


@router.get("/chat/{conversation_id}", response_class=HTMLResponse)
async def chat_page(request: Request, conversation_id: int, db: Session = Depends(get_db)):
    """Chat page for a specific conversation."""
    current_user = get_current_user_from_cookie(request, db)
    return templates.TemplateResponse("chat.html", {
        "request": request,
        "conversation_id": conversation_id,
        "current_user": current_user
    })

