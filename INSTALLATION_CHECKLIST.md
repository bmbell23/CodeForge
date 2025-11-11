# CodeForge Installation Checklist

Use this checklist to ensure CodeForge is properly installed and configured.

## Prerequisites

- [ ] Python 3.8+ installed (`python3 --version`)
- [ ] Node.js 22+ installed (`node --version`)
- [ ] npm installed (`npm --version`)
- [ ] Git installed (`git --version`)
- [ ] ~/projects directory exists

## Installation Steps

### 1. Augment CLI Setup
- [ ] Install Augment CLI: `npm install -g @augmentcode/auggie`
- [ ] Verify installation: `auggie --version`
- [ ] Login to Augment: `auggie login`
- [ ] Test Augment: `auggie "hello"`

### 2. CodeForge Setup
- [ ] Navigate to CodeForge directory: `cd ~/projects/CodeForge`
- [ ] Run setup script: `./setup.sh`
  - OR manually:
    - [ ] Create virtual environment: `python3 -m venv venv`
    - [ ] Activate venv: `source venv/bin/activate`
    - [ ] Install dependencies: `pip install -e .`
    - [ ] Copy .env: `cp .env.example .env`

### 3. Configuration
- [ ] Edit `.env` file
- [ ] Set `PROJECTS_ROOT` to your projects directory
- [ ] Set `SECRET_KEY` to a random string (for production)
- [ ] Verify `PORT` is available (default: 8004)
- [ ] Verify `AUGGIE_PATH` is correct (default: `auggie`)

### 4. Database Setup
- [ ] Database will be created automatically on first run
- [ ] Create first user: `python scripts/create_user.py <username> <email> <password>`
- [ ] Verify user created successfully

### 5. Start Server
- [ ] Activate venv: `source venv/bin/activate`
- [ ] Start server: `python scripts/server.py`
- [ ] Verify server starts without errors
- [ ] Check server is listening on configured port

### 6. Browser Access
- [ ] Open browser to `http://localhost:8004`
- [ ] Verify login page loads
- [ ] Login with created credentials
- [ ] Verify redirect to dashboard

## Feature Testing

### Authentication
- [ ] Can register new user
- [ ] Can login with credentials
- [ ] Can logout
- [ ] Invalid credentials show error
- [ ] Token persists across page refreshes

### Project Management
- [ ] Click "Add Project" button
- [ ] Projects from ~/projects are listed
- [ ] Can add a project
- [ ] Project appears in dropdown
- [ ] Can switch between projects

### Chat Interface
- [ ] Click "New Conversation"
- [ ] Conversation appears in sidebar
- [ ] Can type message in input box
- [ ] Can send message (Enter key)
- [ ] Message appears in chat
- [ ] Augment response streams in real-time
- [ ] Response completes and is saved
- [ ] Can send multiple messages
- [ ] Can switch between conversations
- [ ] Conversation history persists

### File Editor
- [ ] Click "Files" tab
- [ ] File tree loads for selected project
- [ ] Can click on a file
- [ ] File content loads in editor
- [ ] Can edit file content
- [ ] Can save file
- [ ] Changes persist after save

### Git Integration
- [ ] Click "Git" tab
- [ ] Git status shows current branch
- [ ] Modified files are listed
- [ ] Commit history displays
- [ ] Commit details are correct

## Troubleshooting Checks

### Server Won't Start
- [ ] Check Python version is 3.8+
- [ ] Check virtual environment is activated
- [ ] Check all dependencies installed: `pip list`
- [ ] Check port 8004 is not in use: `lsof -i :8004`
- [ ] Check .env file exists and is valid
- [ ] Check database permissions

### Augment Not Responding
- [ ] Check auggie is installed: `which auggie`
- [ ] Check auggie version: `auggie --version`
- [ ] Check auggie login: `auggie login`
- [ ] Test auggie directly: `auggie "test"`
- [ ] Check AUGGIE_PATH in .env
- [ ] Check project path is correct

### WebSocket Connection Failed
- [ ] Check browser console for errors
- [ ] Check server logs for WebSocket errors
- [ ] Verify conversation ID is valid
- [ ] Check firewall settings
- [ ] Try different browser

### Projects Not Loading
- [ ] Check PROJECTS_ROOT in .env
- [ ] Verify directory exists: `ls -la ~/projects`
- [ ] Check directory permissions
- [ ] Verify projects are subdirectories
- [ ] Check server logs for errors

### Files Not Loading
- [ ] Check project is selected
- [ ] Verify project path is correct
- [ ] Check file permissions
- [ ] Verify project is a git repository (for git features)
- [ ] Check browser console for errors

### Git Features Not Working
- [ ] Verify project is a git repository: `cd <project> && git status`
- [ ] Check GitPython is installed: `pip show GitPython`
- [ ] Verify git is installed: `git --version`
- [ ] Check project path is correct

## Performance Checks

- [ ] Page loads in < 2 seconds
- [ ] Chat messages send instantly
- [ ] Augment responses stream smoothly
- [ ] File tree loads quickly
- [ ] File editor is responsive
- [ ] No console errors in browser
- [ ] No errors in server logs

## Security Checks

- [ ] SECRET_KEY is changed from default
- [ ] Database file has correct permissions
- [ ] Can't access other users' data
- [ ] Can't access files outside project directory
- [ ] Passwords are hashed in database
- [ ] JWT tokens expire correctly

## Production Readiness (Optional)

- [ ] Use PostgreSQL instead of SQLite
- [ ] Set strong SECRET_KEY
- [ ] Configure nginx reverse proxy
- [ ] Set up SSL/TLS certificates
- [ ] Configure systemd service
- [ ] Set up log rotation
- [ ] Configure backups
- [ ] Set up monitoring
- [ ] Configure firewall rules
- [ ] Use production WSGI server (Gunicorn)

## Final Verification

- [ ] All features work as expected
- [ ] No errors in browser console
- [ ] No errors in server logs
- [ ] Performance is acceptable
- [ ] Security measures in place
- [ ] Documentation is clear
- [ ] Ready to use!

## Notes

Use this space to record any issues or customizations:

```
Date: _______________
Issues found:


Resolutions:


Custom configuration:


```

## Support

If you encounter issues:

1. Check the troubleshooting section above
2. Review server logs for errors
3. Check browser console for errors
4. Verify all prerequisites are met
5. Review README.md and QUICKSTART.md
6. Check Augment CLI documentation

## Success!

If all checks pass, CodeForge is ready to use! 🎉

Start coding with AI assistance:
1. Select a project
2. Create a new conversation
3. Ask Augment to help you code
4. Edit files as needed
5. Check git status
6. Enjoy your new web-based IDE!

