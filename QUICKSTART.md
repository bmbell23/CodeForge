# CodeForge Quick Start Guide

Get up and running with CodeForge in 5 minutes!

## Prerequisites Check

```bash
# Check Python version (need 3.8+)
python3 --version

# Check Node.js version (need 22+)
node --version

# Check if auggie is installed
which auggie
```

## Installation Steps

### 1. Install Augment CLI (if not already installed)

```bash
npm install -g @augmentcode/auggie
auggie login
```

### 2. Set Up CodeForge

```bash
cd ~/projects/CodeForge

# Create and activate virtual environment
python3 -m venv venv
source venv/bin/activate

# Install CodeForge
pip install -e .
```

### 3. Configure

```bash
# Copy environment template
cp .env.example .env

# Edit configuration (optional - defaults should work)
# nano .env
```

### 4. Create Your User

```bash
# Create a user account
python scripts/create_user.py brandon brandon@example.com mypassword
```

### 5. Start the Server

```bash
# Run the development server
python scripts/server.py
```

## First Use

1. Open your browser to `http://localhost:8004`
2. Log in with your credentials
3. Click "Add Project" button
4. Select projects to add from the list
5. Choose a project from the dropdown
6. Click "New Conversation"
7. Start chatting with Augment!

## Quick Tips

- **Send Message**: Type and press Enter
- **New Line**: Shift + Enter
- **Switch Projects**: Use the dropdown in the top nav
- **View Files**: Click the "Files" tab
- **Check Git**: Click the "Git" tab
- **Multiple Chats**: Create multiple conversations per project

## Common Commands

```bash
# Start server
python scripts/server.py

# Create new user
python scripts/create_user.py <username> <email> <password>

# Create admin user
python scripts/create_user.py admin admin@example.com password --admin

# Reset database (WARNING: deletes all data)
rm codeforge.db
python scripts/create_user.py brandon brandon@example.com mypassword
```

## Troubleshooting

**Can't connect to server?**
- Check if server is running: `ps aux | grep codeforge`
- Check port 8004 is not in use: `lsof -i :8004`

**Augment not responding?**
- Verify auggie is installed: `auggie --version`
- Check you're logged in: `auggie login`
- Ensure project path is correct in settings

**Projects not showing?**
- Verify PROJECTS_ROOT in .env points to ~/projects
- Check directory permissions
- Ensure projects are subdirectories

## Next Steps

- Explore the file editor
- Check out git integration
- Create multiple conversations
- Try different projects

Enjoy coding with CodeForge! 🚀

