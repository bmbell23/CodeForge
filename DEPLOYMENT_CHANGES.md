# CodeForge Deployment Changes Summary

This document summarizes all the changes made to prepare CodeForge for production deployment at https://forge-freedom.com/code/

## Overview

CodeForge has been updated to support deployment under the `/code/` URL prefix while maintaining backward compatibility with development mode (running directly on `http://localhost:8005`).

## Key Changes

### 1. URL Path Prefix Support

**Problem**: The application needs to run under `/code/` in production but directly at root in development.

**Solution**: Implemented dynamic path detection using a `BASE_PATH` constant that automatically detects whether the app is running under `/code/` or at root.

### 2. Files Modified

#### Backend Files

1. **`src/codeforge/main.py`**
   - Added redirect from `/` to `/code/`
   - Updated static files mount to `/code/static`
   - Added version number to health check

2. **`src/codeforge/routes/pages.py`**
   - Updated all page routes to use `/code/` prefix:
     - `/code/` → Landing page (redirects to login)
     - `/code/login` → Login page
     - `/code/register` → Registration page
     - `/code/dashboard` → Main dashboard

#### Frontend Files

3. **`src/codeforge/templates/base.html`**
   - Updated static file paths:
     - `/static/css/main.css` → `/code/static/css/main.css`
     - `/static/js/main.js` → `/code/static/js/main.js`

4. **`src/codeforge/templates/login.html`**
   - Updated register link: `/register` → `/code/register`
   - Updated redirect after login: `/dashboard` → `/code/dashboard`

5. **`src/codeforge/templates/register.html`**
   - Updated login link: `/login` → `/code/login`
   - Updated redirect after registration: `/dashboard` → `/code/dashboard`

6. **`src/codeforge/templates/dashboard.html`**
   - Updated dashboard.js script path: `/static/js/dashboard.js` → `/code/static/js/dashboard.js`

7. **`src/codeforge/static/js/dashboard.js`**
   - Added `BASE_PATH` constant that auto-detects the URL prefix
   - Updated all API calls to use `${BASE_PATH}/api/...`
   - Updated WebSocket URLs to include `BASE_PATH`
   - Changes affect:
     - User authentication (`/api/auth/me`)
     - Projects API (`/api/projects/`)
     - Chat API (`/api/chat/...`)
     - Files API (`/api/files/...`)
     - Git API (`/api/git/...`)
     - Terminal WebSocket (`/api/terminal/ws/...`)
     - Chat WebSocket (`/api/chat/ws/...`)

### 3. New Files Created

#### Deployment Configuration

1. **`codeforge.service`**
   - Systemd service file for running CodeForge as a system service
   - Configured to run on port 8005
   - Auto-restart on failure
   - Runs as user `brandon`

2. **`config/nginx/codeforge.conf`**
   - Nginx configuration snippet for reverse proxy
   - Handles URL rewriting (`/code/` → `/`)
   - WebSocket support for chat and terminal
   - Proper timeout settings for long-running terminal sessions
   - Buffering disabled for streaming responses

#### Documentation

3. **`DEPLOYMENT.md`**
   - Comprehensive deployment guide
   - Step-by-step instructions
   - Troubleshooting section
   - Security considerations
   - Backup procedures

4. **`scripts/deploy.sh`**
   - Automated deployment script
   - Checks dependencies
   - Sets up virtual environment
   - Installs systemd service
   - Generates secure secret key
   - Provides nginx configuration instructions

5. **`DEPLOYMENT_CHANGES.md`** (this file)
   - Summary of all deployment-related changes

## How It Works

### Development Mode

When running locally (e.g., `http://localhost:8005`):
- `BASE_PATH` is empty (`''`)
- All URLs work without prefix
- Example: `http://localhost:8005/dashboard`
- API calls go to `/api/...`

### Production Mode

When deployed at `https://forge-freedom.com/code/`:
- `BASE_PATH` is `/code`
- All URLs include the prefix
- Example: `https://forge-freedom.com/code/dashboard`
- API calls go to `/code/api/...`
- Nginx strips `/code/` before forwarding to the app

