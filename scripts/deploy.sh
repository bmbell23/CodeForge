#!/bin/bash
# CodeForge Deployment Script

set -e  # Exit on error

echo "🚀 CodeForge Deployment Script"
echo "================================"
echo ""

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Get the script directory
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

echo "📁 Project directory: $PROJECT_DIR"
echo ""

# Check if running as root
if [ "$EUID" -eq 0 ]; then 
    echo -e "${RED}❌ Please do not run this script as root${NC}"
    exit 1
fi

# Step 1: Check dependencies
echo "1️⃣  Checking dependencies..."
if ! command -v python3.11 &> /dev/null; then
    echo -e "${RED}❌ Python 3.11 not found. Please install it first.${NC}"
    exit 1
fi

if ! command -v auggie &> /dev/null; then
    echo -e "${YELLOW}⚠️  Augment CLI (auggie) not found.${NC}"
    echo -e "${YELLOW}    You can install it later with: npm install -g @augmentcode/cli${NC}"
    echo -e "${YELLOW}    Or set USE_MOCK_AUGMENT=true in .env to use mock mode${NC}"
fi

echo -e "${GREEN}✅ Dependencies OK${NC}"
echo ""

# Step 2: Set up virtual environment
echo "2️⃣  Setting up virtual environment..."
cd "$PROJECT_DIR"

if [ ! -d "venv" ]; then
    echo "Creating virtual environment..."
    python3.11 -m venv venv
fi

source venv/bin/activate

echo "Installing Python dependencies..."
pip install -q --upgrade pip
pip install -q -r requirements.txt

echo -e "${GREEN}✅ Virtual environment ready${NC}"
echo ""

# Step 3: Check .env file
echo "3️⃣  Checking environment configuration..."
if [ ! -f ".env" ]; then
    echo -e "${YELLOW}⚠️  .env file not found. Creating from template...${NC}"
    cat > .env << EOF
# Application Settings
APP_NAME=CodeForge
HOST=127.0.0.1
PORT=8005

# Security
SECRET_KEY=$(python -c "import secrets; print(secrets.token_urlsafe(32))")

# Database
DATABASE_URL=sqlite:///./codeforge.db

# Projects
PROJECTS_ROOT=/home/brandon/projects

# Augment CLI
USE_MOCK_AUGMENT=true
AUGMENT_CLI_PATH=/usr/local/bin/auggie
EOF
    echo -e "${GREEN}✅ Created .env file with generated secret key${NC}"
else
    echo -e "${GREEN}✅ .env file exists${NC}"
fi
echo ""

# Step 4: Install systemd service
echo "4️⃣  Installing systemd service..."
if [ -f "codeforge.service" ]; then
    echo "Copying service file to /etc/systemd/system/..."
    sudo cp codeforge.service /etc/systemd/system/
    
    echo "Reloading systemd daemon..."
    sudo systemctl daemon-reload
    
    echo "Enabling service..."
    sudo systemctl enable codeforge
    
    echo -e "${GREEN}✅ Systemd service installed${NC}"
else
    echo -e "${RED}❌ codeforge.service file not found${NC}"
    exit 1
fi
echo ""

# Step 5: Start/Restart the service
echo "5️⃣  Starting CodeForge service..."
if systemctl is-active --quiet codeforge; then
    echo "Service is running. Restarting..."
    sudo systemctl restart codeforge
else
    echo "Starting service..."
    sudo systemctl start codeforge
fi

# Wait a moment for the service to start
sleep 2

# Check if service started successfully
if systemctl is-active --quiet codeforge; then
    echo -e "${GREEN}✅ Service started successfully${NC}"
else
    echo -e "${RED}❌ Service failed to start. Check logs with: sudo journalctl -u codeforge -n 50${NC}"
    exit 1
fi
echo ""

# Step 6: Configure nginx
echo "6️⃣  Nginx configuration..."
echo -e "${YELLOW}⚠️  Manual step required:${NC}"
echo ""
echo "Add the following to /etc/nginx/sites-available/forge-freedom.conf:"
echo ""
cat config/nginx/codeforge.conf
echo ""
echo "Then run:"
echo "  sudo nginx -t"
echo "  sudo systemctl reload nginx"
echo ""

# Step 7: Show status
echo "7️⃣  Service status:"
sudo systemctl status codeforge --no-pager -l
echo ""

# Step 8: Show logs
echo "8️⃣  Recent logs:"
sudo journalctl -u codeforge -n 20 --no-pager
echo ""

# Final summary
echo "================================"
echo -e "${GREEN}✅ Deployment complete!${NC}"
echo ""
echo "📝 Next steps:"
echo "  1. Add the nginx configuration (see step 6 above)"
echo "  2. Test nginx: sudo nginx -t"
echo "  3. Reload nginx: sudo systemctl reload nginx"
echo "  4. Visit: https://forge-freedom.com/code/"
echo ""
echo "📊 Useful commands:"
echo "  - View logs: sudo journalctl -u codeforge -f"
echo "  - Restart: sudo systemctl restart codeforge"
echo "  - Stop: sudo systemctl stop codeforge"
echo "  - Status: sudo systemctl status codeforge"
echo ""

