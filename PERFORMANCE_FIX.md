# Performance Fix - Chat Response Streaming

## Problem Identified

The chat interface was experiencing severe performance issues where:
- Sending a message would result in no response for an extended period
- Users would wait indefinitely without seeing any feedback
- The app felt completely unresponsive

## Root Cause

The issue was in `src/codeforge/services/augment_service.py` in the `stream_response()` method (lines 32-53).

### What Was Happening:

```python
async def stream_response(self, prompt: str) -> AsyncGenerator[str, None]:
    # First, get the complete response
    full_response = await self.execute_command(prompt)  # ❌ BLOCKING!
    
    # Parse the response to handle tool calls
    parsed_output = self.tool_call_parser.parse(full_response)
    
    # Stream the parsed user content
    words = parsed_output.user_content.split()
    for i, word in enumerate(words):
        chunk = word + (" " if i < len(words) - 1 else "")
        yield chunk
        await asyncio.sleep(0.02)  # Artificial delay
```

**The Problem:**
1. The method waited for the **ENTIRE** response from Augment CLI to complete before doing anything
2. Only after receiving the complete response would it start "streaming" word-by-word with artificial delays
3. This meant users saw nothing until Augment CLI finished processing (could be minutes for complex queries)
4. The streaming was fake - it was just replaying already-received content slowly

## Solution

Changed the `stream_response()` method to actually stream in real-time:

```python
async def stream_response(self, prompt: str) -> AsyncGenerator[str, None]:
    # Stream the response in real-time as it comes from Augment CLI
    async for chunk in self.stream_response_raw(prompt):
        yield chunk
```

**What This Does:**
1. Streams chunks from Augment CLI as they arrive in real-time
2. Users see responses immediately as Augment generates them
3. No artificial delays or waiting for complete responses
4. True streaming behavior

## Trade-offs

**What We Gained:**
- ✅ Real-time streaming responses
- ✅ Immediate feedback to users
- ✅ Much better perceived performance
- ✅ No more "hanging" or unresponsive feeling

**What We Lost (Temporarily):**
- ⚠️ Tool call parsing/filtering (the `ToolCallParser` functionality)
- Users will now see raw output from Augment CLI including tool call information

**Note:** The tool call parser was designed to clean up the output and hide/minimize tool call details. However, it required waiting for the complete response before parsing, which caused the performance issue. If needed, we can implement streaming tool call filtering in the future, but for now, raw output is much better than no output.

## Update: Streaming Tool Call Filter Added

After the initial fix, a **streaming tool call filter** was implemented to clean up the output while maintaining real-time streaming performance.

### New Implementation:

Created `StreamingToolCallFilter` class in `tool_call_parser.py` that:
- Processes chunks line-by-line as they arrive
- Detects tool call markers (`🔧 Tool call:`, `📋 Tool result:`, `🤖`)
- Filters output based on display mode (MINIMAL by default)
- Maintains real-time streaming - no buffering of complete responses

**Display Modes:**
- `MINIMAL` (default): Shows `🔧 *Using tool_name...*` instead of verbose tool output
- `HIDE`: Completely hides tool calls and results
- `DETAILED`: Shows formatted tool calls with parameters
- `RAW`: Shows everything unfiltered

### Result:
✅ Real-time streaming maintained
✅ Clean, user-friendly output
✅ Tool call noise removed
✅ No performance impact

## Files Changed

- `src/codeforge/services/augment_service.py` - Modified `stream_response()` method to use streaming filter
- `src/codeforge/services/tool_call_parser.py` - Added `StreamingToolCallFilter` class

## Deployment

The fix has been applied and the service has been restarted:
```bash
sudo systemctl restart codeforge
```

Service is now running with the performance fix applied.

## Testing

To verify the fix:
1. Open the CodeForge chat interface
2. Send a message
3. You should now see the response streaming in real-time as Augment generates it
4. No more long waits with no feedback

## Configuration

To change the display mode, modify the `tool_call_mode` parameter when initializing `AugmentService`:

```python
# In src/codeforge/routes/chat.py
augment_service = AugmentService(
    project_path=project_path,
    tool_call_mode=ToolCallDisplayMode.MINIMAL  # or HIDE, DETAILED, RAW
)
```

Default is `MINIMAL` which provides the best balance of clean output and useful information.