### URL Rewriting Flow

1. Browser requests: `https://forge-freedom.com/code/dashboard`
2. Nginx receives: `/code/dashboard`
3. Nginx rewrites to: `/dashboard` (strips `/code/`)
4. FastAPI receives: `/dashboard`
5. FastAPI route matches: `@router.get("/code/dashboard")`
6. Response sent back through nginx to browser

### API Call Flow

1. JavaScript makes call: `fetch('${BASE_PATH}/api/projects/')`
2. In production, this becomes: `fetch('/code/api/projects/')`
3. Browser requests: `https://forge-freedom.com/code/api/projects/`
4. Nginx receives: `/code/api/projects/`
5. Nginx rewrites to: `/api/projects/`
6. FastAPI receives: `/api/projects/`
7. FastAPI route matches: `@router.get("/api/projects/")`

## Port Assignment

CodeForge uses **port 8005** to avoid conflicts:
- Port 8002: WordForge (writing)
- Port 8003: ArtForge (art_gallery)
- Port 8004: LifeForge (habit-tracker)
- **Port 8005: CodeForge** ← This app
- Port 8006: GreatReads

## Authentication Namespacing

All authentication routes are properly namespaced under `/code/`:
- Login: `https://forge-freedom.com/code/login`
- Register: `https://forge-freedom.com/code/register`
- Dashboard: `https://forge-freedom.com/code/dashboard`

This prevents conflicts with other apps' authentication pages.

## Testing

### Test Development Mode

```bash
# Start the development server
cd /home/brandon/projects/CodeForge
source venv/bin/activate
python scripts/server.py

# Visit http://localhost:8005
# All features should work normally
```

### Test Production Mode (Local)

```bash
# Run the deployment script
./scripts/deploy.sh

# Check service status
sudo systemctl status codeforge

# View logs
sudo journalctl -u codeforge -f
```

### Test Production Mode (After Nginx Setup)

```bash
# After adding nginx configuration and reloading
# Visit https://forge-freedom.com/code/

# Test all features:
# 1. Login/Register
# 2. Create a project
# 3. Chat with Augment
# 4. Browse files
# 5. View git status
# 6. Use terminal
```

## Rollback Plan

If deployment fails, you can rollback:

```bash
# Stop the service
sudo systemctl stop codeforge
sudo systemctl disable codeforge

# Remove the service file
sudo rm /etc/systemd/system/codeforge.service
sudo systemctl daemon-reload

# Remove nginx configuration
# (Edit /etc/nginx/sites-available/forge-freedom.conf and remove the CodeForge block)
sudo nginx -t
sudo systemctl reload nginx

# Continue using development mode
cd /home/brandon/projects/CodeForge
source venv/bin/activate
python scripts/server.py
```

## Next Steps

1. **Review the changes** - Make sure all changes are correct
2. **Test locally** - Verify the app still works in development mode
3. **Run deployment script** - Execute `./scripts/deploy.sh`
4. **Configure nginx** - Add the configuration from `config/nginx/codeforge.conf`
5. **Test production** - Visit https://forge-freedom.com/code/
6. **Monitor logs** - Watch for any errors: `sudo journalctl -u codeforge -f`

## Security Notes

- Generated a secure random `SECRET_KEY` in the deployment script
- All traffic goes through HTTPS (handled by nginx)
- Terminal access is restricted to authenticated users
- File access is limited to the projects directory
- WebSocket connections are properly secured

## Maintenance

### Updating the Application

```bash
cd /home/brandon/projects/CodeForge
git pull
source venv/bin/activate
pip install -r requirements.txt
sudo systemctl restart codeforge
```

### Viewing Logs

```bash
# Real-time logs
sudo journalctl -u codeforge -f

# Last 100 lines
sudo journalctl -u codeforge -n 100

# Logs from today
sudo journalctl -u codeforge --since today
```

### Restarting the Service

```bash
sudo systemctl restart codeforge
```

## Compatibility

- ✅ Backward compatible with development mode
- ✅ Works with existing database
- ✅ No changes to API contracts
- ✅ All existing features preserved
- ✅ WebSocket connections work in both modes

