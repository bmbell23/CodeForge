# CodeForge

A modern web-based IDE centered around the Augment CLI, providing a beautiful interface for coding with AI assistance.

## Features

- 🤖 **AI-Powered Coding**: Chat with Augment CLI directly from your browser
- 💬 **Real-time Streaming**: See Augment's responses as they're generated
- 📁 **Project Management**: Manage multiple projects from ~/projects/
- 📝 **File Editor**: Browse and edit files with syntax highlighting
- 🔀 **Git Integration**: View git status, history, and diffs
- 🔄 **Multiple Conversations**: Manage multiple chat sessions per project
- 🎨 **Modern UI**: Clean, responsive interface with dark mode support

## Prerequisites

- Docker and Docker Compose (recommended)
- OR Python 3.8+ and Node.js 22+ (for manual installation)
- Augment CLI account (for AI features)

## Quick Start

### Docker Deployment (Recommended)

```bash
# Clone the repository
git clone https://github.com/yourusername/CodeForge.git
cd CodeForge

# Start the container
docker compose up -d

# Create a user
docker exec -it codeforge_app python scripts/create_user.py <username> <email> <password>

# Access at http://localhost:8005
```

**Note**: You'll need to configure Augment authentication. See [docs/GETTING_STARTED.md](docs/GETTING_STARTED.md) for detailed setup instructions.

### Manual Installation

For development or custom deployments:

#### 1. Install Augment CLI

```bash
npm install -g @augmentcode/auggie
auggie login
```

#### 2. Set up CodeForge

```bash
cd CodeForge

# Create virtual environment
python3 -m venv venv
source venv/bin/activate  # On Windows: venv\Scripts\activate

# Install dependencies
pip install -e .

# Copy environment file
cp .env.example .env

# Edit .env and configure your settings
nano .env
```

#### 3. Configure Environment

Edit `.env` and set:

```env
# Projects root directory (where your code projects are)
PROJECTS_ROOT=/home/brandon/projects

# Secret key for JWT tokens (generate a random string)
SECRET_KEY=your-secret-key-here

# Server settings
HOST=0.0.0.0
PORT=8004

# Augment mode (set to false to use real Augment CLI)
USE_MOCK_AUGMENT=false
```

#### 4. Create a User

```bash
python scripts/create_user.py brandon brandon@example.com yourpassword
```

#### 5. Run the Server

```bash
python scripts/server.py
```

Or use the installed command:

```bash
codeforge-server
```

The application will be available at `http://localhost:8004`

## Usage

### First Time Setup

1. Navigate to `http://localhost:8004`
2. Log in with your credentials
3. Click "Add Project" to scan and add projects from your projects directory
4. Select a project from the dropdown
5. Click "New Conversation" to start chatting with Augment

### Chat Interface

- Type your message in the input box at the bottom
- Press Ctrl+Enter to send (Enter for new line)
- Watch as Augment streams its response in real-time
- All conversations are saved and can be resumed later

### File Editor

- Click the "Files" tab to browse your project files
- Click on a file to open it in the editor
- Edit the file and click "Save File" to save changes
- The editor supports basic text editing

### Git Integration

- Click the "Git" tab to view git information
- See current branch, modified files, and status
- View recent commit history
- See diffs for modified files

## Architecture

CodeForge is built with:

- **Backend**: FastAPI + SQLAlchemy + Python
- **Frontend**: Alpine.js + Tailwind CSS
- **Real-time**: WebSockets for streaming chat
- **AI**: Augment CLI integration via subprocess

### Project Structure

```
CodeForge/
├── src/codeforge/       # Main application code
│   ├── models/          # Database models
│   ├── routes/          # API routes
│   ├── services/        # Business logic (Augment integration)
│   ├── static/          # CSS and JavaScript
│   ├── templates/       # HTML templates
│   ├── auth.py          # Authentication
│   ├── config.py        # Configuration
│   ├── database.py      # Database setup
│   └── main.py          # FastAPI app
├── deployment/          # Deployment files
│   ├── docker/          # Docker configuration
│   └── scripts/         # Deployment scripts
├── docs/                # Documentation
├── scripts/             # Utility scripts
├── data/                # Database and uploads
├── logs/                # Application logs
├── pyproject.toml       # Python dependencies
└── README.md
```

## Development

### Running in Development Mode

```bash
# Activate virtual environment
source venv/bin/activate

# Run with auto-reload
python scripts/server.py
```

### Database

CodeForge uses SQLite by default. The database file is created automatically at `codeforge.db`.

To reset the database:

```bash
rm codeforge.db
python scripts/create_user.py <username> <email> <password>
```

## Deployment

### Docker Deployment (Recommended)

See [deployment/README.md](deployment/README.md) for detailed Docker deployment instructions.

Quick start:
```bash
docker compose up -d
```

### Manual Deployment

For production deployment without Docker:

1. Set a strong `SECRET_KEY` in `.env`
2. Use a production WSGI server (e.g., Gunicorn)
3. Set up nginx as a reverse proxy
4. Configure SSL/TLS certificates
5. Set `DATABASE_URL` to use PostgreSQL instead of SQLite

Example systemd service file is provided at `codeforge.service`.

## Troubleshooting

### Augment CLI Not Found

Make sure `auggie` is installed and in your PATH:

```bash
which auggie
auggie --version
```

### WebSocket Connection Failed

Check that:
- The server is running
- No firewall is blocking the connection
- The WebSocket URL is correct (ws:// for HTTP, wss:// for HTTPS)

### Projects Not Showing

Verify:
- `PROJECTS_ROOT` in `.env` points to the correct directory
- The directory exists and is readable
- Projects are subdirectories of `PROJECTS_ROOT`

## Future Enhancements

- [ ] Syntax highlighting in file editor
- [ ] Terminal integration
- [ ] Code search functionality
- [ ] Collaborative editing
- [ ] Custom Augment commands/workflows
- [ ] Project templates
- [ ] Integrated debugging
- [ ] Performance monitoring

## License

MIT License - See LICENSE file for details

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

