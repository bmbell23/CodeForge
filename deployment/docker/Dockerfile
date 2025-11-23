# CodeForge Dockerfile
FROM python:3.11-slim

# Set environment variables
ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    PYTHONPATH=/app/src

# Install system dependencies
RUN apt-get update && apt-get install -y \
    curl \
    git \
    bash \
    && rm -rf /var/lib/apt/lists/*

# Install Node.js (for Augment CLI)
RUN curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
    && apt-get install -y nodejs

# Install Augment CLI globally
RUN npm install -g @augmentcode/auggie

# Create app directory
WORKDIR /app

# Copy Python requirements first
COPY requirements.txt ./
RUN pip install --no-cache-dir -r requirements.txt

# Fix bcrypt compatibility issues
RUN pip install --no-cache-dir --upgrade bcrypt==4.0.1

# Copy application code
COPY . .

# Install the application in editable mode
RUN pip install --no-cache-dir -e .

# Create necessary directories
RUN mkdir -p /app/data /app/projects

# Create non-root user
RUN useradd -m -u 1000 codeforge && \
    chown -R codeforge:codeforge /app
USER codeforge

# Expose port
EXPOSE 8000

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8000/health || exit 1

# Run the application
CMD ["python", "scripts/server.py"]
