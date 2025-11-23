# CodeForge Deployment

This directory contains all deployment-related files for CodeForge.

## Directory Structure

```
deployment/
├── docker/          # Docker configuration files
│   ├── Dockerfile
│   ├── docker-compose.yml
│   ├── docker-compose.template.yml
│   ├── Dockerfile.template
│   ├── docker-setup.sh
│   ├── forge-stack.docker-compose.yml
│   ├── nginx-docker.conf
│   └── traefik.yml
└── scripts/         # Deployment and setup scripts
    ├── setup.sh
    ├── forge-stack-setup.sh
    ├── migrate-forge-project.sh
    ├── serve-debug.sh
    └── test-codeforge.sh
```

## Quick Start

### Docker Deployment (Recommended)

1. Make sure Docker and Docker Compose are installed
2. Run from the project root:
   ```bash
   docker compose up -d
   ```

### Manual Deployment

1. Run the setup script:
   ```bash
   ./deployment/scripts/setup.sh
   ```

## Configuration

The main Docker configuration is in `deployment/docker/docker-compose.yml`. Key environment variables:

- `AUGGIE_PATH`: Path to auggie CLI (default: `/usr/bin/auggie`)
- `PROJECTS_ROOT`: Root directory for projects (default: `/app/projects`)
- `SECRET_KEY`: Secret key for JWT tokens (change in production!)
- `AUGMENT_SESSION_AUTH`: Augment authentication credentials (JSON format)

## Notes

- The `Dockerfile` and `docker-compose.yml` in the project root are symlinks to the files in this directory
- Projects are mounted read-only from the host at `/home/brandon/projects`
- Database and logs are persisted in `./data` and `./logs` directories

