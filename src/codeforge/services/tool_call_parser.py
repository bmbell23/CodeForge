"""Parser for Augment CLI tool calls and output formatting."""

import re
from typing import List, Dict, Any, Tuple, Optional
from dataclasses import dataclass
from enum import Enum


class ToolCallDisplayMode(Enum):
    """How to display tool calls to users."""
    HIDE = "hide"  # Hide tool calls completely
    MINIMAL = "minimal"  # Show minimal info like "🔧 Using codebase-retrieval"
    DETAILED = "detailed"  # Show full tool call details
    RAW = "raw"  # Show raw tool calls (current behavior)


@dataclass
class ToolCall:
    """Represents a parsed tool call."""
    name: str
    parameters: Dict[str, Any]
    result: Optional[str] = None
    raw_call: str = ""
    raw_result: str = ""


@dataclass
class ParsedOutput:
    """Represents parsed Augment CLI output."""
    user_content: str  # Content to show to users
    tool_calls: List[ToolCall]  # Extracted tool calls
    raw_content: str  # Original content


class ToolCallParser:
    """Parser for Augment CLI tool calls and output."""

    # Pattern to match ANSI escape codes
    ANSI_ESCAPE = re.compile(r'\x1B(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])')

    # Simpler patterns that work with the actual format
    TOOL_CALL_START = re.compile(r'🔧 Tool call: (.+)')
    TOOL_RESULT_START = re.compile(r'📋 Tool result: (.+)')
    
    def __init__(self, display_mode: ToolCallDisplayMode = ToolCallDisplayMode.MINIMAL):
        """Initialize the parser.
        
        Args:
            display_mode: How to display tool calls to users
        """
        self.display_mode = display_mode
    
    def parse(self, content: str) -> ParsedOutput:
        """Parse Augment CLI output to extract tool calls and user content.
        
        Args:
            content: Raw output from Augment CLI
            
        Returns:
            ParsedOutput with separated user content and tool calls
        """
        # Store original content
        raw_content = content
        
        # Extract tool calls
        tool_calls = self._extract_tool_calls(content)
        
        # Generate user-friendly content based on display mode
        user_content = self._generate_user_content(content, tool_calls)
        
        return ParsedOutput(
            user_content=user_content,
            tool_calls=tool_calls,
            raw_content=raw_content
        )
    
    def _extract_tool_calls(self, content: str) -> List[ToolCall]:
        """Extract tool calls from the content."""
        tool_calls = []

        # Remove ANSI codes first for easier parsing
        clean_content = self.ANSI_ESCAPE.sub('', content)
        lines = clean_content.split('\n')

        i = 0
        while i < len(lines):
            line = lines[i].strip()

            # Look for tool call start
            call_match = self.TOOL_CALL_START.match(line)
            if call_match:
                tool_name = call_match.group(1).strip()

                # Collect parameters until we hit the result or another tool call
                params_lines = []
                i += 1
                while i < len(lines):
                    next_line = lines[i].strip()
                    if (self.TOOL_RESULT_START.match(next_line) or
                        self.TOOL_CALL_START.match(next_line) or
                        next_line == '🤖'):
                        break
                    if next_line:  # Skip empty lines
                        params_lines.append(next_line)
                    i += 1

                # Create tool call
                tool_call = ToolCall(
                    name=tool_name,
                    parameters=self._parse_parameters('\n'.join(params_lines)),
                    raw_call=f"🔧 Tool call: {tool_name}\n" + '\n'.join(params_lines)
                )

                # Look for matching result
                if i < len(lines):
                    result_line = lines[i].strip()
                    result_match = self.TOOL_RESULT_START.match(result_line)
                    if result_match and result_match.group(1).strip() == tool_name:
                        # Collect result content
                        result_lines = []
                        i += 1
                        while i < len(lines):
                            next_line = lines[i].strip()
                            if (self.TOOL_CALL_START.match(next_line) or
                                next_line == '🤖'):
                                break
                            result_lines.append(lines[i])  # Keep original formatting
                            i += 1

                        tool_call.result = '\n'.join(result_lines).strip()
                        tool_call.raw_result = f"📋 Tool result: {tool_name}\n" + '\n'.join(result_lines)

                tool_calls.append(tool_call)
                continue

            i += 1

        return tool_calls
    
    def _parse_parameters(self, params_text: str) -> Dict[str, Any]:
        """Parse tool call parameters from text."""
        params = {}
        
        # Simple parameter parsing - look for key: value patterns
        lines = params_text.strip().split('\n')
        for line in lines:
            line = line.strip()
            if ':' in line and not line.startswith('//'):
                key, value = line.split(':', 1)
                key = key.strip().strip('"\'')
                value = value.strip().strip('"\'')
                params[key] = value
        
        return params
    
    def _generate_user_content(self, content: str, tool_calls: List[ToolCall]) -> str:
        """Generate user-friendly content based on display mode."""
        if self.display_mode == ToolCallDisplayMode.RAW:
            return content

        if self.display_mode == ToolCallDisplayMode.HIDE:
            # Remove all tool call sections, then clean ANSI codes
            user_content = self._remove_tool_calls(content)
            user_content = self.ANSI_ESCAPE.sub('', user_content)
        elif self.display_mode == ToolCallDisplayMode.MINIMAL:
            # Replace tool calls with minimal indicators, then clean ANSI codes
            user_content = self._replace_with_minimal_indicators(content, tool_calls)
            user_content = self.ANSI_ESCAPE.sub('', user_content)
        elif self.display_mode == ToolCallDisplayMode.DETAILED:
            # Keep tool calls but format them nicely, then clean ANSI codes
            user_content = self._format_detailed_tool_calls(content, tool_calls)
            user_content = self.ANSI_ESCAPE.sub('', user_content)
        else:
            # Remove ANSI escape codes
            user_content = self.ANSI_ESCAPE.sub('', content)

        return user_content.strip()
    
    def _remove_tool_calls(self, content: str) -> str:
        """Remove all tool call sections from content."""
        lines = content.split('\n')
        filtered_lines = []
        in_tool_section = False

        for line in lines:
            # Check if we're entering a tool section (with or without ANSI codes)
            if ('🔧 Tool call:' in line or '📋 Tool result:' in line or
                '[90m🔧 Tool call:' in line or '[90m📋 Tool result:' in line):
                in_tool_section = True
                continue

            # Check if we're exiting a tool section (robot emoji or new content)
            stripped = line.strip()
            if stripped == '🤖':
                in_tool_section = False
                continue  # Skip the robot emoji itself
            elif in_tool_section and stripped and not stripped.startswith(' ') and not stripped.startswith('\t'):
                # This looks like new content after a tool section
                in_tool_section = False
                filtered_lines.append(line)
            elif not in_tool_section:
                filtered_lines.append(line)

        return '\n'.join(filtered_lines)
    
    def _replace_with_minimal_indicators(self, content: str, tool_calls: List[ToolCall]) -> str:
        """Replace tool calls with minimal indicators."""
        lines = content.split('\n')
        result_lines = []
        in_tool_section = False

        for line in lines:
            # Check if we're entering a tool call section (with or without ANSI codes)
            if ('🔧 Tool call:' in line or '[90m🔧 Tool call:' in line):
                # Extract tool name and add minimal indicator
                if '[90m🔧 Tool call:' in line:
                    tool_name = line.split('[90m🔧 Tool call:')[1].split('[0m')[0].strip()
                else:
                    tool_name = line.replace('🔧 Tool call:', '').strip()
                result_lines.append(f"🔧 *Using {tool_name}...*")
                in_tool_section = True
                continue

            # Check if we're in a tool result section
            if ('📋 Tool result:' in line or '[90m📋 Tool result:' in line):
                in_tool_section = True
                continue

            # Check if we're exiting a tool section
            stripped = line.strip()
            if stripped == '🤖':
                in_tool_section = False
                continue  # Skip robot emoji
            elif in_tool_section and stripped and not stripped.startswith(' ') and not stripped.startswith('\t'):
                # New content after tool section
                in_tool_section = False
                result_lines.append(line)
            elif not in_tool_section:
                result_lines.append(line)

        # Clean up extra whitespace
        result = '\n'.join(result_lines)
        result = re.sub(r'\n\s*\n\s*\n', '\n\n', result)

        return result
    
    def _format_detailed_tool_calls(self, content: str, tool_calls: List[ToolCall]) -> str:
        """Format tool calls in a detailed but user-friendly way."""
        result = content
        
        for tool_call in tool_calls:
            # Create detailed but clean formatting
            formatted_call = f"🔧 **Tool: {tool_call.name}**"
            if tool_call.parameters:
                params_str = ", ".join([f"{k}={v}" for k, v in tool_call.parameters.items()])
                formatted_call += f" ({params_str})"
            
            # Replace raw call
            if tool_call.raw_call:
                clean_raw_call = self.ANSI_ESCAPE.sub('', tool_call.raw_call)
                result = result.replace(clean_raw_call, formatted_call)
            
            # Format result if present
            if tool_call.result and tool_call.raw_result:
                formatted_result = f"📋 **Result:**\n{tool_call.result}"
                clean_raw_result = self.ANSI_ESCAPE.sub('', tool_call.raw_result)
                result = result.replace(clean_raw_result, formatted_result)
        
        return result
