# Claude AI Assistant Guidelines

## ⚠️ CRITICAL SAFETY RULES ⚠️

### NEVER DO THESE OPERATIONS

**System Operations:**
- ❌ `sudo reboot` or any system restart command
- ❌ `shutdown`, `poweroff`, `systemctl reboot`
- ❌ Any operation that restarts the entire server

**Database Operations:**
- ❌ `pg_resetwal` without verified backups
- ❌ `DROP DATABASE` without explicit permission
- ❌ Direct deletion of database files
- ❌ `TRUNCATE` on production tables

**Container Operations:**
- ❌ `docker-compose down` on production
- ❌ Stopping all containers at once
- ❌ `docker system prune -a` without permission
- ❌ Deleting volumes without backups

### ALWAYS DO THIS INSTEAD

**NOTE: This server has Docker permission issues. `docker restart` and `docker-compose restart` WILL FAIL with "permission denied".**

**For Service Issues:**
```bash
# 1. Diagnose first
docker logs <container>
df -h
free -h

# 2. Restart individual service (WORKING METHOD)
PID=$(docker inspect <container> --format '{{.State.Pid}}')
kill $PID
cd /path/to/project
docker-compose up -d
```

**For Database Issues:**
```bash
# 1. Check logs
docker logs <db_container>

# 2. Restart container
docker restart <db_container>

# 3. Restore from backup if needed
# (with user permission)
```

**For Disk Space:**
```bash
# Clean logs
find /var/log -name "*.log" -mtime +30 -delete

# Clean docker images
docker image prune

# NOT: sudo reboot (doesn't help!)
```

## Why These Rules Exist

On January 7, 2026, an AI agent caused catastrophic data loss by:
1. Running `sudo reboot` to fix a container issue (actual issue: disk space)
2. Improper shutdown corrupted PostgreSQL database
3. Using `pg_resetwal` which wiped all data
4. 117,909 photos metadata lost (recovered from backup)

**The fix was simple**: Delete log files to free disk space. No reboot needed.

## General Guidelines

- Always ask before destructive operations
- Diagnose root cause before acting
- Restart individual services, not systems
- Verify backups before database operations
- When in doubt, ask the user

