#!/usr/bin/env python3
"""Script to create a user for CodeForge."""

import sys
from sqlalchemy.orm import Session

from codeforge.database import SessionLocal, engine, Base
from codeforge.models.user import User
from codeforge.auth import get_password_hash


def create_user(username: str, email: str, password: str, is_admin: bool = False):
    """Create a new user."""
    # Create tables if they don't exist
    Base.metadata.create_all(bind=engine)
    
    db = SessionLocal()
    try:
        # Check if user exists
        existing = db.query(User).filter(User.username == username).first()
        if existing:
            print(f"User '{username}' already exists!")
            return
        
        # Create user
        user = User(
            username=username,
            email=email,
            hashed_password=get_password_hash(password),
            is_admin=is_admin,
        )
        db.add(user)
        db.commit()
        db.refresh(user)
        
        print(f"User '{username}' created successfully!")
        print(f"  Email: {email}")
        print(f"  Admin: {is_admin}")
    
    finally:
        db.close()


if __name__ == "__main__":
    if len(sys.argv) < 4:
        print("Usage: python create_user.py <username> <email> <password> [--admin]")
        sys.exit(1)
    
    username = sys.argv[1]
    email = sys.argv[2]
    password = sys.argv[3]
    is_admin = "--admin" in sys.argv
    
    create_user(username, email, password, is_admin)

