"""File management routes."""

import os
from pathlib import Path
from typing import List, Optional
from fastapi import APIRouter, Depends, HTTPException, status
from sqlalchemy.orm import Session
from pydantic import BaseModel

from ..database import get_db
from ..models.user import User
from ..models.project import Project
from ..auth import get_current_active_user
from ..config import settings

router = APIRouter()


class FileNode(BaseModel):
    """File tree node schema."""
    name: str
    path: str
    type: str  # 'file' or 'directory'
    size: Optional[int] = None
    children: Optional[List['FileNode']] = None


class FileContent(BaseModel):
    """File content schema."""
    path: str
    content: str
    size: int


class FileUpdate(BaseModel):
    """File update schema."""
    content: str


@router.get("/{project_id}/tree", response_model=List[FileNode])
def get_file_tree(
    project_id: int,
    path: str = "",
    current_user: User = Depends(get_current_active_user),
    db: Session = Depends(get_db)
):
    """Get file tree for a project."""
    # Get project
    project = db.query(Project).filter(
        Project.id == project_id,
        Project.user_id == current_user.id
    ).first()
    
    if not project:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Project not found"
        )
    
    # Build full path
    project_path = Path(settings.projects_root) / project.path
    if path:
        full_path = project_path / path
    else:
        full_path = project_path
    
    # Security check - ensure path is within project
    try:
        full_path = full_path.resolve()
        if not str(full_path).startswith(str(project_path.resolve())):
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail="Access denied"
            )
    except Exception:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Invalid path"
        )
    
    if not full_path.exists():
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Path not found"
        )
    
    # Build file tree
    nodes = []
    try:
        for item in sorted(full_path.iterdir(), key=lambda x: (not x.is_dir(), x.name)):
            # Skip hidden files and common ignore patterns
            if item.name.startswith('.') or item.name in ['node_modules', '__pycache__', 'venv', 'dist', 'build']:
                continue
            
            rel_path = str(item.relative_to(project_path))
            node = FileNode(
                name=item.name,
                path=rel_path,
                type='directory' if item.is_dir() else 'file',
                size=item.stat().st_size if item.is_file() else None
            )
            nodes.append(node)
    except PermissionError:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Permission denied"
        )
    
    return nodes


@router.get("/{project_id}/content", response_model=FileContent)
def get_file_content(
    project_id: int,
    path: str,
    current_user: User = Depends(get_current_active_user),
    db: Session = Depends(get_db)
):
    """Get file content."""
    # Get project
    project = db.query(Project).filter(
        Project.id == project_id,
        Project.user_id == current_user.id
    ).first()
    
    if not project:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Project not found"
        )
    
    # Build full path
    project_path = Path(settings.projects_root) / project.path
    full_path = project_path / path
    
    # Security check
    try:
        full_path = full_path.resolve()
        if not str(full_path).startswith(str(project_path.resolve())):
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail="Access denied"
            )
    except Exception:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Invalid path"
        )
    
    if not full_path.exists() or not full_path.is_file():
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="File not found"
        )
    
    # Read file content
    try:
        with open(full_path, 'r', encoding='utf-8') as f:
            content = f.read()
        
        return FileContent(
            path=path,
            content=content,
            size=full_path.stat().st_size
        )
    except UnicodeDecodeError:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="File is not a text file"
        )
    except Exception as e:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail=f"Error reading file: {str(e)}"
        )


@router.put("/{project_id}/content")
def update_file_content(
    project_id: int,
    path: str,
    file_update: FileUpdate,
    current_user: User = Depends(get_current_active_user),
    db: Session = Depends(get_db)
):
    """Update file content."""
    # Get project
    project = db.query(Project).filter(
        Project.id == project_id,
        Project.user_id == current_user.id
    ).first()
    
    if not project:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Project not found"
        )
    
    # Build full path
    project_path = Path(settings.projects_root) / project.path
    full_path = project_path / path
    
    # Security check
    try:
        full_path = full_path.resolve()
        if not str(full_path).startswith(str(project_path.resolve())):
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail="Access denied"
            )
    except Exception:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Invalid path"
        )
    
    if not full_path.exists() or not full_path.is_file():
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="File not found"
        )
    
    # Write file content
    try:
        with open(full_path, 'w', encoding='utf-8') as f:
            f.write(file_update.content)
        
        return {"message": "File updated successfully"}
    except Exception as e:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail=f"Error writing file: {str(e)}"
        )

