# CodeForge - Project Summary

## Overview

CodeForge is a modern web-based IDE centered around the Augment CLI (auggie). It provides a beautiful, intuitive interface for coding with AI assistance, managing projects, editing files, and viewing git information - all from your browser.

## What We Built

### Backend (Python/FastAPI)

#### Core Application
- **main.py**: FastAPI application with WebSocket support
- **config.py**: Pydantic-based configuration management
- **database.py**: SQLAlchemy 2.0 database setup
- **auth.py**: JWT authentication with bcrypt password hashing

#### Database Models
- **User**: User accounts with authentication
- **Project**: Code projects from ~/projects/
- **Conversation**: Chat sessions with Augment
- **Message**: Individual messages in conversations

#### API Routes
- **auth.py**: User registration, login, and authentication
- **projects.py**: Project management (list, scan, create, delete)
- **chat.py**: Conversation management with WebSocket streaming
- **files.py**: File browsing and editing with security checks
- **git.py**: Git integration (status, log, diff, branches)
- **pages.py**: HTML page serving

#### Services
- **augment_service.py**: Integration with Augment CLI via subprocess
  - Streaming responses
  - Project-aware execution
  - Error handling

### Frontend (HTML/CSS/JavaScript)

#### Templates (Jinja2)
- **base.html**: Base template with Tailwind CSS and Alpine.js
- **login.html**: User login page
- **register.html**: User registration page
- **dashboard.html**: Main IDE interface with tabs

#### JavaScript (Alpine.js)
- **main.js**: API utilities and authentication
- **dashboard.js**: Main dashboard application logic
  - Project management
  - Conversation handling
  - WebSocket communication
  - File editing
  - Git integration

#### Styling (Tailwind CSS + Custom)
- **main.css**: Custom styles for code editor and scrollbars
- Responsive design
- Dark mode support

### Utilities

#### Scripts
- **server.py**: Development server launcher
- **create_user.py**: User creation utility
- **setup.sh**: Automated setup script

#### Documentation
- **README.md**: Comprehensive documentation
- **QUICKSTART.md**: Quick start guide
- **PROJECT_SUMMARY.md**: This file

#### Configuration
- **.env.example**: Environment configuration template
- **.gitignore**: Git ignore rules
- **pyproject.toml**: Python project configuration
- **alembic.ini**: Database migration configuration

## Key Features

### 1. AI-Powered Chat
- Real-time streaming responses from Augment CLI
- Multiple conversations per project
- Conversation history saved to database
- WebSocket-based communication

### 2. Project Management
- Automatic scanning of ~/projects/ directory
- Multiple project support
- Project switching from dropdown
- Last accessed tracking

### 3. File Editor
- File tree browser
- Text file editing
- Save functionality
- Security checks to prevent directory traversal

### 4. Git Integration
- Current branch and status
- Modified/staged/untracked files
- Commit history with details
- Diff viewing

### 5. Modern UI
- Clean, responsive design
- Dark mode support
- Tailwind CSS styling
- Alpine.js for reactivity
- Tab-based interface

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                         Browser                              │
│  ┌──────────────────────────────────────────────────────┐  │
│  │  Dashboard (Alpine.js + Tailwind CSS)                │  │
│  │  - Chat Interface                                     │  │
│  │  - File Editor                                        │  │
│  │  - Git Viewer                                         │  │
│  └──────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
                           │
                           │ HTTP/WebSocket
                           ▼
