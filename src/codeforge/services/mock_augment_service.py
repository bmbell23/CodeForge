"""Mock Augment service for development/testing without actual Augment CLI."""

import asyncio
from typing import AsyncGenerator, Optional


class MockAugmentService:
    """Mock service that simulates Augment CLI responses."""

    def __init__(self, project_path: Optional[str] = None):
        """Initialize the mock Augment service.
        
        Args:
            project_path: Relative path to the project (not used in mock)
        """
        self.project_path = project_path

    async def stream_response(self, prompt: str) -> AsyncGenerator[str, None]:
        """Simulate streaming response from Augment CLI.
        
        Args:
            prompt: The user's prompt/message
            
        Yields:
            Chunks of the response as they arrive
        """
        # Simulate a helpful AI response
        response = f"""I understand you asked: "{prompt}"

Here's a mock response from the Augment AI assistant:

This is a simulated response since the actual Augment CLI is not installed.
In a real scenario, I would analyze your codebase and provide specific,
context-aware suggestions.

For now, I can help you understand that:
- Your project is located at: {self.project_path or 'No project selected'}
- You asked: {prompt}
- This is a mock response for development/testing

To use the real Augment CLI:
1. Install it: npm install -g @augmentcode/auggie
2. Login: auggie login
3. Update config.py to use AugmentService instead of MockAugmentService

Would you like me to help with anything else?
"""
        
        # Simulate streaming by yielding chunks with delays
        words = response.split()
        for i, word in enumerate(words):
            chunk = word + (" " if i < len(words) - 1 else "")
            yield chunk
            # Simulate network delay
            await asyncio.sleep(0.05)

    async def execute_command(self, prompt: str) -> str:
        """Execute a command and return the full response.
        
        Args:
            prompt: The user's prompt/message
            
        Returns:
            The complete response from mock Augment
        """
        response = ""
        async for chunk in self.stream_response(prompt):
            response += chunk
        return response

    async def check_auggie_installed(self) -> bool:
        """Check if Augment CLI is installed.
        
        Returns:
            False (since this is a mock)
        """
        return False

