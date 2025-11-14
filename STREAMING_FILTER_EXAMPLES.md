# Streaming Tool Call Filter - Output Examples

This document shows how the streaming filter transforms Augment CLI output in different display modes.

## Raw Output (Before Filtering)

This is what Augment CLI actually outputs:

```
🔧 Tool call: view
path: src/life_forge/templates/health.html
type: file

📋 Tool result: view
Here's the result of running cat -n on src/life_forge/templates/health.html:
230
231
232 Date
233 Weight
234 Change
235 BMI
236 Actions
237
238
239
240
Total lines in file: 938

🤖

Perfect! The changes have been successfully implemented. Let me summarize what was removed from the Health page:
```

## MINIMAL Mode (Default) ✨

Clean, concise output that shows tool usage without the noise:

```
🔧 *Using view...*

Perfect! The changes have been successfully implemented. Let me summarize what was removed from the Health page:
```

**Benefits:**
- Users know a tool was used
- No verbose output cluttering the response
- Maintains context of what's happening
- Clean, professional appearance

## HIDE Mode

Completely hides all tool calls:

```
Perfect! The changes have been successfully implemented. Let me summarize what was removed from the Health page:
```

**Benefits:**
- Cleanest possible output
- Users only see the AI's response
- Good for non-technical users

**Drawbacks:**
- No indication that tools were used
- May seem like AI is "guessing" when it's actually checking code

## DETAILED Mode

Shows formatted tool information:

```
🔧 **Tool: view**

Perfect! The changes have been successfully implemented. Let me summarize what was removed from the Health page:
```

**Benefits:**
- More information about what tools are being used
- Still cleaner than raw output
- Good for debugging or understanding AI's process

## RAW Mode

Shows everything exactly as Augment CLI outputs it (same as "Before Filtering" example above).

**Use Cases:**
- Debugging
- Development
- When you need to see exactly what's happening

---

## How It Works

The `StreamingToolCallFilter` processes output **line-by-line** as it arrives:

1. **Detects tool markers**: `🔧 Tool call:`, `📋 Tool result:`, `🤖`
2. **Tracks state**: Knows when we're inside a tool section
3. **Filters in real-time**: Decides what to yield based on display mode
4. **No buffering**: Processes each line immediately, maintaining streaming performance

### Key Features:

✅ **Real-time processing** - No waiting for complete responses
✅ **Line-by-line filtering** - Minimal memory usage
✅ **State tracking** - Knows when we're in tool sections
✅ **ANSI code removal** - Strips terminal formatting codes
✅ **Configurable** - Four display modes to choose from

### Performance Impact:

- **Negligible** - Simple string matching and state tracking
- **No buffering** - Processes and yields immediately
- **Streaming maintained** - Users see output as it's generated

---

## Recommendation

**Use MINIMAL mode (default)** for the best user experience:
- Clean output without noise
- Users know tools are being used
- Professional appearance
- Maintains transparency

Only switch to other modes if you have specific needs:
- **HIDE**: For completely clean output (non-technical users)
- **DETAILED**: For more visibility into tool usage
- **RAW**: For debugging or development

