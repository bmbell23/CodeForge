"""Authentication utilities."""

from datetime import datetime, timedelta
from typing import Optional
from fastapi import Depends, HTTPException, status, Request
from fastapi.security import OAuth2PasswordBearer
from jose import JWTError, jwt
import bcrypt
from sqlalchemy.orm import Session

from .config import settings
from .database import get_db
from .models.user import User

oauth2_scheme = OAuth2PasswordBearer(tokenUrl="api/auth/login")


def verify_password(plain_password: str, hashed_password: str) -> bool:
    """Verify a password against a hash."""
    return bcrypt.checkpw(plain_password.encode('utf-8'), hashed_password.encode('utf-8'))


def get_password_hash(password: str) -> str:
    """Hash a password."""
    salt = bcrypt.gensalt()
    hashed = bcrypt.hashpw(password.encode('utf-8'), salt)
    return hashed.decode('utf-8')


def create_access_token(data: dict, expires_delta: Optional[timedelta] = None) -> str:
    """Create a JWT access token."""
    to_encode = data.copy()
    if expires_delta:
        expire = datetime.utcnow() + expires_delta
    else:
        expire = datetime.utcnow() + timedelta(minutes=settings.access_token_expire_minutes)
    to_encode.update({"exp": expire})
    encoded_jwt = jwt.encode(to_encode, settings.secret_key, algorithm=settings.algorithm)
    return encoded_jwt


def get_current_user(
    token: str = Depends(oauth2_scheme),
    db: Session = Depends(get_db)
) -> User:
    """Get the current authenticated user."""
    credentials_exception = HTTPException(
        status_code=status.HTTP_401_UNAUTHORIZED,
        detail="Could not validate credentials",
        headers={"WWW-Authenticate": "Bearer"},
    )
    try:
        payload = jwt.decode(token, settings.secret_key, algorithms=[settings.algorithm])
        username: str = payload.get("sub")
        if username is None:
            raise credentials_exception
    except JWTError:
        raise credentials_exception
    
    user = db.query(User).filter(User.username == username).first()
    if user is None:
        raise credentials_exception
    return user


def get_current_active_user(current_user: User = Depends(get_current_user)) -> User:
    """Get the current active user."""
    if not current_user.is_active:
        raise HTTPException(status_code=400, detail="Inactive user")
    return current_user


def get_current_user_from_cookie(request: Request, db: Session) -> Optional[User]:
    """Get the current user from the session cookie.

    If no cookie is present, automatically returns the 'brandon' user
    for simplified access on private servers.
    """
    token = request.cookies.get("access_token")

    if not token:
        # Auto-login as brandon user when no authentication cookie is present
        user = db.query(User).filter(User.username == "brandon").first()
        return user

    try:
        payload = jwt.decode(token, settings.secret_key, algorithms=[settings.algorithm])
        username: str = payload.get("sub")
        if username is None:
            # If no username in token, fall back to brandon user
            user = db.query(User).filter(User.username == "brandon").first()
            return user
    except JWTError:
        # If token is invalid, fall back to brandon user
        user = db.query(User).filter(User.username == "brandon").first()
        return user

    user = db.query(User).filter(User.username == username).first()
    return user


def require_current_user(request: Request, db: Session = Depends(get_db)) -> User:
    """Require authentication (always returns brandon user if not authenticated)."""
    user = get_current_user_from_cookie(request, db)

    if user is None:
        # This should never happen since get_current_user_from_cookie
        # now auto-returns brandon, but just in case:
        user = db.query(User).filter(User.username == "brandon").first()
        if not user:
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail="Default user 'brandon' not found in database"
            )

    return user

