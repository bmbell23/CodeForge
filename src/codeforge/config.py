"""Configuration management for CodeForge."""

import os
from pathlib import Path
from pydantic_settings import BaseSettings


class Settings(BaseSettings):
    """Application settings."""

    # Database
    database_url: str = "sqlite:///./codeforge.db"

    # Security
    secret_key: str = "dev-secret-key-change-in-production"
    algorithm: str = "HS256"
    access_token_expire_minutes: int = 43200  # 30 days

    # Server
    host: str = "0.0.0.0"
    port: int = 8004

    # Projects
    projects_root: str = str(Path.home() / "projects")

    # Augment CLI
    auggie_path: str = "auggie"
    node_path: str = "/usr/bin/node"
    use_mock_augment: bool = False  # Set to False when you have auggie installed

    class Config:
        env_file = ".env"
        case_sensitive = False


settings = Settings()