┌─────────────────────────────────────────────────────────────┐
│                    FastAPI Server                            │
│  ┌──────────────────────────────────────────────────────┐  │
│  │  API Routes                                           │  │
│  │  - /api/auth/*     (Authentication)                   │  │
│  │  - /api/projects/* (Project Management)               │  │
│  │  - /api/chat/*     (Conversations + WebSocket)        │  │
│  │  - /api/files/*    (File Operations)                  │  │
│  │  - /api/git/*      (Git Integration)                  │  │
│  └──────────────────────────────────────────────────────┘  │
│  ┌──────────────────────────────────────────────────────┐  │
│  │  Services                                             │  │
│  │  - AugmentService (CLI Integration)                   │  │
│  └──────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
                           │
                           │
        ┌──────────────────┼──────────────────┐
        │                  │                  │
        ▼                  ▼                  ▼
┌──────────────┐  ┌──────────────┐  ┌──────────────┐
│   SQLite     │  │  Augment CLI │  │  Git Repos   │
│   Database   │  │   (auggie)   │  │  (GitPython) │
└──────────────┘  └──────────────┘  └──────────────┘
```

## Technology Stack

### Backend
- **FastAPI**: Modern Python web framework
- **SQLAlchemy 2.0**: ORM for database
- **Pydantic**: Data validation
- **python-jose**: JWT tokens
- **passlib**: Password hashing
- **GitPython**: Git integration
- **WebSockets**: Real-time communication

### Frontend
- **Alpine.js**: Reactive JavaScript framework
- **Tailwind CSS**: Utility-first CSS framework
- **Jinja2**: Template engine

### External
- **Augment CLI**: AI coding assistant
- **SQLite**: Database (default)

## File Structure

```
CodeForge/
├── src/codeforge/
│   ├── models/
│   │   ├── __init__.py
│   │   ├── user.py
│   │   ├── project.py
│   │   ├── conversation.py
│   │   └── message.py
│   ├── routes/
│   │   ├── __init__.py
│   │   ├── auth.py
│   │   ├── projects.py
│   │   ├── chat.py
│   │   ├── files.py
│   │   ├── git.py
│   │   └── pages.py
│   ├── services/
│   │   ├── __init__.py
│   │   └── augment_service.py
│   ├── static/
│   │   ├── css/
│   │   │   └── main.css
│   │   └── js/
│   │       ├── main.js
│   │       └── dashboard.js
│   ├── templates/
│   │   ├── base.html
│   │   ├── login.html
│   │   ├── register.html
│   │   └── dashboard.html
│   ├── __init__.py
│   ├── auth.py
│   ├── config.py
│   ├── database.py
│   └── main.py
├── scripts/
│   ├── server.py
│   └── create_user.py
├── .env.example
├── .gitignore
├── alembic.ini
├── pyproject.toml
├── setup.sh
├── README.md
├── QUICKSTART.md
└── PROJECT_SUMMARY.md
```

## Security Features

1. **JWT Authentication**: Secure token-based auth
2. **Password Hashing**: bcrypt for password storage
3. **Path Validation**: Prevents directory traversal attacks
4. **User Isolation**: Users can only access their own data
5. **Soft Deletes**: Data marked inactive instead of deleted

## Future Enhancements

- [ ] Syntax highlighting in file editor (Monaco Editor)
- [ ] Terminal integration
- [ ] Code search functionality
- [ ] Collaborative editing
- [ ] Custom Augment workflows
- [ ] Project templates
- [ ] Integrated debugging
- [ ] Performance monitoring
- [ ] Multiple file tabs
- [ ] Git commit/push functionality
- [ ] Branch switching
- [ ] Merge conflict resolution
- [ ] Code review features
- [ ] Plugin system

## Getting Started

See **QUICKSTART.md** for a 5-minute setup guide, or **README.md** for comprehensive documentation.

## Development Notes

### Running the Server
```bash
source venv/bin/activate
python scripts/server.py
```

### Creating Users
```bash
python scripts/create_user.py <username> <email> <password>
```

### Database
- SQLite by default (codeforge.db)
- Can be changed to PostgreSQL via DATABASE_URL
- Alembic ready for migrations

### WebSocket Communication
- Client connects to `/api/chat/ws/{conversation_id}`
- Messages sent as JSON
- Responses streamed in chunks
- Automatic reconnection on disconnect

## License

MIT License - See LICENSE file for details

