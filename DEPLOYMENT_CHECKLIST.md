# CodeForge Deployment Checklist

Use this checklist to ensure a smooth deployment to production.

## Pre-Deployment

- [ ] Review all code changes in `DEPLOYMENT_CHANGES.md`
- [ ] Test the application in development mode
  ```bash
  cd /home/brandon/projects/CodeForge
  source venv/bin/activate
  python scripts/server.py
  # Visit http://localhost:8005
  ```
- [ ] Verify all features work:
  - [ ] Login/Register
  - [ ] Create/switch projects
  - [ ] Chat with Augment
  - [ ] File browsing
  - [ ] Git status
  - [ ] Terminal access
- [ ] Commit all changes to git
  ```bash
  git add .
  git commit -m "Prepare for production deployment"
  ```

## Deployment

- [ ] Run the deployment script
  ```bash
  cd /home/brandon/projects/CodeForge
  ./scripts/deploy.sh
  ```
- [ ] Verify the script completed successfully
- [ ] Check that the service is running
  ```bash
  sudo systemctl status codeforge
  ```
- [ ] View the logs for any errors
  ```bash
  sudo journalctl -u codeforge -n 50
  ```

## Nginx Configuration

- [ ] Edit the main nginx configuration
  ```bash
  sudo nano /etc/nginx/sites-available/forge-freedom.conf
  ```
- [ ] Add the CodeForge configuration block (from `config/nginx/codeforge.conf`)
- [ ] Test nginx configuration
  ```bash
  sudo nginx -t
  ```
- [ ] If test passes, reload nginx
  ```bash
  sudo systemctl reload nginx
  ```

## Post-Deployment Testing

- [ ] Visit https://forge-freedom.com/code/
- [ ] Verify the login page loads correctly
- [ ] Test user registration
  - [ ] Create a new account
  - [ ] Verify email validation (if applicable)
- [ ] Test user login
  - [ ] Login with the new account
  - [ ] Verify redirect to dashboard
- [ ] Test project management
  - [ ] Scan for projects
  - [ ] Add a project
  - [ ] Switch between projects
- [ ] Test chat functionality
  - [ ] Create a new conversation
  - [ ] Send a message to Augment
  - [ ] Verify WebSocket connection works
  - [ ] Check markdown rendering
  - [ ] Rename a conversation
- [ ] Test file browsing
  - [ ] View file tree
  - [ ] Open a file
  - [ ] Edit and save a file
- [ ] Test git integration
  - [ ] View git status
  - [ ] View git log
- [ ] Test terminal
  - [ ] Open terminal
  - [ ] Run basic commands (ls, pwd, etc.)
  - [ ] Switch projects and verify terminal switches
  - [ ] Verify terminal sessions persist when switching back
- [ ] Test static files
  - [ ] Verify CSS loads correctly
  - [ ] Verify JavaScript loads correctly
  - [ ] Check browser console for errors

## Monitoring

- [ ] Set up log monitoring
  ```bash
  sudo journalctl -u codeforge -f
  ```
- [ ] Monitor for errors in the first hour
- [ ] Check nginx access logs
  ```bash
  sudo tail -f /var/log/nginx/access.log | grep /code/
  ```
- [ ] Check nginx error logs
  ```bash
  sudo tail -f /var/log/nginx/error.log
  ```

## Security Verification

- [ ] Verify HTTPS is working (no mixed content warnings)
- [ ] Check that HTTP redirects to HTTPS
- [ ] Verify authentication is required for dashboard
- [ ] Test that unauthenticated users can't access protected routes
- [ ] Verify WebSocket connections use WSS (secure WebSocket)
- [ ] Check that the secret key is properly set and secure
- [ ] Verify file access is restricted to projects directory

## Performance

- [ ] Test page load times
- [ ] Verify WebSocket connections are stable
- [ ] Check terminal responsiveness
- [ ] Monitor memory usage
  ```bash
  ps aux | grep codeforge
  ```
- [ ] Monitor CPU usage
  ```bash
  top -p $(pgrep -f codeforge)
  ```

## Backup

- [ ] Backup the database
  ```bash
  cp /home/brandon/projects/CodeForge/codeforge.db \
     /home/brandon/backups/codeforge-$(date +%Y%m%d).db
  ```
- [ ] Backup the .env file
  ```bash
  cp /home/brandon/projects/CodeForge/.env \
     /home/brandon/backups/codeforge-env-$(date +%Y%m%d).txt
  ```
- [ ] Document the deployment
  - [ ] Note the deployment date and time
  - [ ] Record any issues encountered
  - [ ] Document any manual changes made

## Troubleshooting (If Issues Occur)

### Service won't start
- [ ] Check logs: `sudo journalctl -u codeforge -n 100`
- [ ] Verify port 8005 is not in use: `sudo lsof -i :8005`
- [ ] Check .env file exists and is valid
- [ ] Verify virtual environment is set up correctly
- [ ] Check file permissions

### Nginx errors
- [ ] Check nginx error log: `sudo tail -f /var/log/nginx/error.log`
- [ ] Verify nginx configuration: `sudo nginx -t`
- [ ] Check that the service is running: `sudo systemctl status codeforge`
- [ ] Verify port 8005 is accessible: `curl http://localhost:8005/health`

### WebSocket connection fails
- [ ] Verify nginx WebSocket configuration
- [ ] Check browser console for errors
- [ ] Verify SSL certificate is valid
- [ ] Check firewall settings

### Terminal not working
- [ ] Verify bash is installed: `which bash`
- [ ] Check user permissions for projects directory
- [ ] View terminal WebSocket logs in browser console
- [ ] Check service logs for PTY errors

### Static files not loading
- [ ] Verify static files exist in `src/codeforge/static/`
- [ ] Check nginx configuration for `/code/static/` location
- [ ] Verify file permissions
- [ ] Check browser console for 404 errors

## Rollback (If Needed)

- [ ] Stop the service
  ```bash
  sudo systemctl stop codeforge
  sudo systemctl disable codeforge
  ```
- [ ] Remove systemd service
  ```bash
  sudo rm /etc/systemd/system/codeforge.service
  sudo systemctl daemon-reload
  ```
- [ ] Remove nginx configuration
  ```bash
  sudo nano /etc/nginx/sites-available/forge-freedom.conf
  # Remove the CodeForge block
  sudo nginx -t
  sudo systemctl reload nginx
  ```
- [ ] Restore from backup if needed
  ```bash
  cp /home/brandon/backups/codeforge-YYYYMMDD.db \
     /home/brandon/projects/CodeForge/codeforge.db
  ```

## Post-Deployment

- [ ] Update documentation with any changes
- [ ] Notify users that CodeForge is live
- [ ] Set up automated backups (cron job)
- [ ] Set up monitoring/alerting (optional)
- [ ] Plan for regular updates and maintenance

## Success Criteria

✅ Deployment is successful when:
- Service is running and stable
- All features work correctly
- No errors in logs
- HTTPS is working
- WebSocket connections are stable
- Terminal is functional
- Performance is acceptable
- Security checks pass

---

**Deployment Date**: _________________

**Deployed By**: _________________

**Notes**: 
_________________________________________________________________
_________________________________________________________________
_________________________________________________________________

