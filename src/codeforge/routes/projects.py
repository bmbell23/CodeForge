"""Project management routes."""

import os
from pathlib import Path
from typing import List
from datetime import datetime
from fastapi import APIRouter, Depends, HTTPException, status
from sqlalchemy.orm import Session
from pydantic import BaseModel

from ..database import get_db
from ..models.user import User
from ..models.project import Project
from ..auth import get_current_active_user
from ..config import settings

router = APIRouter()


class ProjectCreate(BaseModel):
    """Project creation schema."""
    name: str
    path: str
    description: str = ""


class ProjectUpdate(BaseModel):
    """Project update schema."""
    name: str
    description: str = ""


class ProjectResponse(BaseModel):
    """Project response schema."""
    id: int
    name: str
    path: str
    description: str
    is_active: bool
    last_accessed: datetime
    created_at: datetime

    class Config:
        from_attributes = True


class ProjectScan(BaseModel):
    """Scanned project schema."""
    name: str
    path: str
    exists_in_db: bool


@router.get("/", response_model=List[ProjectResponse])
def list_projects(
    current_user: User = Depends(get_current_active_user),
    db: Session = Depends(get_db)
):
    """List all projects for the current user."""
    projects = db.query(Project).filter(
        Project.user_id == current_user.id,
        Project.is_active == True
    ).order_by(Project.last_accessed.desc()).all()
    return projects


@router.get("/scan", response_model=List[ProjectScan])
def scan_projects(
    current_user: User = Depends(get_current_active_user),
    db: Session = Depends(get_db)
):
    """Scan the projects directory for available projects."""
    projects_root = Path(settings.projects_root)
    if not projects_root.exists():
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail=f"Projects root directory not found: {projects_root}"
        )
    
    scanned_projects = []
    existing_projects = {
        p.path: p for p in db.query(Project).filter(
            Project.user_id == current_user.id
        ).all()
    }
    
    # Scan directories
    for item in projects_root.iterdir():
        if item.is_dir() and not item.name.startswith('.'):
            rel_path = item.name
            scanned_projects.append(ProjectScan(
                name=item.name,
                path=rel_path,
                exists_in_db=rel_path in existing_projects
            ))
    
    return scanned_projects


@router.post("/", response_model=ProjectResponse)
def create_project(
    project_data: ProjectCreate,
    current_user: User = Depends(get_current_active_user),
    db: Session = Depends(get_db)
):
    """Create a new project."""
    # Verify project path exists
    project_path = Path(settings.projects_root) / project_data.path
    if not project_path.exists():
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail=f"Project path not found: {project_path}"
        )
    
    # Check if project already exists
    existing = db.query(Project).filter(
        Project.user_id == current_user.id,
        Project.path == project_data.path
    ).first()
    
    if existing:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Project already exists"
        )
    
    # Create project
    project = Project(
        user_id=current_user.id,
        name=project_data.name,
        path=project_data.path,
        description=project_data.description,
    )
    db.add(project)
    db.commit()
    db.refresh(project)
    return project


@router.get("/{project_id}", response_model=ProjectResponse)
def get_project(
    project_id: int,
    current_user: User = Depends(get_current_active_user),
    db: Session = Depends(get_db)
):
    """Get a specific project."""
    project = db.query(Project).filter(
        Project.id == project_id,
        Project.user_id == current_user.id
    ).first()
    
    if not project:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Project not found"
        )
    
    # Update last accessed
    project.last_accessed = datetime.utcnow()
    db.commit()
    
    return project


@router.put("/{project_id}", response_model=ProjectResponse)
def update_project(
    project_id: int,
    project_data: ProjectUpdate,
    current_user: User = Depends(get_current_active_user),
    db: Session = Depends(get_db)
):
    """Update a project."""
    project = db.query(Project).filter(
        Project.id == project_id,
        Project.user_id == current_user.id
    ).first()

    if not project:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Project not found"
        )

    # Update project fields
    project.name = project_data.name
    project.description = project_data.description
    project.updated_at = datetime.utcnow()

    db.commit()
    db.refresh(project)

    return project


@router.delete("/{project_id}")
def delete_project(
    project_id: int,
    current_user: User = Depends(get_current_active_user),
    db: Session = Depends(get_db)
):
    """Delete a project (soft delete)."""
    project = db.query(Project).filter(
        Project.id == project_id,
        Project.user_id == current_user.id
    ).first()

    if not project:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Project not found"
        )

    project.is_active = False
    db.commit()

    return {"message": "Project deleted successfully"}

