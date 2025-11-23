# Getting Started with CodeForge

This guide will help you get CodeForge up and running quickly.

## Prerequisites

- Docker and Docker Compose (recommended)
- OR Python 3.8+ and Node.js 22+ (for manual installation)
- Augment CLI account (for AI features)

## Quick Start with Docker (Recommended)

### 1. Start the Container

```bash
cd CodeForge
docker compose up -d
```

### 2. Configure Augment Authentication

You need to provide your Augment credentials to the container. Edit `deployment/docker/docker-compose.yml` and update the `AUGMENT_SESSION_AUTH` environment variable with your credentials.

To get your credentials:
```bash
cat ~/.augment/session.json
```

Copy the JSON content and set it in the `AUGMENT_SESSION_AUTH` variable in `docker-compose.yml`.

### 3. Create a User

```bash
docker exec -it codeforge_app python scripts/create_user.py <username> <email> <password>
```

### 4. Access the Application

Open your browser to: `http://localhost:8005`

## Manual Installation

### 1. Install Augment CLI

```bash
npm install -g @augmentcode/auggie
auggie login
```

### 2. Set Up Python Environment

```bash
cd CodeForge

# Create virtual environment
python3 -m venv venv
source venv/bin/activate

# Install dependencies
pip install -e .
```

### 3. Configure Environment

```bash
cp .env.example .env
nano .env
```

Set these variables:
```env
PROJECTS_ROOT=/home/brandon/projects
SECRET_KEY=<generate-a-random-string>
HOST=0.0.0.0
PORT=8004
USE_MOCK_AUGMENT=false
```

### 4. Create a User

```bash
python scripts/create_user.py <username> <email> <password>
```

### 5. Run the Server

```bash
python scripts/server.py
```

Access at: `http://localhost:8004`

## First Steps

1. **Login** with your credentials
2. **Select a project** from the dropdown (projects are auto-scanned from your projects directory)
3. **Start a conversation** by typing a message in the chat
4. **Explore tabs**: Chat, Files, Git, Terminal

## Troubleshooting

### Projects not showing
- Check that `PROJECTS_ROOT` points to the correct directory
- Verify the directory contains subdirectories (your projects)

### Augment authentication errors
- For Docker: Ensure `AUGMENT_SESSION_AUTH` is set correctly in `docker-compose.yml`
- For manual: Run `auggie login` to authenticate

### WebSocket errors
- Make sure you've selected a project and conversation
- Check browser console for specific error messages

## Next Steps

- See [deployment/README.md](../deployment/README.md) for deployment options
- See main [README.md](../README.md) for architecture and features

