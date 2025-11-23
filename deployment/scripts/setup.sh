#!/bin/bash
# Setup script for CodeForge

set -e

echo "🔨 CodeForge Setup Script"
echo "=========================="
echo ""

# Check Python version
echo "Checking Python version..."
python3 --version || { echo "❌ Python 3 not found!"; exit 1; }

# Check Node.js version (optional for mock mode)
echo "Checking Node.js version..."
if command -v node &> /dev/null; then
    node --version
    echo "✅ Node.js found"

    # Check if auggie is installed
    echo "Checking Augment CLI..."
    if ! command -v auggie &> /dev/null; then
        echo "⚠️  Augment CLI not found"
        echo "   You can install it later with: npm install -g @augmentcode/auggie"
        echo "   For now, CodeForge will use mock mode (USE_MOCK_AUGMENT=true)"
    else
        echo "✅ Augment CLI found"
    fi
else
    echo "⚠️  Node.js not found (optional)"
    echo "   CodeForge will run in mock mode (USE_MOCK_AUGMENT=true)"
    echo "   Install Node.js 22+ if you want to use real Augment CLI later"
fi

# Create virtual environment
echo ""
echo "Creating virtual environment..."
if [ ! -d "venv" ]; then
    python3 -m venv venv
    echo "✅ Virtual environment created"
else
    echo "✅ Virtual environment already exists"
fi

# Activate virtual environment
echo "Activating virtual environment..."
source venv/bin/activate

# Install dependencies
echo ""
echo "Installing Python dependencies..."
pip install --upgrade pip
pip install -e .
echo "✅ Dependencies installed"

# Copy .env file if it doesn't exist
echo ""
if [ ! -f ".env" ]; then
    echo "Creating .env file..."
    cp .env.example .env
    echo "✅ .env file created"
    echo "⚠️  Please edit .env and set your configuration"
else
    echo "✅ .env file already exists"
fi

# Create directories
echo ""
echo "Creating directories..."
mkdir -p src/codeforge/static/css
mkdir -p src/codeforge/static/js
mkdir -p src/codeforge/templates
echo "✅ Directories created"

echo ""
echo "=========================="
echo "✅ Setup complete!"
echo ""
echo "Next steps:"
echo "1. Edit .env and configure your settings"
echo "2. Create a user: python scripts/create_user.py <username> <email> <password>"
echo "3. Start the server: python scripts/server.py"
echo "4. Open http://localhost:8004 in your browser"
echo ""
echo "For more information, see README.md or QUICKSTART.md"

