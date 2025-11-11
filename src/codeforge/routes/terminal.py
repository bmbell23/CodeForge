"""Terminal WebSocket routes."""

import asyncio
import os
import pty
import select
import struct
import termios
import fcntl
from pathlib import Path
from typing import Dict
from fastapi import APIRouter, WebSocket, WebSocketDisconnect, Depends
from sqlalchemy.orm import Session

from ..database import get_db
from ..models.project import Project
from ..config import settings

router = APIRouter()

# Store active terminal sessions
active_terminals: Dict[str, dict] = {}


@router.websocket("/ws/{project_id}")
async def terminal_websocket(
    websocket: WebSocket,
    project_id: int,
    db: Session = Depends(get_db)
):
    """WebSocket endpoint for terminal sessions."""
    await websocket.accept()
    
    # Get user from token (we'll need to extract it from query params for WebSocket)
    # For now, we'll skip auth in WebSocket and rely on the HTTP auth
    # In production, you'd want to validate the token from query params
    
    try:
        # Get project
        project = db.query(Project).filter(Project.id == project_id).first()
        if not project:
            await websocket.send_json({
                "type": "error",
                "message": "Project not found"
            })
            await websocket.close()
            return
        
        # Create a pseudo-terminal
        master_fd, slave_fd = pty.openpty()
        
        # Start bash in the project directory
        pid = os.fork()
        
        if pid == 0:  # Child process
            # Close master fd in child
            os.close(master_fd)
            
            # Create new session
            os.setsid()
            
            # Set controlling terminal
            fcntl.ioctl(slave_fd, termios.TIOCSCTTY, 0)
            
            # Duplicate slave fd to stdin, stdout, stderr
            os.dup2(slave_fd, 0)
            os.dup2(slave_fd, 1)
            os.dup2(slave_fd, 2)
            
            # Close slave fd
            if slave_fd > 2:
                os.close(slave_fd)

            # Change to project directory (combine projects_root with relative path)
            project_full_path = Path(settings.projects_root) / project.path
            os.chdir(str(project_full_path))

            # Execute bash
            os.execvp('/bin/bash', ['/bin/bash'])
        
        else:  # Parent process
            # Close slave fd in parent
            os.close(slave_fd)
            
            # Set non-blocking
            flags = fcntl.fcntl(master_fd, fcntl.F_GETFL)
            fcntl.fcntl(master_fd, fcntl.F_SETFL, flags | os.O_NONBLOCK)
            
            # Store terminal info
            session_id = f"{project_id}_{pid}"
            active_terminals[session_id] = {
                'master_fd': master_fd,
                'pid': pid,
                'websocket': websocket
            }
            
            # Create tasks for reading from terminal and WebSocket
            async def read_from_terminal():
                """Read output from terminal and send to WebSocket."""
                while True:
                    try:
                        # Use select to check if data is available
                        readable, _, _ = select.select([master_fd], [], [], 0.1)
                        if readable:
                            data = os.read(master_fd, 1024)
                            if data:
                                await websocket.send_json({
                                    "type": "output",
                                    "data": data.decode('utf-8', errors='ignore')
                                })
                        await asyncio.sleep(0.01)
                    except OSError:
                        break
                    except Exception as e:
                        print(f"Error reading from terminal: {e}")
                        break
            
            async def read_from_websocket():
                """Read input from WebSocket and send to terminal."""
                while True:
                    try:
                        message = await websocket.receive_json()
                        if message.get('type') == 'input':
                            data = message.get('data', '')
                            os.write(master_fd, data.encode('utf-8'))
                        elif message.get('type') == 'resize':
                            # Handle terminal resize
                            rows = message.get('rows', 24)
                            cols = message.get('cols', 80)
                            winsize = struct.pack('HHHH', rows, cols, 0, 0)
                            fcntl.ioctl(master_fd, termios.TIOCSWINSZ, winsize)
                    except WebSocketDisconnect:
                        break
                    except Exception as e:
                        print(f"Error reading from WebSocket: {e}")
                        break
            
            # Run both tasks concurrently
            try:
                await asyncio.gather(
                    read_from_terminal(),
                    read_from_websocket()
                )
            finally:
                # Cleanup
                try:
                    os.close(master_fd)
                    os.kill(pid, 9)
                    os.waitpid(pid, 0)
                except:
                    pass
                
                if session_id in active_terminals:
                    del active_terminals[session_id]
    
    except WebSocketDisconnect:
        print("Terminal WebSocket disconnected")
    except Exception as e:
        print(f"Terminal error: {e}")
        try:
            await websocket.send_json({
                "type": "error",
                "message": str(e)
            })
        except:
            pass
    finally:
        try:
            await websocket.close()
        except:
            pass

