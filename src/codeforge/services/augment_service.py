"""Service for interacting with Augment CLI."""

import asyncio
import os
from pathlib import Path
from typing import AsyncGenerator, Optional

from ..config import settings


class AugmentService:
    """Service for interacting with Augment CLI (auggie)."""

    def __init__(self, project_path: Optional[str] = None):
        """Initialize the Augment service.
        
        Args:
            project_path: Relative path to the project (relative to projects_root)
        """
        self.auggie_path = settings.auggie_path
        self.project_path = project_path
        
        # Build full project path
        if project_path:
            self.full_project_path = Path(settings.projects_root) / project_path
        else:
            self.full_project_path = Path(settings.projects_root)

    async def stream_response(self, prompt: str) -> AsyncGenerator[str, None]:
        """Stream response from Augment CLI.
        
        Args:
            prompt: The user's prompt/message
            
        Yields:
            Chunks of the response as they arrive
        """
        # Build command
        # Using --print flag for non-interactive mode
        cmd = [
            self.auggie_path,
            "--print",
            prompt
        ]
        
        # Set working directory to project path
        cwd = str(self.full_project_path) if self.full_project_path.exists() else None
        
        try:
            # Start the process
            process = await asyncio.create_subprocess_exec(
                *cmd,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                cwd=cwd
            )
            
            # Stream output
            if process.stdout:
                while True:
                    line = await process.stdout.readline()
                    if not line:
                        break
                    
                    # Decode and yield the line
                    chunk = line.decode('utf-8')
                    yield chunk
            
            # Wait for process to complete
            await process.wait()
            
            # Check for errors
            if process.returncode != 0 and process.stderr:
                stderr = await process.stderr.read()
                error_msg = stderr.decode('utf-8')
                yield f"\n\n[Error: {error_msg}]"
        
        except FileNotFoundError:
            yield "[Error: Augment CLI (auggie) not found. Please ensure it's installed and in your PATH.]"
        except Exception as e:
            yield f"[Error: {str(e)}]"

    async def execute_command(self, prompt: str) -> str:
        """Execute a command and return the full response.
        
        Args:
            prompt: The user's prompt/message
            
        Returns:
            The complete response from Augment
        """
        response = ""
        async for chunk in self.stream_response(prompt):
            response += chunk
        return response

    async def check_auggie_installed(self) -> bool:
        """Check if Augment CLI is installed and accessible.
        
        Returns:
            True if auggie is installed, False otherwise
        """
        try:
            process = await asyncio.create_subprocess_exec(
                self.auggie_path,
                "--version",
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE
            )
            await process.wait()
            return process.returncode == 0
        except FileNotFoundError:
            return False
        except Exception:
            return False

