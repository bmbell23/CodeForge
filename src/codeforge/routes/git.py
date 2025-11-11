"""Git integration routes."""

from pathlib import Path
from typing import List, Optional
from datetime import datetime
from fastapi import APIRouter, Depends, HTTPException, status
from sqlalchemy.orm import Session
from pydantic import BaseModel
import git

from ..database import get_db
from ..models.user import User
from ..models.project import Project
from ..auth import get_current_active_user
from ..config import settings

router = APIRouter()


class GitStatus(BaseModel):
    """Git status schema."""
    branch: str
    is_dirty: bool
    untracked_files: List[str]
    modified_files: List[str]
    staged_files: List[str]


class GitCommit(BaseModel):
    """Git commit schema."""
    sha: str
    author: str
    email: str
    message: str
    date: datetime


class GitDiff(BaseModel):
    """Git diff schema."""
    file_path: str
    diff: str


@router.get("/{project_id}/status", response_model=GitStatus)
def get_git_status(
    project_id: int,
    current_user: User = Depends(get_current_active_user),
    db: Session = Depends(get_db)
):
    """Get git status for a project."""
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
    
    # Get project path
    project_path = Path(settings.projects_root) / project.path
    
    try:
        repo = git.Repo(project_path)
    except git.InvalidGitRepositoryError:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Not a git repository"
        )
    
    # Get status
    untracked = repo.untracked_files
    modified = [item.a_path for item in repo.index.diff(None)]
    staged = [item.a_path for item in repo.index.diff('HEAD')]
    
    return GitStatus(
        branch=repo.active_branch.name,
        is_dirty=repo.is_dirty(),
        untracked_files=untracked,
        modified_files=modified,
        staged_files=staged
    )


@router.get("/{project_id}/log", response_model=List[GitCommit])
def get_git_log(
    project_id: int,
    limit: int = 50,
    current_user: User = Depends(get_current_active_user),
    db: Session = Depends(get_db)
):
    """Get git commit history."""
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
    
    # Get project path
    project_path = Path(settings.projects_root) / project.path
    
    try:
        repo = git.Repo(project_path)
    except git.InvalidGitRepositoryError:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Not a git repository"
        )
    
    # Get commits
    commits = []
    for commit in repo.iter_commits(max_count=limit):
        commits.append(GitCommit(
            sha=commit.hexsha,
            author=commit.author.name,
            email=commit.author.email,
            message=commit.message.strip(),
            date=datetime.fromtimestamp(commit.committed_date)
        ))
    
    return commits


@router.get("/{project_id}/diff", response_model=List[GitDiff])
def get_git_diff(
    project_id: int,
    staged: bool = False,
    current_user: User = Depends(get_current_active_user),
    db: Session = Depends(get_db)
):
    """Get git diff for modified files."""
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
    
    # Get project path
    project_path = Path(settings.projects_root) / project.path
    
    try:
        repo = git.Repo(project_path)
    except git.InvalidGitRepositoryError:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Not a git repository"
        )
    
    # Get diffs
    diffs = []
    if staged:
        diff_index = repo.index.diff('HEAD')
    else:
        diff_index = repo.index.diff(None)
    
    for diff_item in diff_index:
        diffs.append(GitDiff(
            file_path=diff_item.a_path,
            diff=diff_item.diff.decode('utf-8') if diff_item.diff else ""
        ))
    
    return diffs


@router.get("/{project_id}/branches", response_model=List[str])
def get_git_branches(
    project_id: int,
    current_user: User = Depends(get_current_active_user),
    db: Session = Depends(get_db)
):
    """Get list of git branches."""
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
    
    # Get project path
    project_path = Path(settings.projects_root) / project.path
    
    try:
        repo = git.Repo(project_path)
    except git.InvalidGitRepositoryError:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Not a git repository"
        )
    
    # Get branches
    branches = [branch.name for branch in repo.branches]
    return branches

