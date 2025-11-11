# CodeForge Deployment Guide

This guide explains how to deploy CodeForge to production at https://forge-freedom.com/code/

## Port Assignment

CodeForge uses **port 8005** to avoid conflicts with other applications:
- Port 8002: WordForge (writing)
- Port 8003: ArtForge (art_gallery)
- Port 8004: LifeForge (habit-tracker)
- **Port 8005: CodeForge** ← This app
- Port 8006: GreatReads

## Deployment Steps

### 1. Install System Dependencies

```bash
# Install Python 3.11+ if not already installed
sudo apt update
sudo apt install python3.11 python3.11-venv python3-pip

# Install Node.js (for Augment CLI)
curl -fsSL https://deb.nodesource.com/setup_20.x | sudo -E bash -
sudo apt install -y nodejs

# Install Augment CLI globally
sudo npm install -g @augmentcode/cli
```

### 2. Set Up the Application

```bash
# Navigate to the project directory
cd /home/brandon/projects/CodeForge

# Create and activate virtual environment
python3.11 -m venv venv
source venv/bin/activate

# Install Python dependencies
pip install -r requirements.txt

# Create production .env file
cp .env .env.production
```

### 3. Configure Environment Variables

Edit `.env` for production settings:

```bash
# Application Settings
APP_NAME=CodeForge
HOST=127.0.0.1
PORT=8005

# Security
SECRET_KEY=<generate-a-secure-random-key>

# Database
DATABASE_URL=sqlite:///./codeforge.db

# Projects
PROJECTS_ROOT=/home/brandon/projects

# Augment CLI
USE_MOCK_AUGMENT=false
AUGMENT_CLI_PATH=/usr/local/bin/auggie
```

Generate a secure secret key:
```bash
python -c "import secrets; print(secrets.token_urlsafe(32))"
```

### 4. Install Systemd Service

```bash
# Copy the service file to systemd directory
sudo cp codeforge.service /etc/systemd/system/

# Reload systemd to recognize the new service
sudo systemctl daemon-reload

# Enable the service to start on boot
sudo systemctl enable codeforge

# Start the service
sudo systemctl start codeforge

# Check the status
sudo systemctl status codeforge
```

### 5. Configure Nginx

Add the CodeForge configuration to the main nginx config file:

```bash
# Edit the main forge-freedom nginx configuration
sudo nano /etc/nginx/sites-available/forge-freedom.conf
```

Add the following location block inside the `server` block (after the other app configurations):

```nginx
    # CodeForge - AI-Powered Web IDE (port 8005)
    location /code/ {
        # Remove the /code prefix when forwarding to the app
        rewrite ^/code/(.*)$ /$1 break;

        # Forward to the FastAPI application
        proxy_pass http://127.0.0.1:8005;

        # Standard proxy headers
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header X-Forwarded-Host $host;
        proxy_set_header X-Forwarded-Port $server_port;
        proxy_set_header X-Forwarded-Prefix /code;
        
        # WebSocket support (for chat and terminal)
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        
        # Timeout settings (longer for terminal sessions)
        proxy_connect_timeout 60s;
        proxy_send_timeout 300s;
        proxy_read_timeout 300s;
        
        # Buffer settings
        proxy_buffering off;  # Disable buffering for streaming responses
    }

    # Redirect /code to /code/ (with trailing slash)
    location = /code {
        return 301 /code/;
    }
```

Or use the pre-configured file:

```bash
# The configuration is already in config/nginx/codeforge.conf
# Just copy the content into the main nginx config
cat config/nginx/codeforge.conf
```

### 6. Test and Reload Nginx

```bash
# Test nginx configuration
sudo nginx -t

# If the test passes, reload nginx
sudo systemctl reload nginx
```

### 7. Verify Deployment

1. Check that the service is running:
   ```bash
   sudo systemctl status codeforge
   ```

2. Check the logs:
   ```bash
   sudo journalctl -u codeforge -f
   ```

3. Test the application:
   - Open https://forge-freedom.com/code/ in your browser
   - You should see the CodeForge login page
   - Try logging in and creating a project
   - Test the terminal functionality

## Troubleshooting

### Service won't start

```bash
# Check the logs
sudo journalctl -u codeforge -n 50

# Check if port 8005 is already in use
sudo lsof -i :8005

# Restart the service
sudo systemctl restart codeforge
```

### Nginx errors

```bash
# Check nginx error logs
sudo tail -f /var/log/nginx/error.log

# Check nginx access logs
sudo tail -f /var/log/nginx/access.log
```

### WebSocket connection issues

- Make sure nginx is configured with `proxy_http_version 1.1` and the `Upgrade` headers
- Check that the firewall allows WebSocket connections
- Verify that the SSL certificate is valid (WebSockets require HTTPS in production)

### Terminal not working

- Check that the user running the service has permission to access the projects directory
- Verify that `bash` is installed and accessible
- Check the terminal WebSocket logs in the browser console

## Updating the Application

```bash
# Pull the latest changes
cd /home/brandon/projects/CodeForge
git pull

# Activate virtual environment
source venv/bin/activate

# Install any new dependencies
pip install -r requirements.txt

# Restart the service
sudo systemctl restart codeforge

# Check the status
sudo systemctl status codeforge
```

## Authentication Routes

All authentication routes are namespaced under `/code/` to avoid conflicts with other apps:
- Login: https://forge-freedom.com/code/login
- Register: https://forge-freedom.com/code/register
- Dashboard: https://forge-freedom.com/code/dashboard

## Static Files

Static files are served under `/code/static/`:
- CSS: https://forge-freedom.com/code/static/css/
- JavaScript: https://forge-freedom.com/code/static/js/
- Images: https://forge-freedom.com/code/static/images/

## API Endpoints

All API endpoints are under `/code/api/`:
- Auth: `/code/api/auth/`
- Chat: `/code/api/chat/`
- Projects: `/code/api/projects/`
- Files: `/code/api/files/`
- Git: `/code/api/git/`
- Terminal: `/code/api/terminal/`

## Security Considerations

1. **Secret Key**: Make sure to use a strong, randomly generated secret key in production
2. **HTTPS**: All traffic should be over HTTPS (handled by nginx)
3. **File Access**: The application has access to the projects directory - ensure proper permissions
4. **Terminal Access**: The terminal feature provides shell access - restrict user registration if needed
5. **Database**: The SQLite database should be backed up regularly

## Backup

```bash
# Backup the database
cp /home/brandon/projects/CodeForge/codeforge.db /home/brandon/backups/codeforge-$(date +%Y%m%d).db

# Backup the .env file
cp /home/brandon/projects/CodeForge/.env /home/brandon/backups/codeforge-env-$(date +%Y%m%d).txt
```

