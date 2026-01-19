# AI Agent Guidelines for CodeForge

## 🚨 CRITICAL SYSTEM OPERATION RULES 🚨

### FORBIDDEN OPERATIONS - NEVER DO THESE

These operations have caused catastrophic data loss in the past. **NEVER** perform these without explicit user permission:

#### ⛔ System-Level Operations
- `sudo reboot` - NEVER restart the entire system
- `systemctl reboot` - NEVER restart the entire system
- `shutdown` - NEVER shutdown the system
- `poweroff` - NEVER power off the system
- `init 6` - NEVER restart via init

#### ⛔ Database Operations
- `pg_resetwal` - NEVER use PostgreSQL recovery tools without verified backups
- `DROP DATABASE` - NEVER drop databases without explicit permission
- Direct database file deletion - NEVER delete database files
- `TRUNCATE` on production tables - NEVER truncate without permission

#### ⛔ Container Operations
- `docker-compose down` on production - NEVER stop all services at once
- `docker stop $(docker ps -aq)` - NEVER stop all containers
- `docker system prune -a` - NEVER prune without permission
- Deleting volumes without backup - NEVER delete data volumes

### ✅ CORRECT PROCEDURES

#### Restarting Services

**NOTE: This server has Docker permission issues. `docker restart` and `docker-compose restart` WILL FAIL with "permission denied".**

**WORKING METHOD:**
```bash
# Step 1: Find the container PID
PID=$(docker inspect <container_name> --format '{{.State.Pid}}')

# Step 2: Kill the process
kill $PID

# Step 3: Bring it back up
cd /path/to/project
docker-compose up -d

# Example for CodeForge:
PID=$(docker inspect da7d9e211ea5_codeforge_app --format '{{.State.Pid}}')
kill $PID
cd /home/brandon/projects/CodeForge
docker-compose up -d
```

**METHODS THAT DON'T WORK ON THIS SERVER:**
```bash
# These FAIL with "permission denied":
docker restart <container>          # ❌ FAILS
docker-compose restart <service>    # ❌ FAILS
sudo docker restart <container>     # ❌ FAILS
docker-compose down                 # ❌ FAILS (can't stop)

# NEVER do this:
sudo reboot                         # ❌ CATASTROPHIC
```

#### Handling Database Issues
```bash
# CORRECT: Check logs first
docker logs <db_container>

# CORRECT: Restart database container
docker restart <db_container>

# CORRECT: Restore from backup
docker exec <db_container> pg_restore ...

# WRONG: Don't do this
pg_resetwal
DROP DATABASE
```

#### Disk Space Issues
```bash
# CORRECT: Clean logs
find /var/log -name "*.log" -mtime +30 -delete

# CORRECT: Clean docker
docker system df
docker image prune

# WRONG: Don't do this
sudo reboot  # This doesn't free disk space!
```

### 🔍 Diagnostic Steps Before Any Action

1. **Check logs**: `docker logs <container>`
2. **Check resources**: `df -h`, `free -h`, `docker stats`
3. **Check status**: `docker ps -a`, `systemctl status <service>`
4. **Identify root cause**: Don't guess, investigate
5. **Ask user**: If unsure, always ask before proceeding

### 📝 Incident Report: What Went Wrong

**Date**: January 7, 2026

**What happened**:
- AI agent ran `sudo reboot` to fix a container issue
- System reboot caused improper container shutdown
- PostgreSQL database corrupted during shutdown
- AI agent then used `pg_resetwal` which wiped the database
- All user data (117,909 photos metadata) was lost
- Only recovered due to automated backups

**Root cause**:
- Actual issue was disk space (logs filled up)
- Simple `rm` of log files would have fixed it
- Reboot was unnecessary and destructive

**Lesson**:
- Always diagnose before acting
- Never reboot production systems
- Always verify backups before destructive operations
- Restart individual services, not entire systems

## Project Guidelines

### Development Workflow
1. Make changes in feature branches
2. Test locally before committing
3. Use docker-compose for local development
4. Keep production and dev environments separate

### Code Standards
- Python 3.8+ with type hints
- FastAPI for REST API
- SQLAlchemy for database ORM
- Pydantic for data validation
- Follow PEP 8 style guide

### Docker Usage
- Use `docker-compose.dev.yml` for development
- Use `docker-compose.yml` for production
- Never mix dev and prod databases
- Always use volumes for persistent data

### Database Management
- SQLite for development
- PostgreSQL for production (if migrated)
- Always backup before migrations
- Use Alembic for schema changes
- Never edit database files directly

