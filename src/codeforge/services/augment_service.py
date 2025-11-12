"""Service for interacting with Augment CLI."""

import asyncio
import os
from pathlib import Path
from typing import AsyncGenerator, Optional

from ..config import settings
from .tool_call_parser import ToolCallParser, ToolCallDisplayMode


class AugmentService:
    """Service for interacting with Augment CLI (auggie)."""

    def __init__(self, project_path: Optional[str] = None, tool_call_mode: ToolCallDisplayMode = ToolCallDisplayMode.MINIMAL):
        """Initialize the Augment service.

        Args:
            project_path: Relative path to the project (relative to projects_root)
            tool_call_mode: How to display tool calls to users
        """
        self.auggie_path = settings.auggie_path
        self.project_path = project_path
        self.tool_call_parser = ToolCallParser(display_mode=tool_call_mode)

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
            Chunks of the response as they arrive (processed through tool call parser)
        """
        # First, get the complete response
        full_response = await self.execute_command(prompt)

        # Parse the response to handle tool calls
        parsed_output = self.tool_call_parser.parse(full_response)

        # Stream the parsed user content
        words = parsed_output.user_content.split()
        for i, word in enumerate(words):
            chunk = word + (" " if i < len(words) - 1 else "")
            yield chunk
            # Small delay to simulate streaming
            await asyncio.sleep(0.02)

    async def stream_response_raw(self, prompt: str) -> AsyncGenerator[str, None]:
        """Stream raw response from Augment CLI without tool call processing.

        Args:
            prompt: The user's prompt/message

        Yields:
            Raw chunks of the response as they arrive
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
            The complete raw response from Augment (before tool call processing)
        """
        response = ""
        async for chunk in self.stream_response_raw(prompt):
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

